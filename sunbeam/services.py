"""Service management — status, logs, restart."""
import subprocess
import sys
from pathlib import Path

import sunbeam.kube as _kube_mod
from sunbeam.kube import kube, kube_out, parse_target
from sunbeam.tools import ensure_tool
from sunbeam.output import step, ok, warn, die

MANAGED_NS = ["data", "devtools", "ingress", "lasuite", "matrix", "media", "ory",
              "storage", "vault-secrets-operator"]

SERVICES_TO_RESTART = [
    ("ory",      "hydra"),
    ("ory",      "kratos"),
    ("ory",      "login-ui"),
    ("devtools", "gitea"),
    ("storage",  "seaweedfs-filer"),
    ("lasuite",  "hive"),
    ("lasuite",  "people-backend"),
    ("lasuite",  "people-frontend"),
    ("lasuite",  "people-celery-worker"),
    ("lasuite",  "people-celery-beat"),
    ("lasuite",  "projects"),
    ("matrix",   "tuwunel"),
    ("media",    "livekit-server"),
]


def _k8s_ctx():
    """Return the kubectl --context flag matching the active environment."""
    return [_kube_mod.context_arg()]


def _capture_out(cmd, *, default=""):
    r = subprocess.run(cmd, capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else default


def _vso_sync_status():
    """Print VSO VaultStaticSecret and VaultDynamicSecret sync health.

    VSS synced = status.secretMAC is non-empty.
    VDS synced = status.lastRenewalTime is non-zero.
    """
    step("VSO secret sync status...")
    all_ok = True

    # VaultStaticSecrets: synced when secretMAC is populated
    vss_raw = _capture_out([
        "kubectl", *_k8s_ctx(), "get", "vaultstaticsecret", "-A", "--no-headers",
        "-o=custom-columns="
        "NS:.metadata.namespace,NAME:.metadata.name,MAC:.status.secretMAC",
    ])
    cur_ns = None
    for line in sorted(vss_raw.splitlines()):
        cols = line.split()
        if len(cols) < 2:
            continue
        ns, name = cols[0], cols[1]
        mac = cols[2] if len(cols) > 2 else ""
        synced = bool(mac and mac != "<none>")
        if not synced:
            all_ok = False
        icon = "\u2713" if synced else "\u2717"
        if ns != cur_ns:
            print(f"  {ns} (VSS):")
            cur_ns = ns
        print(f"    {icon} {name}")

    # VaultDynamicSecrets: synced when lastRenewalTime is non-zero
    vds_raw = _capture_out([
        "kubectl", *_k8s_ctx(), "get", "vaultdynamicsecret", "-A", "--no-headers",
        "-o=custom-columns="
        "NS:.metadata.namespace,NAME:.metadata.name,RENEWED:.status.lastRenewalTime",
    ])
    cur_ns = None
    for line in sorted(vds_raw.splitlines()):
        cols = line.split()
        if len(cols) < 2:
            continue
        ns, name = cols[0], cols[1]
        renewed = cols[2] if len(cols) > 2 else "0"
        synced = renewed not in ("", "0", "<none>")
        if not synced:
            all_ok = False
        icon = "\u2713" if synced else "\u2717"
        if ns != cur_ns:
            print(f"  {ns} (VDS):")
            cur_ns = ns
        print(f"    {icon} {name}")

    print()
    if all_ok:
        ok("All VSO secrets synced.")
    else:
        warn("Some VSO secrets are not synced.")


def cmd_status(target: str | None):
    """Show pod health, optionally filtered by namespace or namespace/service."""
    step("Pod health across all namespaces...")

    ns_set = set(MANAGED_NS)

    if target is None:
        # All pods across managed namespaces
        raw = _capture_out([
            "kubectl", *_k8s_ctx(),
            "get", "pods",
            "--field-selector=metadata.namespace!= kube-system",
            "-A", "--no-headers",
        ])
        pods = []
        for line in raw.splitlines():
            cols = line.split()
            if len(cols) < 4:
                continue
            ns = cols[0]
            if ns not in ns_set:
                continue
            pods.append(cols)
    else:
        ns, name = parse_target(target)
        if name:
            # Specific service: namespace/service
            raw = _capture_out([
                "kubectl", *_k8s_ctx(),
                "get", "pods", "-n", ns, "-l", f"app={name}", "--no-headers",
            ])
            pods = []
            for line in raw.splitlines():
                cols = line.split()
                if len(cols) < 3:
                    continue
                # Prepend namespace since -n output doesn't include it
                pods.append([ns] + cols)
        else:
            # Namespace only
            raw = _capture_out([
                "kubectl", *_k8s_ctx(),
                "get", "pods", "-n", ns, "--no-headers",
            ])
            pods = []
            for line in raw.splitlines():
                cols = line.split()
                if len(cols) < 3:
                    continue
                pods.append([ns] + cols)

    if not pods:
        warn("No pods found in managed namespaces.")
        return

    all_ok = True
    cur_ns = None
    icon_map = {"Running": "\u2713", "Completed": "\u2713", "Succeeded": "\u2713",
                "Pending": "\u25cb", "Failed": "\u2717", "Unknown": "?"}
    for cols in sorted(pods, key=lambda c: (c[0], c[1])):
        ns, name, ready, status = cols[0], cols[1], cols[2], cols[3]
        if ns != cur_ns:
            print(f"  {ns}:")
            cur_ns = ns
        icon = icon_map.get(status, "?")
        unhealthy = status not in ("Running", "Completed", "Succeeded")
        # Only check ready ratio for Running pods — Completed/Succeeded pods
        # legitimately report 0/N containers ready.
        if not unhealthy and status == "Running" and "/" in ready:
            r, t = ready.split("/")
            unhealthy = r != t
        if unhealthy:
            all_ok = False
        print(f"    {icon} {name:<50} {ready:<6} {status}")

    print()
    if all_ok:
        ok("All pods healthy.")
    else:
        warn("Some pods are not ready.")

    _vso_sync_status()


def cmd_logs(target: str, follow: bool):
    """Stream logs for a service. Target must include service name (e.g. ory/kratos)."""
    ns, name = parse_target(target)
    if not name:
        die("Logs require a service name, e.g. 'ory/kratos'.")

    _kube_mod.ensure_tunnel()
    kubectl = str(ensure_tool("kubectl"))
    cmd = [kubectl, _kube_mod.context_arg(), "-n", ns, "logs",
           "-l", f"app={name}", "--tail=100"]
    if follow:
        cmd.append("--follow")

    proc = subprocess.Popen(cmd)
    proc.wait()


def cmd_get(target: str, output: str = "yaml"):
    """Print raw kubectl get output for a pod or resource (ns/name).

    Usage: sunbeam get vault-secrets-operator/vault-secrets-operator-test
           sunbeam get ory/kratos-abc -o json
    """
    ns, name = parse_target(target)
    if not ns or not name:
        die("get requires namespace/name, e.g. 'sunbeam get ory/kratos-abc'")
    # Try pod first, fall back to any resource type if caller passes kind/ns/name
    result = kube_out("get", "pod", name, "-n", ns, f"-o={output}")
    if not result:
        die(f"Pod {ns}/{name} not found.")
    print(result)


def cmd_restart(target: str | None):
    """Restart deployments. None=all, 'ory'=namespace, 'ory/kratos'=specific."""
    step("Restarting services...")

    if target is None:
        matched = SERVICES_TO_RESTART
    else:
        ns, name = parse_target(target)
        if name:
            matched = [(n, d) for n, d in SERVICES_TO_RESTART if n == ns and d == name]
        else:
            matched = [(n, d) for n, d in SERVICES_TO_RESTART if n == ns]

    if not matched:
        warn(f"No matching services for target: {target}")
        return

    for ns, dep in matched:
        kube("-n", ns, "rollout", "restart", f"deployment/{dep}", check=False)
    ok("Done.")
