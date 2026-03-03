"""Kubernetes interface — kubectl/kustomize wrappers, domain substitution, target parsing."""
import subprocess
from pathlib import Path

from sunbeam.tools import run_tool, CACHE_DIR
from sunbeam.output import die


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
    """Run kubectl with --context=sunbeam."""
    text = not isinstance(input, bytes)
    return run_tool("kubectl", "--context=sunbeam", *args,
                    input=input, text=text, check=check,
                    capture_output=False)


def kube_out(*args) -> str:
    """Run kubectl and return stdout (empty string on failure)."""
    r = run_tool("kubectl", "--context=sunbeam", *args,
                 capture_output=True, text=True, check=False)
    return r.stdout.strip() if r.returncode == 0 else ""


def kube_ok(*args) -> bool:
    """Return True if kubectl command exits 0."""
    r = run_tool("kubectl", "--context=sunbeam", *args,
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
    args = ["kubectl", "--context=sunbeam", "exec", "-n", ns, pod]
    if container:
        args += ["-c", container]
    args += ["--", *cmd]
    r = run_tool(*args, capture_output=True, text=True, check=False)
    return r.returncode, r.stdout.strip()


def get_domain() -> str:
    """Discover the active domain from cluster state.

    Reads a known substituted configmap value; falls back to the Lima VM IP.
    """
    raw = kube_out("get", "configmap", "lasuite-oidc-provider", "-n", "lasuite",
                   "-o=jsonpath={.data.OIDC_OP_JWKS_ENDPOINT}")
    if raw and "https://auth." in raw:
        # e.g. "https://auth.192.168.105.2.sslip.io/.well-known/jwks.json"
        return raw.split("https://auth.")[1].split("/")[0]
    ip = get_lima_ip()
    return f"{ip}.sslip.io"


def cmd_k8s(kubectl_args: list[str]) -> int:
    """Transparent kubectl --context=sunbeam passthrough. Returns kubectl's exit code."""
    from sunbeam.tools import ensure_tool
    import os
    bin_path = ensure_tool("kubectl")
    r = subprocess.run([str(bin_path), "--context=sunbeam", *kubectl_args])
    return r.returncode


def kustomize_build(overlay: Path, domain: str) -> str:
    """Run kustomize build --enable-helm and apply domain substitution."""
    r = run_tool(
        "kustomize", "build", "--enable-helm", str(overlay),
        capture_output=True, text=True, check=True,
    )
    text = r.stdout
    text = domain_replace(text, domain)
    text = text.replace("\n      annotations: null", "")
    return text
