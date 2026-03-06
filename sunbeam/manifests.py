"""Manifest build + apply — kustomize overlay with domain substitution."""
import time
from pathlib import Path

from sunbeam.kube import kube, kube_out, kube_ok, kube_apply, kustomize_build, get_lima_ip, get_domain
from sunbeam.output import step, ok, warn

REPO_ROOT = Path(__file__).parents[2] / "infrastructure"
MANAGED_NS = ["data", "devtools", "ingress", "lasuite", "media", "monitoring", "ory",
              "storage", "vault-secrets-operator"]


def pre_apply_cleanup(namespaces=None):
    """Delete immutable resources that must be re-created on each apply.

    Also prunes VaultStaticSecrets that share a name with a VaultDynamicSecret --
    kubectl apply doesn't delete the old resource when a manifest switches kinds,
    and VSO refuses to overwrite a secret owned by a different resource type.

    namespaces: if given, only clean those namespaces; otherwise clean all MANAGED_NS.
    """
    ns_list = namespaces if namespaces is not None else MANAGED_NS
    ok("Cleaning up immutable Jobs and test Pods...")
    for ns in ns_list:
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
    for ns in ns_list:
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


def _snapshot_configmaps() -> dict:
    """Return {ns/name: resourceVersion} for all ConfigMaps in managed namespaces."""
    result = {}
    for ns in MANAGED_NS:
        out = kube_out(
            "get", "configmaps", "-n", ns, "--ignore-not-found",
            "-o=jsonpath={range .items[*]}{.metadata.name}={.metadata.resourceVersion}\\n{end}",
        )
        for line in out.splitlines():
            if "=" in line:
                name, rv = line.split("=", 1)
                result[f"{ns}/{name}"] = rv
    return result


def _restart_for_changed_configmaps(before: dict, after: dict):
    """Restart deployments that mount any ConfigMap whose resourceVersion changed."""
    changed_by_ns: dict = {}
    for key, rv in after.items():
        if before.get(key) != rv:
            ns, name = key.split("/", 1)
            changed_by_ns.setdefault(ns, set()).add(name)

    for ns, cm_names in changed_by_ns.items():
        out = kube_out(
            "get", "deployments", "-n", ns, "--ignore-not-found",
            "-o=jsonpath={range .items[*]}{.metadata.name}:"
            "{range .spec.template.spec.volumes[*]}{.configMap.name},{end};{end}",
        )
        for entry in out.split(";"):
            entry = entry.strip()
            if not entry or ":" not in entry:
                continue
            dep, vols = entry.split(":", 1)
            mounted = {v.strip() for v in vols.split(",") if v.strip()}
            if mounted & cm_names:
                ok(f"Restarting {ns}/{dep} (ConfigMap updated)...")
                kube("rollout", "restart", f"deployment/{dep}", "-n", ns, check=False)


def _wait_for_webhook(ns: str, svc: str, timeout: int = 120) -> bool:
    """Poll until a webhook service endpoint exists (signals webhook is ready).

    Returns True if the webhook is ready within timeout seconds.
    """
    ok(f"Waiting for {ns}/{svc} webhook (up to {timeout}s)...")
    deadline = time.time() + timeout
    while time.time() < deadline:
        eps = kube_out("get", "endpoints", svc, "-n", ns,
                       "-o=jsonpath={.subsets[0].addresses[0].ip}", "--ignore-not-found")
        if eps:
            ok(f"  {ns}/{svc} ready.")
            return True
        time.sleep(3)
    warn(f"  {ns}/{svc} not ready after {timeout}s — continuing anyway.")
    return False


def _apply_mkcert_ca_configmap():
    """Create/update gitea-mkcert-ca ConfigMap from the local mkcert root CA.

    Only called in local env. The ConfigMap is mounted into Gitea so Go's TLS
    stack trusts the mkcert wildcard cert when making server-side OIDC calls.
    """
    import subprocess, json
    from pathlib import Path
    caroot = subprocess.run(["mkcert", "-CAROOT"], capture_output=True, text=True).stdout.strip()
    if not caroot:
        warn("mkcert not found — skipping gitea-mkcert-ca ConfigMap.")
        return
    ca_pem = Path(caroot) / "rootCA.pem"
    if not ca_pem.exists():
        warn(f"mkcert root CA not found at {ca_pem} — skipping.")
        return
    cm = json.dumps({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {"name": "gitea-mkcert-ca", "namespace": "devtools"},
        "data": {"ca.crt": ca_pem.read_text()},
    })
    kube("apply", "--server-side", "-f", "-", input=cm)
    ok("gitea-mkcert-ca ConfigMap applied.")


def _filter_by_namespace(manifests: str, namespace: str) -> str:
    """Return only the YAML documents that belong to the given namespace.

    Also includes the Namespace resource itself (safe to re-apply).
    Uses simple string matching — namespace always appears as 'namespace: <name>'
    in top-level metadata, so this is reliable without a full YAML parser.
    """
    kept = []
    for doc in manifests.split("\n---"):
        doc = doc.strip()
        if not doc:
            continue
        if f"namespace: {namespace}" in doc:
            kept.append(doc)
        elif "kind: Namespace" in doc and f"name: {namespace}" in doc:
            kept.append(doc)
    if not kept:
        return ""
    return "---\n" + "\n---\n".join(kept) + "\n"


def cmd_apply(env: str = "local", domain: str = "", email: str = "", namespace: str = ""):
    """Build kustomize overlay for env, substitute domain/email, kubectl apply.

    Runs a second convergence pass if cert-manager is present in the overlay —
    cert-manager registers a ValidatingWebhook that must be running before
    ClusterIssuer / Certificate resources can be created.
    """
    if env == "production":
        if not domain:
            # Try to discover domain from running cluster
            domain = get_domain()
        if not domain:
            from sunbeam.output import die
            die("--domain is required for production apply on first deploy")
        overlay = REPO_ROOT / "overlays" / "production"
    else:
        ip = get_lima_ip()
        domain = f"{ip}.sslip.io"
        overlay = REPO_ROOT / "overlays" / "local"

    scope = f" [{namespace}]" if namespace else ""
    step(f"Applying manifests (env: {env}, domain: {domain}){scope}...")
    if env == "local":
        _apply_mkcert_ca_configmap()
    ns_list = [namespace] if namespace else None
    pre_apply_cleanup(namespaces=ns_list)
    before = _snapshot_configmaps()
    manifests = kustomize_build(overlay, domain, email=email)

    if namespace:
        manifests = _filter_by_namespace(manifests, namespace)
        if not manifests.strip():
            warn(f"No resources found for namespace '{namespace}' — check the name and try again.")
            return

    # First pass: may emit errors for resources that depend on webhooks not yet running
    # (e.g. cert-manager ClusterIssuer/Certificate), which is expected on first deploy.
    kube("apply", "--server-side", "--force-conflicts", "-f", "-",
         input=manifests, check=False)

    # If cert-manager is in the overlay, wait for its webhook then re-apply
    # so that ClusterIssuer and Certificate resources converge.
    # Skip for partial applies unless the target IS cert-manager.
    cert_manager_present = (overlay / "../../base/cert-manager").resolve().exists()
    if cert_manager_present and not namespace:
        if _wait_for_webhook("cert-manager", "cert-manager-webhook", timeout=120):
            ok("Running convergence pass for cert-manager resources...")
            manifests2 = kustomize_build(overlay, domain, email=email)
            kube("apply", "--server-side", "--force-conflicts", "-f", "-", input=manifests2)

    _restart_for_changed_configmaps(before, _snapshot_configmaps())
    ok("Applied.")
