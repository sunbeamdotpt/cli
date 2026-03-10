"""Cluster lifecycle — Lima VM, kubeconfig, Linkerd, TLS, core service readiness."""
import base64
import json
import shutil
import subprocess
import time
from pathlib import Path

from sunbeam.kube import (kube, kube_out, kube_ok, kube_apply,
                           kustomize_build, get_lima_ip, ensure_ns, create_secret, ns_exists)
from sunbeam.tools import run_tool, CACHE_DIR
from sunbeam.output import step, ok, warn, die

LIMA_VM = "sunbeam"
from sunbeam.config import get_infra_dir as _get_infra_dir
SECRETS_DIR = _get_infra_dir() / "secrets" / "local"

GITEA_ADMIN_USER = "gitea_admin"


# ---------------------------------------------------------------------------
# Lima VM
# ---------------------------------------------------------------------------

def _lima_status() -> str:
    """Return the Lima VM status, handling both JSON-array and NDJSON output."""
    r = subprocess.run(["limactl", "list", "--json"],
                       capture_output=True, text=True)
    raw = r.stdout.strip() if r.returncode == 0 else ""
    if not raw:
        return "none"
    vms: list[dict] = []
    try:
        parsed = json.loads(raw)
        vms = parsed if isinstance(parsed, list) else [parsed]
    except json.JSONDecodeError:
        for line in raw.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                vms.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    for vm in vms:
        if vm.get("name") == LIMA_VM:
            return vm.get("status", "unknown")
    return "none"


def ensure_lima_vm():
    step("Lima VM...")
    status = _lima_status()
    if status == "none":
        ok("Creating 'sunbeam' (k3s  6 CPU / 12 GB / 60 GB)...")
        subprocess.run(
            ["limactl", "start",
             "--name=sunbeam", "template:k3s",
             "--memory=12", "--cpus=6", "--disk=60",
             "--vm-type=vz", "--mount-type=virtiofs",
             "--rosetta"],
            check=True,
        )
    elif status == "Running":
        ok("Already running.")
    else:
        ok(f"Starting (current status: {status})...")
        subprocess.run(["limactl", "start", LIMA_VM], check=True)


# ---------------------------------------------------------------------------
# Kubeconfig
# ---------------------------------------------------------------------------

def merge_kubeconfig():
    step("Merging kubeconfig...")
    lima_kube = Path.home() / f".lima/{LIMA_VM}/copied-from-guest/kubeconfig.yaml"
    if not lima_kube.exists():
        die(f"Lima kubeconfig not found: {lima_kube}")

    tmp = Path("/tmp/sunbeam-kube")
    tmp.mkdir(exist_ok=True)
    try:
        for query, filename in [
            (".clusters[0].cluster.certificate-authority-data", "ca.crt"),
            (".users[0].user.client-certificate-data",          "client.crt"),
            (".users[0].user.client-key-data",                  "client.key"),
        ]:
            r = subprocess.run(["yq", query, str(lima_kube)],
                               capture_output=True, text=True)
            b64 = r.stdout.strip() if r.returncode == 0 else ""
            (tmp / filename).write_bytes(base64.b64decode(b64))

        subprocess.run(
            ["kubectl", "config", "set-cluster", LIMA_VM,
             "--server=https://127.0.0.1:6443",
             f"--certificate-authority={tmp}/ca.crt", "--embed-certs=true"],
            check=True,
        )
        subprocess.run(
            ["kubectl", "config", "set-credentials", f"{LIMA_VM}-admin",
             f"--client-certificate={tmp}/client.crt",
             f"--client-key={tmp}/client.key", "--embed-certs=true"],
            check=True,
        )
        subprocess.run(
            ["kubectl", "config", "set-context", LIMA_VM,
             f"--cluster={LIMA_VM}", f"--user={LIMA_VM}-admin"],
            check=True,
        )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    ok("Context 'sunbeam' ready.")


# ---------------------------------------------------------------------------
# Traefik
# ---------------------------------------------------------------------------

def disable_traefik():
    step("Traefik...")
    if kube_ok("get", "helmchart", "traefik", "-n", "kube-system"):
        ok("Removing (replaced by Pingora)...")
        kube("delete", "helmchart", "traefik", "traefik-crd",
             "-n", "kube-system", check=False)
    subprocess.run(
        ["limactl", "shell", LIMA_VM,
         "sudo", "rm", "-f",
         "/var/lib/rancher/k3s/server/manifests/traefik.yaml"],
        capture_output=True,
    )
    # Write k3s config so Traefik can never return after a k3s restart.
    subprocess.run(
        ["limactl", "shell", LIMA_VM, "sudo", "tee",
         "/etc/rancher/k3s/config.yaml"],
        input="disable:\n  - traefik\n",
        text=True,
        capture_output=True,
    )
    ok("Done.")


# ---------------------------------------------------------------------------
# cert-manager
# ---------------------------------------------------------------------------

def ensure_cert_manager():
    step("cert-manager...")
    if ns_exists("cert-manager"):
        ok("Already installed.")
        return
    ok("Installing...")
    kube("apply", "-f",
         "https://github.com/cert-manager/cert-manager/releases/download/v1.17.0/cert-manager.yaml")
    for dep in ["cert-manager", "cert-manager-webhook", "cert-manager-cainjector"]:
        kube("rollout", "status", f"deployment/{dep}",
             "-n", "cert-manager", "--timeout=120s")
    ok("Installed.")


# ---------------------------------------------------------------------------
# Linkerd
# ---------------------------------------------------------------------------

def ensure_linkerd():
    step("Linkerd...")
    if ns_exists("linkerd"):
        ok("Already installed.")
        return
    ok("Installing Gateway API CRDs...")
    kube("apply", "--server-side", "-f",
         "https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.4.0/standard-install.yaml")
    ok("Installing Linkerd CRDs...")
    r = subprocess.run(["linkerd", "install", "--crds"],
                       capture_output=True, text=True)
    crds = r.stdout.strip() if r.returncode == 0 else ""
    kube_apply(crds)
    ok("Installing Linkerd control plane...")
    r = subprocess.run(["linkerd", "install"],
                       capture_output=True, text=True)
    cp = r.stdout.strip() if r.returncode == 0 else ""
    kube_apply(cp)
    for dep in ["linkerd-identity", "linkerd-destination", "linkerd-proxy-injector"]:
        kube("rollout", "status", f"deployment/{dep}",
             "-n", "linkerd", "--timeout=120s")
    ok("Installed.")


# ---------------------------------------------------------------------------
# TLS certificate
# ---------------------------------------------------------------------------

def ensure_tls_cert(domain: str | None = None) -> str:
    step("TLS certificate...")
    ip = get_lima_ip()
    if domain is None:
        domain = f"{ip}.sslip.io"
    cert = SECRETS_DIR / "tls.crt"
    if cert.exists():
        ok(f"Cert exists. Domain: {domain}")
        return domain
    ok(f"Generating wildcard cert for *.{domain}...")
    SECRETS_DIR.mkdir(parents=True, exist_ok=True)
    subprocess.run(["mkcert", f"*.{domain}"], cwd=SECRETS_DIR, check=True)
    for src, dst in [
        (f"_wildcard.{domain}.pem",     "tls.crt"),
        (f"_wildcard.{domain}-key.pem", "tls.key"),
    ]:
        (SECRETS_DIR / src).rename(SECRETS_DIR / dst)
    ok(f"Cert generated. Domain: {domain}")
    return domain


# ---------------------------------------------------------------------------
# TLS secret
# ---------------------------------------------------------------------------

def ensure_tls_secret(domain: str):
    step("TLS secret...")
    ensure_ns("ingress")
    manifest = kube_out(
        "create", "secret", "tls", "pingora-tls",
        f"--cert={SECRETS_DIR}/tls.crt",
        f"--key={SECRETS_DIR}/tls.key",
        "-n", "ingress",
        "--dry-run=client", "-o=yaml",
    )
    if manifest:
        kube_apply(manifest)
    ok("Done.")


# ---------------------------------------------------------------------------
# Wait for core
# ---------------------------------------------------------------------------

def wait_for_core():
    step("Waiting for core services...")
    for ns, dep in [("data", "valkey"), ("ory", "kratos"), ("ory", "hydra")]:
        kube("rollout", "status", f"deployment/{dep}",
             "-n", ns, "--timeout=120s", check=False)
    ok("Core services ready.")


# ---------------------------------------------------------------------------
# Print URLs
# ---------------------------------------------------------------------------

def print_urls(domain: str, gitea_admin_pass: str = ""):
    print(f"\n{'─' * 60}")
    print(f"  Stack is up.  Domain: {domain}")
    print(f"{'─' * 60}")
    for name, url in [
        ("Auth",     f"https://auth.{domain}/"),
        ("Docs",     f"https://docs.{domain}/"),
        ("Meet",     f"https://meet.{domain}/"),
        ("Drive",    f"https://drive.{domain}/"),
        ("Chat",     f"https://chat.{domain}/"),
        ("Mail",     f"https://mail.{domain}/"),
        ("People",   f"https://people.{domain}/"),
        ("Gitea",    f"https://src.{domain}/  ({GITEA_ADMIN_USER} / {gitea_admin_pass})"),
    ]:
        print(f"  {name:<10} {url}")
    print()
    print("  OpenBao UI:")
    print(f"    kubectl --context=sunbeam -n data port-forward svc/openbao 8200:8200")
    print(f"    http://localhost:8200")
    token_cmd = "kubectl --context=sunbeam -n data get secret openbao-keys -o jsonpath='{.data.root-token}' | base64 -d"
    print(f"    token: {token_cmd}")
    print(f"{'─' * 60}\n")


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------

def cmd_up():
    from sunbeam.manifests import cmd_apply
    from sunbeam.secrets import cmd_seed
    from sunbeam.gitea import cmd_bootstrap, setup_lima_vm_registry
    from sunbeam.images import cmd_mirror

    ensure_lima_vm()
    merge_kubeconfig()
    disable_traefik()
    ensure_cert_manager()
    ensure_linkerd()
    domain = ensure_tls_cert()
    ensure_tls_secret(domain)
    cmd_apply()
    creds = cmd_seed()
    admin_pass = creds.get("gitea-admin-password", "") if isinstance(creds, dict) else ""
    setup_lima_vm_registry(domain, admin_pass)
    cmd_bootstrap()
    cmd_mirror()
    wait_for_core()
    print_urls(domain, admin_pass)


def cmd_down():
    subprocess.run(["limactl", "stop", LIMA_VM])
