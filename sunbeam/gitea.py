"""Gitea bootstrap — registry trust, admin setup, org creation."""
import base64
import json
import subprocess
import time

from sunbeam.kube import kube, kube_out
from sunbeam.output import step, ok, warn

LIMA_VM = "sunbeam"
GITEA_ADMIN_USER = "gitea_admin"
GITEA_ADMIN_EMAIL = "gitea@local.domain"
K8S_CTX = ["--context=sunbeam"]


def _capture_out(cmd, *, default=""):
    r = subprocess.run(cmd, capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else default


def _run(cmd, *, check=True, input=None, capture=False, cwd=None):
    text = not isinstance(input, bytes)
    return subprocess.run(cmd, check=check, text=text, input=input,
                          capture_output=capture, cwd=cwd)


def _kube_ok(*args):
    return subprocess.run(
        ["kubectl", *K8S_CTX, *args], capture_output=True
    ).returncode == 0


def setup_lima_vm_registry(domain: str, gitea_admin_pass: str = ""):
    """Install mkcert root CA in the Lima VM and configure k3s to auth with Gitea.

    Restarts k3s if either configuration changes so pods don't fight TLS errors
    or get unauthenticated pulls on the first deploy.
    """
    step("Configuring Lima VM registry trust...")
    changed = False

    # Install mkcert root CA so containerd trusts our wildcard TLS cert
    caroot = _capture_out(["mkcert", "-CAROOT"])
    if caroot:
        from pathlib import Path
        ca_pem = Path(caroot) / "rootCA.pem"
        if ca_pem.exists():
            already = subprocess.run(
                ["limactl", "shell", LIMA_VM, "test", "-f",
                 "/usr/local/share/ca-certificates/mkcert-root.crt"],
                capture_output=True,
            ).returncode == 0
            if not already:
                _run(["limactl", "copy", str(ca_pem),
                      f"{LIMA_VM}:/tmp/mkcert-root.pem"])
                _run(["limactl", "shell", LIMA_VM, "sudo", "cp",
                      "/tmp/mkcert-root.pem",
                      "/usr/local/share/ca-certificates/mkcert-root.crt"])
                _run(["limactl", "shell", LIMA_VM, "sudo",
                      "update-ca-certificates"])
                ok("mkcert CA installed in VM.")
                changed = True
            else:
                ok("mkcert CA already installed.")

    # Write k3s registries.yaml (auth for Gitea container registry)
    registry_host = f"src.{domain}"
    want = (
        f'configs:\n'
        f'  "{registry_host}":\n'
        f'    auth:\n'
        f'      username: "{GITEA_ADMIN_USER}"\n'
        f'      password: "{gitea_admin_pass}"\n'
    )
    existing = _capture_out(["limactl", "shell", LIMA_VM,
                             "sudo", "cat",
                             "/etc/rancher/k3s/registries.yaml"])
    if existing.strip() != want.strip():
        subprocess.run(
            ["limactl", "shell", LIMA_VM, "sudo", "tee",
             "/etc/rancher/k3s/registries.yaml"],
            input=want, text=True, capture_output=True,
        )
        ok(f"Registry config written for {registry_host}.")
        changed = True
    else:
        ok("Registry config up to date.")

    if changed:
        ok("Restarting k3s to apply changes...")
        subprocess.run(
            ["limactl", "shell", LIMA_VM, "sudo", "systemctl", "restart",
             "k3s"],
            capture_output=True,
        )
        # Wait for API server to come back
        for _ in range(40):
            if _kube_ok("get", "nodes"):
                break
            time.sleep(3)
        # Extra settle time -- pods take a moment to start terminating/restarting
        time.sleep(15)
        ok("k3s restarted.")


def cmd_bootstrap(domain: str = "", gitea_admin_pass: str = ""):
    """Ensure Gitea admin has a known password and create the studio/internal orgs."""
    if not domain:
        from sunbeam.kube import get_lima_ip
        ip = get_lima_ip()
        domain = f"{ip}.sslip.io"
    if not gitea_admin_pass:
        b64 = kube_out("-n", "devtools", "get", "secret",
                       "gitea-admin-credentials",
                       "-o=jsonpath={.data.password}")
        if b64:
            gitea_admin_pass = base64.b64decode(b64).decode()

    step("Bootstrapping Gitea...")

    # Wait for a Running + Ready Gitea pod
    pod = ""
    for _ in range(60):
        candidate = kube_out(
            "-n", "devtools", "get", "pods",
            "-l=app.kubernetes.io/name=gitea",
            "--field-selector=status.phase=Running",
            "-o=jsonpath={.items[0].metadata.name}",
        )
        if candidate:
            ready = kube_out("-n", "devtools", "get", "pod", candidate,
                             "-o=jsonpath={.status.containerStatuses[0].ready}")
            if ready == "true":
                pod = candidate
                break
        time.sleep(3)

    if not pod:
        warn("Gitea pod not ready after 3 min -- skipping bootstrap.")
        return

    def gitea_exec(*args):
        return subprocess.run(
            ["kubectl", *K8S_CTX, "-n", "devtools", "exec", pod, "-c",
             "gitea", "--"] + list(args),
            capture_output=True, text=True,
        )

    # Ensure admin has the generated password and no forced-change flag.
    r = gitea_exec("gitea", "admin", "user", "change-password",
                   "--username", GITEA_ADMIN_USER, "--password",
                   gitea_admin_pass, "--must-change-password=false")
    if r.returncode == 0 or "password" in (r.stdout + r.stderr).lower():
        ok(f"Admin '{GITEA_ADMIN_USER}' password set.")
    else:
        warn(f"change-password: {r.stderr.strip()}")

    def api(method, path, data=None):
        args = [
            "curl", "-s", "-X", method,
            f"http://localhost:3000/api/v1{path}",
            "-H", "Content-Type: application/json",
            "-u", f"{GITEA_ADMIN_USER}:{gitea_admin_pass}",
        ]
        if data:
            args += ["-d", json.dumps(data)]
        r = gitea_exec(*args)
        try:
            return json.loads(r.stdout)
        except json.JSONDecodeError:
            return {}

    for org_name, visibility, desc in [
        ("studio",   "public",  "Public source code"),
        ("internal", "private", "Internal tools and services"),
    ]:
        result = api("POST", "/orgs", {
            "username":    org_name,
            "visibility":  visibility,
            "description": desc,
        })
        if "id" in result:
            ok(f"Created org '{org_name}'.")
        elif "already" in result.get("message", "").lower():
            ok(f"Org '{org_name}' already exists.")
        else:
            warn(f"Org '{org_name}': {result.get('message', result)}")

    ok(f"Gitea ready -- https://src.{domain} ({GITEA_ADMIN_USER} / <from "
       f"openbao>)")
