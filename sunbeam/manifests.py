"""Manifest build + apply — kustomize overlay with domain substitution."""
from pathlib import Path

from sunbeam.kube import kube, kube_out, kube_ok, kube_apply, kustomize_build, get_lima_ip
from sunbeam.output import step, ok, warn

REPO_ROOT = Path(__file__).parents[2] / "infrastructure"
MANAGED_NS = ["data", "devtools", "ingress", "lasuite", "media", "ory", "storage",
              "vault-secrets-operator"]


def pre_apply_cleanup():
    """Delete immutable resources that must be re-created on each apply.

    Also prunes VaultStaticSecrets that share a name with a VaultDynamicSecret --
    kubectl apply doesn't delete the old resource when a manifest switches kinds,
    and VSO refuses to overwrite a secret owned by a different resource type.
    """
    ok("Cleaning up immutable Jobs and test Pods...")
    for ns in MANAGED_NS:
        kube("delete", "jobs", "--all", "-n", ns, "--ignore-not-found", check=False)
        # Query all pods (no phase filter) — CrashLoopBackOff pods report phase=Running
        # so filtering on phase!=Running would silently skip them.
        pods_out = kube_out("get", "pods", "-n", ns,
                            "-o=jsonpath={.items[*].metadata.name}")
        for pod in pods_out.split():
            if pod.endswith(("-test-connection", "-server-test", "-test")):
                kube("delete", "pod", pod, "-n", ns, "--ignore-not-found", check=False)

    # Prune VaultStaticSecrets that were replaced by VaultDynamicSecrets.
    # When a manifest transitions a resource from VSS -> VDS, apply won't delete
    # the old VSS; it just creates the new VDS alongside it. VSO then errors
    # "not the owner" because the K8s secret's ownerRef still points to the VSS.
    ok("Pruning stale VaultStaticSecrets superseded by VaultDynamicSecrets...")
    for ns in MANAGED_NS:
        vss_names = set(kube_out(
            "get", "vaultstaticsecret", "-n", ns,
            "-o=jsonpath={.items[*].metadata.name}", "--ignore-not-found",
        ).split())
        vds_names = set(kube_out(
            "get", "vaultdynamicsecret", "-n", ns,
            "-o=jsonpath={.items[*].metadata.name}", "--ignore-not-found",
        ).split())
        for stale in vss_names & vds_names:
            ok(f"  deleting stale VaultStaticSecret {ns}/{stale}")
            kube("delete", "vaultstaticsecret", stale, "-n", ns,
                 "--ignore-not-found", check=False)


def cmd_apply():
    """Get Lima IP, build domain, kustomize_build, kube_apply."""
    ip = get_lima_ip()
    domain = f"{ip}.sslip.io"
    step(f"Applying manifests (domain: {domain})...")
    pre_apply_cleanup()
    manifests = kustomize_build(REPO_ROOT / "overlays" / "local", domain)
    kube("apply", "--server-side", "--force-conflicts", "-f", "-", input=manifests)
    ok("Applied.")
