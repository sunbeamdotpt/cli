"""Image building, mirroring, and pushing to Gitea registry."""
import base64
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

from sunbeam.config import get_repo_root as _get_repo_root
from sunbeam.kube import kube, kube_out, get_lima_ip
from sunbeam.output import step, ok, warn, die

LIMA_VM = "sunbeam"
GITEA_ADMIN_USER = "gitea_admin"
MANAGED_NS = ["data", "devtools", "ingress", "lasuite", "matrix", "media", "ory",
              "storage", "vault-secrets-operator"]

AMD64_ONLY_IMAGES = [
    ("docker.io/lasuite/people-backend:latest",    "studio", "people-backend",    "latest"),
    ("docker.io/lasuite/people-frontend:latest",   "studio", "people-frontend",   "latest"),
    ("docker.io/lasuite/impress-backend:latest",   "studio", "impress-backend",   "latest"),
    ("docker.io/lasuite/impress-frontend:latest",  "studio", "impress-frontend",  "latest"),
    ("docker.io/lasuite/impress-y-provider:latest","studio", "impress-y-provider","latest"),
]

_MIRROR_SCRIPT_BODY = r'''
import json, hashlib, io, tarfile, os, subprocess, urllib.request

CONTENT_STORE = (
    "/var/lib/rancher/k3s/agent/containerd"
    "/io.containerd.content.v1.content/blobs/sha256"
)

def blob_path(h):
    return os.path.join(CONTENT_STORE, h)

def blob_exists(h):
    return os.path.exists(blob_path(h))

def read_blob(h):
    with open(blob_path(h), "rb") as f:
        return f.read()

def add_tar_entry(tar, name, data):
    info = tarfile.TarInfo(name=name)
    info.size = len(data)
    tar.addfile(info, io.BytesIO(data))

def get_image_digest(ref):
    r = subprocess.run(
        ["ctr", "-n", "k8s.io", "images", "ls", "name==" + ref],
        capture_output=True, text=True,
    )
    for line in r.stdout.splitlines():
        if ref in line:
            for part in line.split():
                if part.startswith("sha256:"):
                    return part[7:]
    return None

def fetch_index_from_registry(repo, tag):
    url = (
        "https://auth.docker.io/token"
        f"?service=registry.docker.io&scope=repository:{repo}:pull"
    )
    with urllib.request.urlopen(url) as resp:
        token = json.loads(resp.read())["token"]
    accept = ",".join([
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
    ])
    req = urllib.request.Request(
        f"https://registry-1.docker.io/v2/{repo}/manifests/{tag}",
        headers={"Authorization": f"Bearer {token}", "Accept": accept},
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read())

def make_oci_tar(ref, new_index_bytes, amd64_manifest_bytes):
    ix_hex    = hashlib.sha256(new_index_bytes).hexdigest()
    amd64_hex = json.loads(new_index_bytes)["manifests"][0]["digest"].replace("sha256:", "")
    layout = json.dumps({"imageLayoutVersion": "1.0.0"}).encode()
    top = json.dumps({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "digest": f"sha256:{ix_hex}",
            "size": len(new_index_bytes),
            "annotations": {"org.opencontainers.image.ref.name": ref},
        }],
    }, separators=(",", ":")).encode()
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:") as tar:
        add_tar_entry(tar, "oci-layout", layout)
        add_tar_entry(tar, "index.json", top)
        add_tar_entry(tar, f"blobs/sha256/{ix_hex}", new_index_bytes)
        add_tar_entry(tar, f"blobs/sha256/{amd64_hex}", amd64_manifest_bytes)
    return buf.getvalue()

def import_ref(ref, tar_bytes):
    subprocess.run(["ctr", "-n", "k8s.io", "images", "rm", ref], capture_output=True)
    r = subprocess.run(
        ["ctr", "-n", "k8s.io", "images", "import", "--all-platforms", "-"],
        input=tar_bytes, capture_output=True,
    )
    if r.returncode:
        print(f"    import failed: {r.stderr.decode()}")
        return False
    subprocess.run(
        ["ctr", "-n", "k8s.io", "images", "label", ref, "io.cri-containerd.image=managed"],
        capture_output=True,
    )
    return True

def process(src, tgt, user, pwd):
    print(f"  {src}")

    # Pull by tag — may fail on arm64-only images but still puts the index blob in the store
    subprocess.run(["ctr", "-n", "k8s.io", "images", "pull", src], capture_output=True)

    ix_hex = get_image_digest(src)
    if ix_hex and blob_exists(ix_hex):
        index = json.loads(read_blob(ix_hex))
    else:
        print("    index not in content store — fetching from docker.io...")
        no_prefix = src.replace("docker.io/", "")
        parts = no_prefix.split(":", 1)
        repo, tag = parts[0], (parts[1] if len(parts) > 1 else "latest")
        index = fetch_index_from_registry(repo, tag)

    amd64 = next(
        (m for m in index.get("manifests", [])
         if m.get("platform", {}).get("architecture") == "amd64"
         and m.get("platform", {}).get("os") == "linux"),
        None,
    )
    if not amd64:
        print("    skip: no linux/amd64 entry in index")
        return

    amd64_hex = amd64["digest"].replace("sha256:", "")

    # Always pull by digest with --platform linux/amd64 to ensure all layer
    # blobs are downloaded to the content store (the index pull in step 1 only
    # fetches the manifest blob, not the layers, on an arm64 host).
    print("    pulling amd64 manifest + layers by digest...")
    repo_base = src.rsplit(":", 1)[0]
    subprocess.run(
        ["ctr", "-n", "k8s.io", "images", "pull",
         "--platform", "linux/amd64",
         f"{repo_base}@sha256:{amd64_hex}"],
        capture_output=True,
    )
    if not blob_exists(amd64_hex):
        print("    failed: amd64 manifest blob missing after pull")
        return

    amd64_bytes = read_blob(amd64_hex)

    # Patched index: keep amd64 + add arm64 alias pointing at same manifest
    arm64 = {
        "mediaType": amd64["mediaType"],
        "digest":    amd64["digest"],
        "size":      amd64["size"],
        "platform":  {"architecture": "arm64", "os": "linux"},
    }
    new_index = dict(index)
    new_index["manifests"] = [amd64, arm64]
    new_index_bytes = json.dumps(new_index, separators=(",", ":")).encode()

    # Import with Gitea target name
    if not import_ref(tgt, make_oci_tar(tgt, new_index_bytes, amd64_bytes)):
        return
    # Also patch the original source ref so pods still using docker.io name work
    import_ref(src, make_oci_tar(src, new_index_bytes, amd64_bytes))

    # Push to Gitea registry
    print(f"    pushing to registry...")
    r = subprocess.run(
        ["ctr", "-n", "k8s.io", "images", "push",
         "--user", f"{user}:{pwd}", tgt],
        capture_output=True, text=True,
    )
    status = "OK" if r.returncode == 0 else f"PUSH FAILED: {r.stderr.strip()}"
    print(f"    {status}")

for _src, _tgt in TARGETS:
    process(_src, _tgt, USER, PASS)
'''


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _capture_out(cmd, *, default=""):
    r = subprocess.run(cmd, capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else default


def _run(cmd, *, check=True, input=None, capture=False, cwd=None):
    text = not isinstance(input, bytes)
    return subprocess.run(cmd, check=check, text=text, input=input,
                          capture_output=capture, cwd=cwd)


# ---------------------------------------------------------------------------
# Build environment & generic builder
# ---------------------------------------------------------------------------

@dataclass
class BuildEnv:
    """Resolved build environment — production (remote k8s) or local (Lima)."""
    is_prod: bool
    domain: str
    registry: str
    admin_pass: str
    platform: str
    ssh_host: str | None = None


def _get_build_env() -> BuildEnv:
    """Detect prod vs local and resolve registry credentials."""
    from sunbeam import kube as _kube
    is_prod = bool(_kube._ssh_host)

    if is_prod:
        domain = os.environ.get("SUNBEAM_DOMAIN", "sunbeam.pt")
    else:
        ip = get_lima_ip()
        domain = f"{ip}.sslip.io"

    b64 = kube_out("-n", "devtools", "get", "secret",
                   "gitea-admin-credentials", "-o=jsonpath={.data.password}")
    if not b64:
        die("gitea-admin-credentials secret not found -- run seed first.")
    admin_pass = base64.b64decode(b64).decode()

    return BuildEnv(
        is_prod=is_prod,
        domain=domain,
        registry=f"src.{domain}",
        admin_pass=admin_pass,
        platform="linux/amd64" if is_prod else "linux/arm64",
        ssh_host=_kube._ssh_host if is_prod else None,
    )


def _buildctl_build_and_push(
    env: BuildEnv,
    image: str,
    dockerfile: Path,
    context_dir: Path,
    *,
    target: str | None = None,
    build_args: dict[str, str] | None = None,
    no_cache: bool = False,
) -> None:
    """Build and push an image via buildkitd running in k3s.

    Port-forwards to the buildkitd service in the `build` namespace,
    runs `buildctl build`, and pushes the image directly to the Gitea
    registry from inside the cluster. No local Docker daemon needed.
    Works for both production and local Lima k3s.
    """
    from sunbeam import kube as _kube
    from sunbeam.tools import ensure_tool

    buildctl = ensure_tool("buildctl")
    kubectl = ensure_tool("kubectl")

    with socket.socket() as s:
        s.bind(("", 0))
        local_port = s.getsockname()[1]

    ctx_args = [_kube.context_arg()]

    auth_token = base64.b64encode(
        f"{GITEA_ADMIN_USER}:{env.admin_pass}".encode()
    ).decode()
    docker_cfg = {"auths": {env.registry: {"auth": auth_token}}}

    with tempfile.TemporaryDirectory() as tmpdir:
        cfg_path = Path(tmpdir) / "config.json"
        cfg_path.write_text(json.dumps(docker_cfg))

        pf = subprocess.Popen(
            [str(kubectl), *ctx_args,
             "port-forward", "-n", "build", "svc/buildkitd",
             f"{local_port}:1234"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.time() + 15
        while time.time() < deadline:
            try:
                with socket.create_connection(("127.0.0.1", local_port), timeout=1):
                    break
            except OSError:
                time.sleep(0.3)
        else:
            pf.terminate()
            raise RuntimeError(
                f"buildkitd port-forward on :{local_port} did not become ready within 15s"
            )

        try:
            cmd = [
                str(buildctl), "build",
                "--frontend", "dockerfile.v0",
                "--local", f"context={context_dir}",
                "--local", f"dockerfile={dockerfile.parent}",
                "--opt", f"filename={dockerfile.name}",
                "--opt", f"platform={env.platform}",
                "--output", f"type=image,name={image},push=true",
            ]
            if target:
                cmd += ["--opt", f"target={target}"]
            if no_cache:
                cmd += ["--no-cache"]
            if build_args:
                for k, v in build_args.items():
                    cmd += ["--opt", f"build-arg:{k}={v}"]
            run_env = {
                **os.environ,
                "BUILDKIT_HOST": f"tcp://127.0.0.1:{local_port}",
                "DOCKER_CONFIG": tmpdir,
            }
            subprocess.run(cmd, env=run_env, check=True)
        finally:
            pf.terminate()
            pf.wait()


def _build_image(
    env: BuildEnv,
    image: str,
    dockerfile: Path,
    context_dir: Path,
    *,
    target: str | None = None,
    build_args: dict[str, str] | None = None,
    push: bool = False,
    no_cache: bool = False,
    cleanup_paths: list[Path] | None = None,
) -> None:
    """Build a container image via buildkitd and push to the Gitea registry.

    Both production and local builds use the in-cluster buildkitd. The image
    is built for the environment's platform and pushed directly to the registry.
    """
    ok(f"Building image ({env.platform}{f', {target} target' if target else ''})...")

    if not push:
        warn("Builds require --push (buildkitd pushes directly to registry); skipping.")
        return

    try:
        _buildctl_build_and_push(
            env=env,
            image=image,
            dockerfile=dockerfile,
            context_dir=context_dir,
            target=target,
            build_args=build_args,
            no_cache=no_cache,
        )
    finally:
        for p in (cleanup_paths or []):
            if p.exists():
                if p.is_dir():
                    shutil.rmtree(str(p), ignore_errors=True)
                else:
                    p.unlink(missing_ok=True)


def _get_node_addresses() -> list[str]:
    """Return one SSH-reachable IP per node in the cluster.

    Each node may report both IPv4 and IPv6 InternalIPs.  We pick one per
    node name, preferring IPv4 (more likely to have SSH reachable).
    """
    # Get "nodeName ip" pairs
    raw = kube_out(
        "get", "nodes",
        "-o", "jsonpath={range .items[*]}{.metadata.name}{\"\\n\"}"
              "{range .status.addresses[?(@.type==\"InternalIP\")]}{.address}{\" \"}{end}{\"\\n\"}{end}",
    )
    lines = [l.strip() for l in raw.strip().split("\n") if l.strip()]
    seen_nodes: dict[str, str] = {}
    # Lines alternate: node name, then space-separated IPs
    i = 0
    while i < len(lines) - 1:
        node_name = lines[i]
        addrs = lines[i + 1].split()
        i += 2
        if node_name in seen_nodes:
            continue
        # Prefer IPv4 (no colons)
        ipv4 = [a for a in addrs if ":" not in a]
        seen_nodes[node_name] = ipv4[0] if ipv4 else addrs[0]
    return list(seen_nodes.values())


def _ctr_pull_on_nodes(env: BuildEnv, images: list[str]):
    """SSH to each k3s node and pull images into containerd.

    For k3s with imagePullPolicy: IfNotPresent, the image must be present
    in containerd *before* the rollout restart.  buildkitd pushes to the
    Gitea registry; we SSH to each node and ctr-pull so containerd has the
    fresh layers.
    """
    if not images:
        return
    nodes = _get_node_addresses()
    if not nodes:
        warn("Could not detect node addresses; skipping ctr pull.")
        return

    ssh_user = env.ssh_host.split("@")[0] if env.ssh_host and "@" in env.ssh_host else "root"

    for node_ip in nodes:
        for img in images:
            ok(f"Pulling {img} into containerd on {node_ip}...")
            r = subprocess.run(
                ["ssh", "-p", "2222",
                 "-o", "StrictHostKeyChecking=no", f"{ssh_user}@{node_ip}",
                 f"sudo ctr -n k8s.io images pull {img}"],
                capture_output=True, text=True,
            )
            if r.returncode != 0:
                die(f"ctr pull failed on {node_ip}: {r.stderr.strip()}")
            ok(f"Pulled {img} on {node_ip}")


def _deploy_rollout(env: BuildEnv, deployments: list[str], namespace: str,
                    timeout: str = "180s", images: list[str] | None = None):
    """Apply manifests for the target namespace and rolling-restart the given deployments.

    For single-node k3s (env.ssh_host is set), pulls *images* into containerd
    on the node via SSH before restarting, so imagePullPolicy: IfNotPresent
    picks up the new layers.
    """
    from sunbeam.manifests import cmd_apply
    cmd_apply(env="production" if env.is_prod else "local", domain=env.domain,
              namespace=namespace)

    # Pull fresh images into containerd on every node before rollout
    if images:
        _ctr_pull_on_nodes(env, images)

    for dep in deployments:
        ok(f"Rolling {dep}...")
        kube("rollout", "restart", f"deployment/{dep}", "-n", namespace)
    for dep in deployments:
        kube("rollout", "status", f"deployment/{dep}", "-n", namespace,
             f"--timeout={timeout}")
    ok("Redeployed.")


# ---------------------------------------------------------------------------
# Mirroring
# ---------------------------------------------------------------------------

def cmd_mirror(domain: str = "", gitea_admin_pass: str = ""):
    """Patch amd64-only images with an arm64 alias and push to Gitea registry."""
    if not domain:
        ip = get_lima_ip()
        domain = f"{ip}.sslip.io"
    if not gitea_admin_pass:
        b64 = kube_out("-n", "devtools", "get", "secret",
                       "gitea-admin-credentials", "-o=jsonpath={.data.password}")
        if b64:
            gitea_admin_pass = base64.b64decode(b64).decode()

    step("Mirroring amd64-only images to Gitea registry...")

    registry = f"src.{domain}"
    targets = [
        (src, f"{registry}/{org}/{repo}:{tag}")
        for src, org, repo, tag in AMD64_ONLY_IMAGES
    ]

    header = (
        f"TARGETS = {repr(targets)}\n"
        f"USER = {repr(GITEA_ADMIN_USER)}\n"
        f"PASS = {repr(gitea_admin_pass)}\n"
    )
    script = header + _MIRROR_SCRIPT_BODY

    _run(["limactl", "shell", LIMA_VM, "sudo", "python3", "-c", script])

    # Delete any pods stuck in image-pull error states
    ok("Clearing image-pull-error pods...")
    error_reasons = {"ImagePullBackOff", "ErrImagePull", "ErrImageNeverPull"}
    for ns in MANAGED_NS:
        pods_raw = kube_out(
            "-n", ns, "get", "pods",
            "-o=jsonpath={range .items[*]}"
            "{.metadata.name}:{.status.containerStatuses[0].state.waiting.reason}\\n"
            "{end}",
        )
        for line in pods_raw.splitlines():
            if not line:
                continue
            parts = line.split(":", 1)
            if len(parts) == 2 and parts[1] in error_reasons:
                kube("delete", "pod", parts[0], "-n", ns,
                     "--ignore-not-found", check=False)
    ok("Done.")


# ---------------------------------------------------------------------------
# Build dispatch
# ---------------------------------------------------------------------------

def cmd_build(what: str, push: bool = False, deploy: bool = False, no_cache: bool = False):
    """Build an image. Pass push=True to push, deploy=True to also apply + rollout."""
    try:
        _cmd_build(what, push=push, deploy=deploy, no_cache=no_cache)
    except subprocess.CalledProcessError as exc:
        cmd_str = " ".join(str(a) for a in exc.cmd)
        die(f"Build step failed (exit {exc.returncode}): {cmd_str}")


def _cmd_build(what: str, push: bool = False, deploy: bool = False, no_cache: bool = False):
    if what == "proxy":
        _build_proxy(push=push, deploy=deploy)
    elif what == "integration":
        _build_integration(push=push, deploy=deploy)
    elif what == "kratos-admin":
        _build_kratos_admin(push=push, deploy=deploy)
    elif what == "meet":
        _build_meet(push=push, deploy=deploy)
    elif what == "docs-frontend":
        _build_la_suite_frontend(
            app="docs-frontend",
            repo_dir=_get_repo_root() / "docs",
            workspace_rel="src/frontend",
            app_rel="src/frontend/apps/impress",
            dockerfile_rel="src/frontend/Dockerfile",
            image_name="impress-frontend",
            deployment="docs-frontend",
            namespace="lasuite",
            push=push,
            deploy=deploy,
        )
    elif what in ("people", "people-frontend"):
        _build_people(push=push, deploy=deploy)
    elif what in ("messages", "messages-backend", "messages-frontend",
                  "messages-mta-in", "messages-mta-out", "messages-mpa",
                  "messages-socks-proxy"):
        _build_messages(what, push=push, deploy=deploy)
    elif what == "tuwunel":
        _build_tuwunel(push=push, deploy=deploy)
    elif what == "calendars":
        _build_calendars(push=push, deploy=deploy)
    elif what == "projects":
        _build_projects(push=push, deploy=deploy)
    elif what == "sol":
        _build_sol(push=push, deploy=deploy)
    else:
        die(f"Unknown build target: {what}")


# ---------------------------------------------------------------------------
# Per-service build functions
# ---------------------------------------------------------------------------

def _build_proxy(push: bool = False, deploy: bool = False):
    env = _get_build_env()

    proxy_dir = _get_repo_root() / "proxy"
    if not proxy_dir.is_dir():
        die(f"Proxy source not found at {proxy_dir}")

    image = f"{env.registry}/studio/proxy:latest"
    step(f"Building sunbeam-proxy -> {image} ...")

    # Both local and production use the same Dockerfile and build via
    # the in-cluster buildkitd. The buildkitd on each environment
    # compiles natively for its own architecture (arm64 on Lima,
    # amd64 on Scaleway).
    _build_image(env, image, proxy_dir / "Dockerfile", proxy_dir, push=push)

    if deploy:
        _deploy_rollout(env, ["pingora"], "ingress", timeout="120s",
                        images=[image])


def _build_tuwunel(push: bool = False, deploy: bool = False):
    """Build tuwunel Matrix homeserver image from source."""
    env = _get_build_env()

    tuwunel_dir = _get_repo_root() / "tuwunel"
    if not tuwunel_dir.is_dir():
        die(f"Tuwunel source not found at {tuwunel_dir}")

    image = f"{env.registry}/studio/tuwunel:latest"
    step(f"Building tuwunel -> {image} ...")

    # buildkitd runs on the x86_64 server — builds natively, no cross-compilation.
    _build_image(env, image, tuwunel_dir / "Dockerfile", tuwunel_dir, push=push)

    if deploy:
        _deploy_rollout(env, ["tuwunel"], "matrix", timeout="180s",
                        images=[image])


def _build_integration(push: bool = False, deploy: bool = False):
    env = _get_build_env()

    sunbeam_dir = _get_repo_root()
    integration_service_dir = sunbeam_dir / "integration-service"
    dockerfile = integration_service_dir / "Dockerfile"
    dockerignore = integration_service_dir / ".dockerignore"

    if not dockerfile.exists():
        die(f"integration-service Dockerfile not found at {dockerfile}")
    if not (sunbeam_dir / "integration" / "packages" / "widgets").is_dir():
        die(f"integration repo not found at {sunbeam_dir / 'integration'} -- "
            "run: cd sunbeam && git clone https://github.com/suitenumerique/integration.git")

    image = f"{env.registry}/studio/integration:latest"
    step(f"Building integration -> {image} ...")

    # .dockerignore needs to be at context root (sunbeam/)
    root_ignore = sunbeam_dir / ".dockerignore"
    copied_ignore = False
    if not root_ignore.exists() and dockerignore.exists():
        shutil.copy(str(dockerignore), str(root_ignore))
        copied_ignore = True
    try:
        _build_image(env, image, dockerfile, sunbeam_dir, push=push)
    finally:
        if copied_ignore and root_ignore.exists():
            root_ignore.unlink()

    if deploy:
        _deploy_rollout(env, ["integration"], "lasuite", timeout="120s")


def _build_kratos_admin(push: bool = False, deploy: bool = False):
    env = _get_build_env()

    kratos_admin_dir = _get_repo_root() / "kratos-admin"
    if not kratos_admin_dir.is_dir():
        die(f"kratos-admin source not found at {kratos_admin_dir}")

    image = f"{env.registry}/studio/kratos-admin-ui:latest"

    step(f"Building kratos-admin-ui -> {image} ...")

    _build_image(
        env, image,
        kratos_admin_dir / "Dockerfile", kratos_admin_dir,
        push=push,
    )

    if deploy:
        _deploy_rollout(env, ["kratos-admin-ui"], "ory", timeout="120s")


def _build_meet(push: bool = False, deploy: bool = False):
    """Build meet-backend and meet-frontend images from source."""
    env = _get_build_env()

    meet_dir = _get_repo_root() / "meet"
    if not meet_dir.is_dir():
        die(f"meet source not found at {meet_dir}")

    backend_image = f"{env.registry}/studio/meet-backend:latest"
    frontend_image = f"{env.registry}/studio/meet-frontend:latest"

    step(f"Building meet-backend -> {backend_image} ...")
    _build_image(
        env, backend_image,
        meet_dir / "Dockerfile", meet_dir,
        target="backend-production",
        push=push,
    )

    step(f"Building meet-frontend -> {frontend_image} ...")
    frontend_dockerfile = meet_dir / "src" / "frontend" / "Dockerfile"
    if not frontend_dockerfile.exists():
        die(f"meet frontend Dockerfile not found at {frontend_dockerfile}")
    _build_image(
        env, frontend_image,
        frontend_dockerfile, meet_dir,
        target="frontend-production",
        build_args={"VITE_API_BASE_URL": ""},
        push=push,
    )

    if deploy:
        _deploy_rollout(
            env,
            ["meet-backend", "meet-celery-worker", "meet-frontend"],
            "lasuite",
        )


def _build_people(push: bool = False, deploy: bool = False):
    """Build people-frontend from source."""
    env = _get_build_env()

    people_dir = _get_repo_root() / "people"
    if not people_dir.is_dir():
        die(f"people source not found at {people_dir}")

    if not shutil.which("yarn"):
        die("yarn not found on PATH -- install Node.js + yarn first (nvm use 22).")

    workspace_dir = people_dir / "src" / "frontend"
    app_dir = people_dir / "src" / "frontend" / "apps" / "desk"
    dockerfile = people_dir / "src" / "frontend" / "Dockerfile"
    if not dockerfile.exists():
        die(f"Dockerfile not found at {dockerfile}")

    image = f"{env.registry}/studio/people-frontend:latest"
    step(f"Building people-frontend -> {image} ...")

    ok("Updating yarn.lock (yarn install in workspace)...")
    _run(["yarn", "install", "--ignore-engines"], cwd=str(workspace_dir))

    ok("Regenerating cunningham design tokens (cunningham -g css,ts)...")
    cunningham_bin = workspace_dir / "node_modules" / ".bin" / "cunningham"
    _run([str(cunningham_bin), "-g", "css,ts", "-o", "src/cunningham", "--utility-classes"],
         cwd=str(app_dir))

    _build_image(
        env, image,
        dockerfile, people_dir,
        target="frontend-production",
        build_args={"DOCKER_USER": "101"},
        push=push,
    )

    if deploy:
        _deploy_rollout(env, ["people-frontend"], "lasuite")


def _build_messages(what: str, push: bool = False, deploy: bool = False):
    """Build one or all messages images from source."""
    env = _get_build_env()

    messages_dir = _get_repo_root() / "messages"
    if not messages_dir.is_dir():
        die(f"messages source not found at {messages_dir}")

    all_components = [
        ("messages-backend",     "messages-backend",     "src/backend/Dockerfile",       "runtime-distroless-prod"),
        ("messages-frontend",    "messages-frontend",    "src/frontend/Dockerfile",       "runtime-prod"),
        ("messages-mta-in",      "messages-mta-in",      "src/mta-in/Dockerfile",         None),
        ("messages-mta-out",     "messages-mta-out",     "src/mta-out/Dockerfile",        None),
        ("messages-mpa",         "messages-mpa",         "src/mpa/rspamd/Dockerfile",     None),
        ("messages-socks-proxy", "messages-socks-proxy", "src/socks-proxy/Dockerfile",    None),
    ]
    components = all_components if what == "messages" else [
        c for c in all_components if c[0] == what
    ]

    built_images = []
    for component, image_name, dockerfile_rel, target in components:
        dockerfile = messages_dir / dockerfile_rel
        if not dockerfile.exists():
            warn(f"Dockerfile not found at {dockerfile} -- skipping {component}")
            continue

        image = f"{env.registry}/studio/{image_name}:latest"
        context_dir = dockerfile.parent
        step(f"Building {component} -> {image} ...")

        # Patch ghcr.io/astral-sh/uv COPY for messages-backend on local builds
        cleanup_paths: list[Path] = []
        actual_dockerfile = dockerfile
        if not env.is_prod and image_name == "messages-backend":
            actual_dockerfile, cleanup_paths = _patch_dockerfile_uv(
                dockerfile, context_dir, env.platform
            )

        _build_image(
            env, image,
            actual_dockerfile, context_dir,
            target=target,
            push=push,
            cleanup_paths=cleanup_paths,
        )
        built_images.append(image)

    if deploy and built_images:
        _deploy_rollout(
            env,
            ["messages-backend", "messages-worker", "messages-frontend",
             "messages-mta-in", "messages-mta-out", "messages-mpa",
             "messages-socks-proxy"],
            "lasuite",
        )


def _build_la_suite_frontend(
    app: str,
    repo_dir: Path,
    workspace_rel: str,
    app_rel: str,
    dockerfile_rel: str,
    image_name: str,
    deployment: str,
    namespace: str,
    push: bool = False,
    deploy: bool = False,
):
    """Build a La Suite frontend image from source and push to the Gitea registry."""
    env = _get_build_env()

    if not shutil.which("yarn"):
        die("yarn not found on PATH — install Node.js + yarn first (nvm use 22).")

    workspace_dir = repo_dir / workspace_rel
    app_dir = repo_dir / app_rel
    dockerfile = repo_dir / dockerfile_rel

    if not repo_dir.is_dir():
        die(f"{app} source not found at {repo_dir}")
    if not dockerfile.exists():
        die(f"Dockerfile not found at {dockerfile}")

    image = f"{env.registry}/studio/{image_name}:latest"
    step(f"Building {app} -> {image} ...")

    ok("Updating yarn.lock (yarn install in workspace)...")
    _run(["yarn", "install", "--ignore-engines"], cwd=str(workspace_dir))

    ok("Regenerating cunningham design tokens (yarn build-theme)...")
    _run(["yarn", "build-theme"], cwd=str(app_dir))

    _build_image(
        env, image,
        dockerfile, repo_dir,
        target="frontend-production",
        build_args={"DOCKER_USER": "101"},
        push=push,
    )

    if deploy:
        _deploy_rollout(env, [deployment], namespace)


def _patch_dockerfile_uv(
    dockerfile_path: Path,
    messages_dir: Path,
    platform: str,
) -> tuple[Path, list[Path]]:
    """Download uv from GitHub releases and return a patched Dockerfile path.

    The docker-container buildkit driver cannot access the host Docker daemon's
    local image cache, so --build-context docker-image:// silently falls through
    to docker.io. oci-layout:// is the only local-context type that works, but
    it requires producing an OCI tar and extracting it.

    The simplest reliable approach: stage the downloaded binaries inside the
    build context directory and patch the Dockerfile to use a plain COPY instead
    of COPY --from=ghcr.io/... The patched Dockerfile is written next to the
    original; both it and the staging dir are cleaned up by the caller.

    Returns (patched_dockerfile_path, [paths_to_cleanup]).
    """
    import re as _re
    import tarfile as _tf
    import urllib.request as _url

    content = dockerfile_path.read_text()

    copy_match = _re.search(
        r'(COPY\s+--from=ghcr\.io/astral-sh/uv@sha256:[a-f0-9]+\s+/uv\s+/uvx\s+/bin/)',
        content,
    )
    if not copy_match:
        return (dockerfile_path, [])
    original_copy = copy_match.group(1)

    version_match = _re.search(r'oci://ghcr\.io/astral-sh/uv:(\S+)', content)
    if not version_match:
        warn("Could not find uv version comment in Dockerfile; ghcr.io pull may fail.")
        return (dockerfile_path, [])
    version = version_match.group(1)

    arch = "x86_64" if "amd64" in platform else "aarch64"
    url = (
        f"https://github.com/astral-sh/uv/releases/download/{version}/"
        f"uv-{arch}-unknown-linux-gnu.tar.gz"
    )

    stage_dir = messages_dir / "_sunbeam_uv_stage"
    patched_df = dockerfile_path.parent / "Dockerfile._sunbeam_patched"
    cleanup = [stage_dir, patched_df]

    ok(f"Downloading uv {version} ({arch}) from GitHub releases to bypass ghcr.io...")
    try:
        stage_dir.mkdir(exist_ok=True)
        tarball = stage_dir / "uv.tar.gz"
        _url.urlretrieve(url, str(tarball))

        with _tf.open(str(tarball), "r:gz") as tf:
            for member in tf.getmembers():
                name = os.path.basename(member.name)
                if name in ("uv", "uvx") and member.isfile():
                    member.name = name
                    tf.extract(member, str(stage_dir))
        tarball.unlink()

        uv_path = stage_dir / "uv"
        uvx_path = stage_dir / "uvx"
        if not uv_path.exists():
            warn("uv binary not found in release tarball; build may fail.")
            return (dockerfile_path, cleanup)
        uv_path.chmod(0o755)
        if uvx_path.exists():
            uvx_path.chmod(0o755)

        patched = content.replace(
            original_copy,
            "COPY _sunbeam_uv_stage/uv _sunbeam_uv_stage/uvx /bin/",
        )
        patched_df.write_text(patched)
        ok(f"  uv {version} staged; using patched Dockerfile.")
        return (patched_df, cleanup)

    except Exception as exc:
        warn(f"Failed to stage uv binaries: {exc}")
        return (dockerfile_path, cleanup)


def _build_projects(push: bool = False, deploy: bool = False):
    """Build projects (Planka Kanban) image from source."""
    env = _get_build_env()

    projects_dir = _get_repo_root() / "projects"
    if not projects_dir.is_dir():
        die(f"projects source not found at {projects_dir}")

    image = f"{env.registry}/studio/projects:latest"
    step(f"Building projects -> {image} ...")

    _build_image(env, image, projects_dir / "Dockerfile", projects_dir, push=push)

    if deploy:
        _deploy_rollout(env, ["projects"], "lasuite", timeout="180s",
                        images=[image])


def _build_sol(push: bool = False, deploy: bool = False):
    """Build Sol virtual librarian image from source.

    # TODO: first deploy requires registration enabled on tuwunel to create
    # the @sol:sunbeam.pt bot account. Flow:
    #   1. Set allow_registration = true in tuwunel-config.yaml
    #   2. Apply + restart tuwunel
    #   3. Register bot via POST /_matrix/client/v3/register with registration token
    #   4. Store access_token + device_id in OpenBao at secret/sol
    #   5. Set allow_registration = false, re-apply
    #   6. Then build + deploy sol
    # This should be automated as `sunbeam user create-bot <name>`.
    """
    env = _get_build_env()

    sol_dir = _get_repo_root() / "sol"
    if not sol_dir.is_dir():
        die(f"Sol source not found at {sol_dir}")

    image = f"{env.registry}/studio/sol:latest"
    step(f"Building sol -> {image} ...")

    _build_image(env, image, sol_dir / "Dockerfile", sol_dir, push=push)

    if deploy:
        _deploy_rollout(env, ["sol"], "matrix", timeout="120s")


def _build_calendars(push: bool = False, deploy: bool = False):
    env = _get_build_env()
    cal_dir = _get_repo_root() / "calendars"
    if not cal_dir.is_dir():
        die(f"calendars source not found at {cal_dir}")

    backend_dir = cal_dir / "src" / "backend"
    backend_image = f"{env.registry}/studio/calendars-backend:latest"
    step(f"Building calendars-backend -> {backend_image} ...")

    # Stage translations.json into the build context so the production image
    # has it at /data/translations.json (Docker Compose mounts it; we bake it in).
    translations_src = (cal_dir / "src" / "frontend" / "apps" / "calendars"
                        / "src" / "features" / "i18n" / "translations.json")
    translations_dst = backend_dir / "_translations.json"
    cleanup: list[Path] = []
    dockerfile = backend_dir / "Dockerfile"
    if translations_src.exists():
        shutil.copy(str(translations_src), str(translations_dst))
        cleanup.append(translations_dst)
        # Patch Dockerfile to COPY translations into production image
        patched = dockerfile.read_text() + (
            "\n# Sunbeam: bake translations.json for default calendar names\n"
            "COPY _translations.json /data/translations.json\n"
        )
        patched_df = backend_dir / "Dockerfile._sunbeam_patched"
        patched_df.write_text(patched)
        cleanup.append(patched_df)
        dockerfile = patched_df

    _build_image(env, backend_image,
                 dockerfile,
                 backend_dir,
                 target="backend-production",
                 push=push,
                 cleanup_paths=cleanup)

    caldav_image = f"{env.registry}/studio/calendars-caldav:latest"
    step(f"Building calendars-caldav -> {caldav_image} ...")
    _build_image(env, caldav_image,
                 cal_dir / "src" / "caldav" / "Dockerfile",
                 cal_dir / "src" / "caldav",
                 push=push)

    frontend_image = f"{env.registry}/studio/calendars-frontend:latest"
    step(f"Building calendars-frontend -> {frontend_image} ...")
    integration_base = f"https://integration.{env.domain}"
    _build_image(env, frontend_image,
                 cal_dir / "src" / "frontend" / "Dockerfile",
                 cal_dir / "src" / "frontend",
                 target="frontend-production",
                 build_args={
                     "VISIO_BASE_URL": f"https://meet.{env.domain}",
                     "GAUFRE_WIDGET_PATH": f"{integration_base}/api/v2/lagaufre.js",
                     "GAUFRE_API_URL": f"{integration_base}/api/v2/services.json",
                     "THEME_CSS_URL": f"{integration_base}/api/v2/theme.css",
                 },
                 push=push)

    if deploy:
        _deploy_rollout(env,
                        ["calendars-backend", "calendars-worker",
                         "calendars-caldav", "calendars-frontend"],
                        "lasuite", timeout="180s",
                        images=[backend_image, caldav_image, frontend_image])
