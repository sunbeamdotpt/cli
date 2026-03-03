"""Image mirroring — patch amd64-only images + push to Gitea registry."""
import base64
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


def cmd_build(what: str):
    """Build and push an image. Supports 'proxy' and 'kratos-admin'."""
    if what == "proxy":
        _build_proxy()
    elif what == "kratos-admin":
        _build_kratos_admin()
    else:
        die(f"Unknown build target: {what}")


def _build_proxy():
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
    image = f"{registry}/studio/sunbeam-proxy:latest"

    step(f"Building sunbeam-proxy -> {image} ...")

    # Ensure the Lima Docker VM trusts our mkcert CA for this registry.
    _trust_registry_in_docker_vm(registry)

    # Authenticate Docker with Gitea before the build so --push succeeds.
    ok("Logging in to Gitea registry...")
    r = subprocess.run(
        ["docker", "login", registry,
         "--username", GITEA_ADMIN_USER, "--password-stdin"],
        input=admin_pass, text=True, capture_output=True,
    )
    if r.returncode != 0:
        die(f"docker login failed:\n{r.stderr.strip()}")

    ok("Building image (linux/arm64, push)...")
    _run(["docker", "buildx", "build",
          "--platform", "linux/arm64",
          "--push",
          "-t", image,
          str(proxy_dir)])

    ok(f"Pushed {image}")

    # On single-node clusters, pre-seed the image directly into k3s containerd.
    # This breaks the circular dependency: when the proxy restarts, Pingora goes
    # down before the new pod starts, making the Gitea registry (behind Pingora)
    # unreachable for the image pull.  By importing into containerd first,
    # imagePullPolicy: IfNotPresent means k8s never needs to contact the registry.
    nodes = kube_out("get", "nodes", "-o=jsonpath={.items[*].metadata.name}").split()
    if len(nodes) == 1:
        ok("Single-node cluster: pre-seeding image into k3s containerd...")
        save = subprocess.Popen(
            ["docker", "save", image],
            stdout=subprocess.PIPE,
        )
        ctr = subprocess.run(
            ["limactl", "shell", LIMA_VM, "--",
             "sudo", "ctr", "-n", "k8s.io", "images", "import", "-"],
            stdin=save.stdout,
            capture_output=True,
        )
        save.stdout.close()
        save.wait()
        if ctr.returncode != 0:
            warn(f"containerd import failed (will fall back to registry pull):\n"
                 f"{ctr.stderr.decode().strip()}")
        else:
            ok("Image pre-seeded.")

    # Apply manifests so the Deployment spec reflects the Gitea image ref.
    from sunbeam.manifests import cmd_apply
    cmd_apply()

    # Roll the pingora pod.
    ok("Rolling pingora deployment...")
    kube("rollout", "restart", "deployment/pingora", "-n", "ingress")
    kube("rollout", "status", "deployment/pingora", "-n", "ingress",
         "--timeout=120s")
    ok("Pingora redeployed.")


def _build_kratos_admin():
    ip = get_lima_ip()
    domain = f"{ip}.sslip.io"

    b64 = kube_out("-n", "devtools", "get", "secret",
                   "gitea-admin-credentials", "-o=jsonpath={.data.password}")
    if not b64:
        die("gitea-admin-credentials secret not found -- run seed first.")
    admin_pass = base64.b64decode(b64).decode()

    if not shutil.which("docker"):
        die("docker not found -- is the Lima docker VM running?")

    # kratos-admin source
    kratos_admin_dir = Path(__file__).resolve().parents[2] / "kratos-admin"
    if not kratos_admin_dir.is_dir():
        die(f"kratos-admin source not found at {kratos_admin_dir}")

    registry = f"src.{domain}"
    image = f"{registry}/studio/kratos-admin-ui:latest"

    step(f"Building kratos-admin-ui -> {image} ...")

    _trust_registry_in_docker_vm(registry)

    ok("Logging in to Gitea registry...")
    r = subprocess.run(
        ["docker", "login", registry,
         "--username", GITEA_ADMIN_USER, "--password-stdin"],
        input=admin_pass, text=True, capture_output=True,
    )
    if r.returncode != 0:
        die(f"docker login failed:\n{r.stderr.strip()}")

    ok("Building image (linux/arm64, push)...")
    _run(["docker", "buildx", "build",
          "--platform", "linux/arm64",
          "--push",
          "-t", image,
          str(kratos_admin_dir)])

    ok(f"Pushed {image}")

    # Pre-seed into k3s containerd (same pattern as proxy)
    nodes = kube_out("get", "nodes", "-o=jsonpath={.items[*].metadata.name}").split()
    if len(nodes) == 1:
        ok("Single-node cluster: pre-seeding image into k3s containerd...")
        save = subprocess.Popen(
            ["docker", "save", image],
            stdout=subprocess.PIPE,
        )
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

    from sunbeam.manifests import cmd_apply
    cmd_apply()

    ok("Rolling kratos-admin-ui deployment...")
    kube("rollout", "restart", "deployment/kratos-admin-ui", "-n", "ory")
    kube("rollout", "status", "deployment/kratos-admin-ui", "-n", "ory",
         "--timeout=120s")
    ok("kratos-admin-ui redeployed.")
