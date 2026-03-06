"""Image mirroring — patch amd64-only images + push to Gitea registry."""
import base64
import os
import shutil
import subprocess
import sys
from pathlib import Path

from sunbeam.kube import kube, kube_out, get_lima_ip
from sunbeam.output import step, ok, warn, die

LIMA_VM = "sunbeam"
LIMA_DOCKER_VM = "docker"
GITEA_ADMIN_USER = "gitea_admin"
MANAGED_NS = ["data", "devtools", "ingress", "lasuite", "media", "ory", "storage",
              "vault-secrets-operator"]

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


def _capture_out(cmd, *, default=""):
    r = subprocess.run(cmd, capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else default


def _run(cmd, *, check=True, input=None, capture=False, cwd=None):
    text = not isinstance(input, bytes)
    return subprocess.run(cmd, check=check, text=text, input=input,
                          capture_output=capture, cwd=cwd)


def _seed_and_push(image: str, admin_pass: str):
    """Pre-seed a locally-built Docker image into k3s containerd, then push
    to the Gitea registry via 'ctr images push' inside the Lima VM.

    This avoids 'docker push' entirely — the Lima k3s VM's containerd already
    trusts the mkcert CA (used for image pulls from Gitea), so ctr push works
    where docker push would hit a TLS cert verification error on the Mac.
    """
    ok("Pre-seeding image into k3s containerd...")
    save = subprocess.Popen(["docker", "save", image], stdout=subprocess.PIPE)
    ctr = subprocess.run(
        ["limactl", "shell", LIMA_VM, "--",
         "sudo", "ctr", "-n", "k8s.io", "images", "import", "-"],
        stdin=save.stdout,
        capture_output=True,
    )
    save.stdout.close()
    save.wait()
    if ctr.returncode != 0:
        warn(f"containerd import failed:\n{ctr.stderr.decode().strip()}")
    else:
        ok("Image pre-seeded.")

    ok("Pushing to Gitea registry (via ctr in Lima VM)...")
    push = subprocess.run(
        ["limactl", "shell", LIMA_VM, "--",
         "sudo", "ctr", "-n", "k8s.io", "images", "push",
         "--user", f"{GITEA_ADMIN_USER}:{admin_pass}", image],
        capture_output=True, text=True,
    )
    if push.returncode != 0:
        warn(f"ctr push failed (image is pre-seeded; cluster will work without push):\n"
             f"{push.stderr.strip()}")
    else:
        ok(f"Pushed {image}")


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


def _trust_registry_in_docker_vm(registry: str):
    """Install the mkcert CA into the Lima Docker VM's per-registry cert dir.

    The Lima Docker VM runs rootless Docker, which reads custom CA certs from
    ~/.config/docker/certs.d/<registry>/ca.crt (not /etc/docker/certs.d/).
    No daemon restart required -- Docker reads the file per-connection.
    """
    caroot = _capture_out(["mkcert", "-CAROOT"])
    if not caroot:
        warn("mkcert -CAROOT returned nothing -- skipping Docker CA install.")
        return
    ca_pem = Path(caroot) / "rootCA.pem"
    if not ca_pem.exists():
        warn(f"mkcert CA not found at {ca_pem} -- skipping Docker CA install.")
        return

    _run(["limactl", "copy", str(ca_pem), f"{LIMA_DOCKER_VM}:/tmp/registry-ca.pem"])
    _run(["limactl", "shell", LIMA_DOCKER_VM, "--", "sh", "-c",
          f"mkdir -p ~/.config/docker/certs.d/{registry} && "
          f"cp /tmp/registry-ca.pem ~/.config/docker/certs.d/{registry}/ca.crt"])
    ok(f"mkcert CA installed in Docker VM for {registry}.")


def cmd_build(what: str, push: bool = False, deploy: bool = False):
    """Build an image. Pass push=True to push, deploy=True to also apply + rollout."""
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
            repo_dir=Path(__file__).resolve().parents[2] / "docs",
            workspace_rel="src/frontend",
            app_rel="src/frontend/apps/impress",
            dockerfile_rel="src/frontend/Dockerfile",
            image_name="impress-frontend",
            deployment="docs-frontend",
            namespace="lasuite",
            push=push,
            deploy=deploy,
        )
    elif what == "people-frontend":
        _build_la_suite_frontend(
            app="people-frontend",
            repo_dir=Path(__file__).resolve().parents[2] / "people",
            workspace_rel="src/frontend",
            app_rel="src/frontend/apps/desk",
            dockerfile_rel="src/frontend/Dockerfile",
            image_name="people-frontend",
            deployment="people-frontend",
            namespace="lasuite",
            push=push,
            deploy=deploy,
        )
    else:
        die(f"Unknown build target: {what}")



def _seed_image_production(image: str, ssh_host: str, admin_pass: str):
    """Build linux/amd64 image, pipe into production containerd via SSH, then push to Gitea."""
    ok("Importing image into production containerd via SSH pipe...")
    save = subprocess.Popen(["docker", "save", image], stdout=subprocess.PIPE)
    import_cmd = f"sudo ctr -n k8s.io images import -"
    ctr = subprocess.run(
        ["ssh", "-p", "2222", "-o", "StrictHostKeyChecking=no", ssh_host, import_cmd],
        stdin=save.stdout,
        capture_output=True,
    )
    save.stdout.close()
    save.wait()
    if ctr.returncode != 0:
        warn(f"containerd import failed:\n{ctr.stderr.decode().strip()}")
        return False
    ok("Image imported into production containerd.")

    ok("Pushing image to Gitea registry (via ctr on production server)...")
    push = subprocess.run(
        ["ssh", "-p", "2222", "-o", "StrictHostKeyChecking=no", ssh_host,
         f"sudo ctr -n k8s.io images push --user {GITEA_ADMIN_USER}:{admin_pass} {image}"],
        capture_output=True, text=True,
    )
    if push.returncode != 0:
        warn(f"ctr push failed (image is pre-seeded; cluster will start):\n{push.stderr.strip()}")
    else:
        ok(f"Pushed {image} to Gitea registry.")
    return True


def _build_proxy(push: bool = False, deploy: bool = False):
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

    if not shutil.which("docker"):
        die("docker not found -- is the Lima docker VM running?")

    # Proxy source lives adjacent to the infrastructure repo
    proxy_dir = Path(__file__).resolve().parents[2] / "proxy"
    if not proxy_dir.is_dir():
        die(f"Proxy source not found at {proxy_dir}")

    registry = f"src.{domain}"
    image = f"{registry}/studio/proxy:latest"

    step(f"Building sunbeam-proxy -> {image} ...")

    if is_prod:
        # Production (x86_64 server): cross-compile on the Mac arm64 host using
        # x86_64-linux-musl-gcc (brew install filosottile/musl-cross/musl-cross),
        # then package the pre-built static binary into a minimal Docker image.
        # This avoids QEMU x86_64 emulation which crashes rustc (SIGSEGV).
        musl_gcc = shutil.which("x86_64-linux-musl-gcc")
        if not musl_gcc:
            die(
                "x86_64-linux-musl-gcc not found.\n"
                "Install: brew install filosottile/musl-cross/musl-cross"
            )
        ok("Cross-compiling sunbeam-proxy for x86_64-musl (native, no QEMU)...")
        import os as _os
        env = dict(_os.environ)
        env["CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"] = musl_gcc
        env["CC_x86_64_unknown_linux_musl"] = musl_gcc
        env["RUSTFLAGS"] = "-C target-feature=+crt-static"
        r = subprocess.run(
            ["cargo", "build", "--release", "--target", "x86_64-unknown-linux-musl"],
            cwd=str(proxy_dir),
            env=env,
        )
        if r.returncode != 0:
            die("cargo build failed.")
        binary = proxy_dir / "target" / "x86_64-unknown-linux-musl" / "release" / "sunbeam-proxy"

        # Download tini static binary for amd64 if not cached
        import tempfile, urllib.request
        tmpdir = Path(tempfile.mkdtemp(prefix="proxy-pkg-"))
        tini_path = tmpdir / "tini"
        ok("Downloading tini-static-amd64...")
        urllib.request.urlretrieve(
            "https://github.com/krallin/tini/releases/download/v0.19.0/tini-static-amd64",
            str(tini_path),
        )
        tini_path.chmod(0o755)
        shutil.copy(str(binary), str(tmpdir / "sunbeam-proxy"))
        (tmpdir / "Dockerfile").write_text(
            "FROM cgr.dev/chainguard/static:latest\n"
            "COPY tini /tini\n"
            "COPY sunbeam-proxy /usr/local/bin/sunbeam-proxy\n"
            "EXPOSE 80 443\n"
            'ENTRYPOINT ["/tini", "--", "/usr/local/bin/sunbeam-proxy"]\n'
        )
        ok("Packaging into Docker image (linux/amd64, pre-built binary)...")
        _run(["docker", "buildx", "build",
              "--platform", "linux/amd64",
              "--provenance=false",
              "--load",
              "-t", image,
              str(tmpdir)])
        shutil.rmtree(str(tmpdir), ignore_errors=True)
        if push:
            _seed_image_production(image, _kube._ssh_host, admin_pass)
    else:
        # Local Lima dev: build linux/arm64 natively.
        _trust_registry_in_docker_vm(registry)

        ok("Logging in to Gitea registry...")
        r = subprocess.run(
            ["limactl", "shell", LIMA_DOCKER_VM, "--",
             "docker", "login", registry,
             "--username", GITEA_ADMIN_USER, "--password-stdin"],
            input=admin_pass, text=True, capture_output=True,
        )
        if r.returncode != 0:
            die(f"docker login failed:\n{r.stderr.strip()}")

        ok("Building image (linux/arm64)...")
        _run(["docker", "buildx", "build",
              "--platform", "linux/arm64",
              "--provenance=false",
              "--load",
              "-t", image,
              str(proxy_dir)])

        if push:
            ok("Pushing image...")
            _run(["docker", "push", image])
            _seed_and_push(image, admin_pass)

    if deploy:
        from sunbeam.manifests import cmd_apply
        cmd_apply(env="production" if is_prod else "local", domain=domain)
        ok("Rolling pingora deployment...")
        kube("rollout", "restart", "deployment/pingora", "-n", "ingress")
        kube("rollout", "status", "deployment/pingora", "-n", "ingress",
             "--timeout=120s")
        ok("Pingora redeployed.")


def _build_integration(push: bool = False, deploy: bool = False):
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

    if not shutil.which("docker"):
        die("docker not found -- is the Lima docker VM running?")

    # Build context is the sunbeam/ root so Dockerfile can reach both
    # integration/packages/ (upstream widget + logos) and integration-service/.
    sunbeam_dir = Path(__file__).resolve().parents[2]
    integration_service_dir = sunbeam_dir / "integration-service"
    dockerfile = integration_service_dir / "Dockerfile"
    dockerignore = integration_service_dir / ".dockerignore"

    if not dockerfile.exists():
        die(f"integration-service Dockerfile not found at {dockerfile}")
    if not (sunbeam_dir / "integration" / "packages" / "widgets").is_dir():
        die(f"integration repo not found at {sunbeam_dir / 'integration'} -- "
            "run: cd sunbeam && git clone https://github.com/suitenumerique/integration.git")

    registry = f"src.{domain}"
    image = f"{registry}/studio/integration:latest"

    step(f"Building integration -> {image} ...")

    platform = "linux/amd64" if is_prod else "linux/arm64"

    # --file points to integration-service/Dockerfile; context is sunbeam/ root.
    # Copy .dockerignore to context root temporarily if needed.
    root_ignore = sunbeam_dir / ".dockerignore"
    copied_ignore = False
    if not root_ignore.exists() and dockerignore.exists():
        shutil.copy(str(dockerignore), str(root_ignore))
        copied_ignore = True
    try:
        ok(f"Building image ({platform})...")
        _run(["docker", "buildx", "build",
              "--platform", platform,
              "--provenance=false",
              "--load",
              "-f", str(dockerfile),
              "-t", image,
              str(sunbeam_dir)])
    finally:
        if copied_ignore and root_ignore.exists():
            root_ignore.unlink()

    if push:
        if is_prod:
            _seed_image_production(image, _kube._ssh_host, admin_pass)
        else:
            _trust_registry_in_docker_vm(registry)
            ok("Logging in to Gitea registry...")
            r = subprocess.run(
                ["limactl", "shell", LIMA_DOCKER_VM, "--",
                 "docker", "login", registry,
                 "--username", GITEA_ADMIN_USER, "--password-stdin"],
                input=admin_pass, text=True, capture_output=True,
            )
            if r.returncode != 0:
                die(f"docker login failed:\n{r.stderr.strip()}")
            _seed_and_push(image, admin_pass)

    if deploy:
        from sunbeam.manifests import cmd_apply
        cmd_apply(env="production" if is_prod else "local", domain=domain)
        ok("Rolling integration deployment...")
        kube("rollout", "restart", "deployment/integration", "-n", "lasuite")
        kube("rollout", "status", "deployment/integration", "-n", "lasuite",
             "--timeout=120s")
        ok("Integration redeployed.")


def _build_kratos_admin(push: bool = False, deploy: bool = False):
    from sunbeam import kube as _kube

    is_prod = bool(_kube._ssh_host)

    b64 = kube_out("-n", "devtools", "get", "secret",
                   "gitea-admin-credentials", "-o=jsonpath={.data.password}")
    if not b64:
        die("gitea-admin-credentials secret not found -- run seed first.")
    admin_pass = base64.b64decode(b64).decode()

    # kratos-admin source
    kratos_admin_dir = Path(__file__).resolve().parents[2] / "kratos-admin"
    if not kratos_admin_dir.is_dir():
        die(f"kratos-admin source not found at {kratos_admin_dir}")

    if is_prod:
        domain = os.environ.get("SUNBEAM_DOMAIN", "sunbeam.pt")
        registry = f"src.{domain}"
        image = f"{registry}/studio/kratos-admin-ui:latest"
        ssh_host = _kube._ssh_host

        step(f"Building kratos-admin-ui (linux/amd64, native cross-compile) -> {image} ...")

        if not shutil.which("deno"):
            die("deno not found — install Deno: https://deno.land/")
        if not shutil.which("npm"):
            die("npm not found — install Node.js")

        ok("Building UI assets (npm run build)...")
        _run(["npm", "run", "build"], cwd=str(kratos_admin_dir / "ui"))

        ok("Cross-compiling Deno binary for x86_64-linux-gnu...")
        _run([
            "deno", "compile",
            "--target", "x86_64-unknown-linux-gnu",
            "--allow-net", "--allow-read", "--allow-env",
            "--include", "ui/dist",
            "-o", "kratos-admin-x86_64",
            "main.ts",
        ], cwd=str(kratos_admin_dir))

        bin_path = kratos_admin_dir / "kratos-admin-x86_64"
        if not bin_path.exists():
            die("Deno cross-compilation produced no binary")

        # Build minimal Docker image
        pkg_dir = Path("/tmp/kratos-admin-pkg")
        pkg_dir.mkdir(exist_ok=True)
        import shutil as _sh
        _sh.copy2(str(bin_path), str(pkg_dir / "kratos-admin"))
        # Copy ui/dist for serveStatic (binary has it embedded but keep external copy for fallback)
        (pkg_dir / "dockerfile").write_text(
            "FROM gcr.io/distroless/cc-debian12:nonroot\n"
            "WORKDIR /app\n"
            "COPY kratos-admin ./\n"
            "EXPOSE 3000\n"
            'ENTRYPOINT ["/app/kratos-admin"]\n'
        )

        ok("Building Docker image...")
        _run([
            "docker", "buildx", "build",
            "--platform", "linux/amd64",
            "--provenance=false",
            "--load",
            "-f", str(pkg_dir / "dockerfile"),
            "-t", image,
            str(pkg_dir),
        ])

        if push:
            _seed_image_production(image, ssh_host, admin_pass)

        if deploy:
            from sunbeam.manifests import cmd_apply
            cmd_apply(env="production", domain=domain)

    else:
        ip = get_lima_ip()
        domain = f"{ip}.sslip.io"
        registry = f"src.{domain}"
        image = f"{registry}/studio/kratos-admin-ui:latest"

        if not shutil.which("docker"):
            die("docker not found -- is the Lima docker VM running?")

        step(f"Building kratos-admin-ui -> {image} ...")

        _trust_registry_in_docker_vm(registry)

        ok("Logging in to Gitea registry...")
        r = subprocess.run(
            ["limactl", "shell", LIMA_DOCKER_VM, "--",
             "docker", "login", registry,
             "--username", GITEA_ADMIN_USER, "--password-stdin"],
            input=admin_pass, text=True, capture_output=True,
        )
        if r.returncode != 0:
            die(f"docker login failed:\n{r.stderr.strip()}")

        ok("Building image (linux/arm64)...")
        _run(["docker", "buildx", "build",
              "--platform", "linux/arm64",
              "--provenance=false",
              "--load",
              "-t", image,
              str(kratos_admin_dir)])

        if push:
            _seed_and_push(image, admin_pass)

        if deploy:
            from sunbeam.manifests import cmd_apply
            cmd_apply()

    if deploy:
        ok("Rolling kratos-admin-ui deployment...")
        kube("rollout", "restart", "deployment/kratos-admin-ui", "-n", "ory")
        kube("rollout", "status", "deployment/kratos-admin-ui", "-n", "ory",
             "--timeout=120s")
        ok("kratos-admin-ui redeployed.")


def _build_meet(push: bool = False, deploy: bool = False):
    """Build meet-backend and meet-frontend images from source."""
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

    if not shutil.which("docker"):
        die("docker not found -- is the Lima docker VM running?")

    meet_dir = Path(__file__).resolve().parents[2] / "meet"
    if not meet_dir.is_dir():
        die(f"meet source not found at {meet_dir}")

    registry = f"src.{domain}"
    backend_image  = f"{registry}/studio/meet-backend:latest"
    frontend_image = f"{registry}/studio/meet-frontend:latest"
    platform = "linux/amd64" if is_prod else "linux/arm64"

    if not is_prod:
        _trust_registry_in_docker_vm(registry)
        ok("Logging in to Gitea registry...")
        r = subprocess.run(
            ["limactl", "shell", LIMA_DOCKER_VM, "--",
             "docker", "login", registry,
             "--username", GITEA_ADMIN_USER, "--password-stdin"],
            input=admin_pass, text=True, capture_output=True,
        )
        if r.returncode != 0:
            die(f"docker login failed:\n{r.stderr.strip()}")

    step(f"Building meet-backend -> {backend_image} ...")
    ok(f"Building image ({platform}, backend-production target)...")
    _run(["docker", "buildx", "build",
          "--platform", platform,
          "--provenance=false",
          "--target", "backend-production",
          "--load",
          "-t", backend_image,
          str(meet_dir)])

    if push:
        if is_prod:
            _seed_image_production(backend_image, _kube._ssh_host, admin_pass)
        else:
            _seed_and_push(backend_image, admin_pass)

    step(f"Building meet-frontend -> {frontend_image} ...")
    frontend_dockerfile = meet_dir / "src" / "frontend" / "Dockerfile"
    if not frontend_dockerfile.exists():
        die(f"meet frontend Dockerfile not found at {frontend_dockerfile}")

    ok(f"Building image ({platform}, frontend-production target)...")
    _run(["docker", "buildx", "build",
          "--platform", platform,
          "--provenance=false",
          "--target", "frontend-production",
          "--build-arg", "VITE_API_BASE_URL=",
          "--load",
          "-f", str(frontend_dockerfile),
          "-t", frontend_image,
          str(meet_dir)])

    if push:
        if is_prod:
            _seed_image_production(frontend_image, _kube._ssh_host, admin_pass)
        else:
            _seed_and_push(frontend_image, admin_pass)

    if deploy:
        from sunbeam.manifests import cmd_apply
        cmd_apply(env="production" if is_prod else "local", domain=domain)
        for deployment in ("meet-backend", "meet-celery-worker", "meet-frontend"):
            ok(f"Rolling {deployment} deployment...")
            kube("rollout", "restart", f"deployment/{deployment}", "-n", "lasuite")
        for deployment in ("meet-backend", "meet-celery-worker", "meet-frontend"):
            kube("rollout", "status", f"deployment/{deployment}", "-n", "lasuite",
                 "--timeout=180s")
        ok("Meet redeployed.")


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
    """Build a La Suite frontend image from source and push to the Gitea registry.

    Steps:
    1. yarn install in the workspace root — updates yarn.lock for new packages.
    2. yarn build-theme in the app dir — regenerates cunningham token CSS/TS.
    3. docker buildx build --target frontend-production → push.
    4. Pre-seed into k3s containerd.
    5. sunbeam apply + rollout restart.
    """
    if not shutil.which("yarn"):
        die("yarn not found on PATH — install Node.js + yarn first (nvm use 22).")
    if not shutil.which("docker"):
        die("docker not found — is the Lima docker VM running?")

    ip = get_lima_ip()
    domain = f"{ip}.sslip.io"

    b64 = kube_out("-n", "devtools", "get", "secret",
                   "gitea-admin-credentials", "-o=jsonpath={.data.password}")
    if not b64:
        die("gitea-admin-credentials secret not found — run seed first.")
    admin_pass = base64.b64decode(b64).decode()

    workspace_dir = repo_dir / workspace_rel
    app_dir = repo_dir / app_rel
    dockerfile = repo_dir / dockerfile_rel

    if not repo_dir.is_dir():
        die(f"{app} source not found at {repo_dir}")
    if not dockerfile.exists():
        die(f"Dockerfile not found at {dockerfile}")

    registry = f"src.{domain}"
    image = f"{registry}/studio/{image_name}:latest"

    step(f"Building {app} -> {image} ...")

    ok("Updating yarn.lock (yarn install in workspace)...")
    _run(["yarn", "install"], cwd=str(workspace_dir))

    ok("Regenerating cunningham design tokens (yarn build-theme)...")
    _run(["yarn", "build-theme"], cwd=str(app_dir))

    if push:
        _trust_registry_in_docker_vm(registry)
        ok("Logging in to Gitea registry...")
        r = subprocess.run(
            ["limactl", "shell", LIMA_DOCKER_VM, "--",
             "docker", "login", registry,
             "--username", GITEA_ADMIN_USER, "--password-stdin"],
            input=admin_pass, text=True, capture_output=True,
        )
        if r.returncode != 0:
            die(f"docker login failed:\n{r.stderr.strip()}")

    ok("Building image (linux/arm64, frontend-production target)...")
    _run(["docker", "buildx", "build",
          "--platform", "linux/arm64",
          "--provenance=false",
          "--target", "frontend-production",
          "--load",
          "-f", str(dockerfile),
          "-t", image,
          str(repo_dir)])

    if push:
        _seed_and_push(image, admin_pass)

    if deploy:
        from sunbeam.manifests import cmd_apply
        cmd_apply()
        ok(f"Rolling {deployment} deployment...")
        kube("rollout", "restart", f"deployment/{deployment}", "-n", namespace)
        kube("rollout", "status", f"deployment/{deployment}", "-n", namespace,
             "--timeout=180s")
        ok(f"{deployment} redeployed.")
