"""Kubernetes interface — kubectl/kustomize wrappers, domain substitution, target parsing."""
import subprocess
import time
from contextlib import contextmanager
from pathlib import Path

from sunbeam.tools import run_tool, CACHE_DIR
from sunbeam.output import die, ok

# Active kubectl context. Set once at startup via set_context().
# Defaults to "sunbeam" (Lima VM) for local dev.
_context: str = "sunbeam"

# SSH host for production tunnel. Set alongside context for production env.
_ssh_host: str = ""
_tunnel_proc: subprocess.Popen | None = None


def set_context(ctx: str, ssh_host: str = "") -> None:
    global _context, _ssh_host
    _context = ctx
    _ssh_host = ssh_host


def context_arg() -> str:
    """Return '--context=<active>' for use in subprocess command lists."""
    return f"--context={_context}"


def ensure_tunnel() -> None:
    """Open SSH tunnel to localhost:16443 → remote:6443 for production if needed."""
    global _tunnel_proc
    if not _ssh_host:
        return
    import socket
    try:
        with socket.create_connection(("127.0.0.1", 16443), timeout=0.5):
            return  # already open
    except (ConnectionRefusedError, TimeoutError, OSError):
        pass
    ok(f"Opening SSH tunnel to {_ssh_host}...")
    _tunnel_proc = subprocess.Popen(
        ["ssh", "-p", "2222", "-L", "16443:127.0.0.1:6443", "-N", "-o", "ExitOnForwardFailure=yes",
         "-o", "StrictHostKeyChecking=no", _ssh_host],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    for _ in range(10):
        try:
            with socket.create_connection(("127.0.0.1", 16443), timeout=0.5):
                return
        except (ConnectionRefusedError, TimeoutError, OSError):
            time.sleep(0.5)
    die(f"SSH tunnel to {_ssh_host} did not open in time")


def parse_target(s: str | None) -> tuple[str | None, str | None]:
    """Parse 'ns/name' -> ('ns', 'name'), 'ns' -> ('ns', None), None -> (None, None)."""
    if s is None:
        return (None, None)
    parts = s.split("/")
    if len(parts) == 1:
        return (parts[0], None)
    if len(parts) == 2:
        return (parts[0], parts[1])
    raise ValueError(f"Invalid target {s!r}: expected 'namespace' or 'namespace/name'")


def domain_replace(text: str, domain: str) -> str:
    """Replace all occurrences of DOMAIN_SUFFIX with domain."""
    return text.replace("DOMAIN_SUFFIX", domain)


def get_lima_ip() -> str:
    """Get the socket_vmnet IP of the Lima sunbeam VM (192.168.105.x)."""
    r = subprocess.run(
        ["limactl", "shell", "sunbeam", "ip", "-4", "addr", "show", "eth1"],
        capture_output=True, text=True,
    )
    for line in r.stdout.splitlines():
        if "inet " in line:
            return line.strip().split()[1].split("/")[0]
    # fallback: second IP from hostname -I
    r2 = subprocess.run(
        ["limactl", "shell", "sunbeam", "hostname", "-I"],
        capture_output=True, text=True,
    )
    ips = r2.stdout.strip().split()
    return ips[-1] if len(ips) >= 2 else (ips[0] if ips else "")


def kube(*args, input=None, check=True) -> subprocess.CompletedProcess:
    """Run kubectl against the active context, opening SSH tunnel if needed."""
    ensure_tunnel()
    text = not isinstance(input, bytes)
    return run_tool("kubectl", context_arg(), *args,
                    input=input, text=text, check=check,
                    capture_output=False)


def kube_out(*args) -> str:
    """Run kubectl and return stdout (empty string on failure)."""
    ensure_tunnel()
    r = run_tool("kubectl", context_arg(), *args,
                 capture_output=True, text=True, check=False)
    return r.stdout.strip() if r.returncode == 0 else ""


def kube_ok(*args) -> bool:
    """Return True if kubectl command exits 0."""
    ensure_tunnel()
    r = run_tool("kubectl", context_arg(), *args,
                 capture_output=True, check=False)
    return r.returncode == 0


def kube_apply(manifest: str, *, server_side: bool = True) -> None:
    """Pipe manifest YAML to kubectl apply."""
    args = ["apply", "-f", "-"]
    if server_side:
        args += ["--server-side", "--force-conflicts"]
    kube(*args, input=manifest)


def ns_exists(ns: str) -> bool:
    return kube_ok("get", "namespace", ns)


def ensure_ns(ns: str) -> None:
    manifest = kube_out("create", "namespace", ns, "--dry-run=client", "-o=yaml")
    if manifest:
        kube_apply(manifest)


def create_secret(ns: str, name: str, **literals) -> None:
    """Create or update a K8s generic secret idempotently via server-side apply."""
    args = ["create", "secret", "generic", name, f"-n={ns}"]
    for k, v in literals.items():
        args.append(f"--from-literal={k}={v}")
    args += ["--dry-run=client", "-o=yaml"]
    manifest = kube_out(*args)
    if manifest:
        kube("apply", "--server-side", "--force-conflicts",
             "--field-manager=sunbeam", "-f", "-", input=manifest)


def kube_exec(ns: str, pod: str, *cmd: str, container: str | None = None) -> tuple[int, str]:
    """Run a command inside a pod. Returns (returncode, stdout)."""
    args = ["kubectl", context_arg(), "exec", "-n", ns, pod]
    if container:
        args += ["-c", container]
    args += ["--", *cmd]
    r = run_tool(*args, capture_output=True, text=True, check=False)
    return r.returncode, r.stdout.strip()


def get_domain() -> str:
    """Discover the active domain from cluster state.

    Tries multiple reliable anchors; falls back to the Lima VM IP for local dev.
    """
    import base64

    # 1. Gitea inline-config secret: server section contains DOMAIN=src.<domain>
    #    Works in both local and production because DOMAIN_SUFFIX is substituted
    #    into gitea-values.yaml at apply time.
    raw = kube_out("get", "secret", "gitea-inline-config", "-n", "devtools",
                   "-o=jsonpath={.data.server}", "--ignore-not-found")
    if raw:
        try:
            server_ini = base64.b64decode(raw).decode()
            for line in server_ini.splitlines():
                if line.startswith("DOMAIN=src."):
                    # e.g. "DOMAIN=src.sunbeam.pt"
                    return line.split("DOMAIN=src.", 1)[1].strip()
        except Exception:
            pass

    # 2. Fallback: lasuite-oidc-provider configmap (works if La Suite is deployed)
    raw2 = kube_out("get", "configmap", "lasuite-oidc-provider", "-n", "lasuite",
                    "-o=jsonpath={.data.OIDC_OP_JWKS_ENDPOINT}", "--ignore-not-found")
    if raw2 and "https://auth." in raw2:
        return raw2.split("https://auth.")[1].split("/")[0]

    # 3. Local dev fallback
    ip = get_lima_ip()
    return f"{ip}.sslip.io"


def cmd_k8s(kubectl_args: list[str]) -> int:
    """Transparent kubectl passthrough for the active context."""
    ensure_tunnel()
    from sunbeam.tools import ensure_tool
    bin_path = ensure_tool("kubectl")
    r = subprocess.run([str(bin_path), context_arg(), *kubectl_args])
    return r.returncode


def cmd_bao(bao_args: list[str]) -> int:
    """Run bao CLI inside the OpenBao pod with the root token. Returns exit code.

    Automatically resolves the pod name and root token from the cluster, then
    runs ``kubectl exec openbao-0 -- sh -c "VAULT_TOKEN=<tok> bao <args>"``
    so callers never need to handle raw kubectl exec or token management.
    """
    ob_pod = kube_out("-n", "data", "get", "pod",
                      "-l", "app.kubernetes.io/name=openbao",
                      "-o", "jsonpath={.items[0].metadata.name}")
    if not ob_pod:
        from sunbeam.output import die
        die("OpenBao pod not found — is the cluster running?")

    token_b64 = kube_out("-n", "data", "get", "secret", "openbao-keys",
                         "-o", "jsonpath={.data.root-token}")
    import base64
    root_token = base64.b64decode(token_b64).decode() if token_b64 else ""
    if not root_token:
        from sunbeam.output import die
        die("root-token not found in openbao-keys secret")

    cmd_str = "VAULT_TOKEN=" + root_token + " bao " + " ".join(bao_args)
    r = subprocess.run(
        ["kubectl", context_arg(), "-n", "data", "exec", ob_pod,
         "-c", "openbao", "--", "sh", "-c", cmd_str]
    )
    return r.returncode


def kustomize_build(overlay: Path, domain: str, email: str = "") -> str:
    """Run kustomize build --enable-helm and apply domain/email substitution."""
    r = run_tool(
        "kustomize", "build", "--enable-helm", str(overlay),
        capture_output=True, text=True, check=True,
    )
    text = r.stdout
    text = domain_replace(text, domain)
    if email:
        text = text.replace("ACME_EMAIL", email)
    text = text.replace("\n      annotations: null", "")
    return text
