"""Manifest build + apply — kustomize overlay with domain substitution."""
import time
from pathlib import Path

from sunbeam.kube import kube, kube_out, kube_ok, kube_apply, kustomize_build, get_lima_ip, get_domain
from sunbeam.output import step, ok, warn

from sunbeam.config import get_infra_dir as _get_infra_dir
REPO_ROOT = _get_infra_dir()
MANAGED_NS = ["data", "devtools", "ingress", "lasuite", "matrix", "media", "monitoring",
              "ory", "storage", "vault-secrets-operator"]


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


def _patch_tuwunel_oauth2_redirect(domain: str):
    """Patch the tuwunel OAuth2Client redirect URI with the actual client_id.

    Hydra-maester generates the client_id when it first reconciles the
    OAuth2Client CRD, storing it in the oidc-tuwunel Secret. We read that
    secret and patch the CRD's redirectUris to include the correct callback
    path that tuwunel will use.
    """
    import base64, json

    client_id_b64 = kube_out("get", "secret", "oidc-tuwunel", "-n", "matrix",
                             "-o=jsonpath={.data.CLIENT_ID}", "--ignore-not-found")
    if not client_id_b64:
        warn("oidc-tuwunel secret not yet available — skipping redirect URI patch. "
             "Re-run 'sunbeam apply matrix' after hydra-maester has reconciled.")
        return

    client_id = base64.b64decode(client_id_b64).decode()
    redirect_uri = f"https://messages.{domain}/_matrix/client/unstable/login/sso/callback/{client_id}"

    # Check current redirect URIs to avoid unnecessary patches.
    current = kube_out("get", "oauth2client", "tuwunel", "-n", "matrix",
                       "-o=jsonpath={.spec.redirectUris[*]}", "--ignore-not-found")
    if redirect_uri in current.split():
        return

    patch = json.dumps({"spec": {"redirectUris": [redirect_uri]}})
    kube("patch", "oauth2client", "tuwunel", "-n", "matrix",
         "--type=merge", f"-p={patch}", check=False)
    ok(f"Patched tuwunel OAuth2Client redirect URI.")


def _os_api(path: str, method: str = "GET", data: str | None = None) -> str:
    """Call OpenSearch API via kubectl exec. Returns response body."""
    cmd = ["exec", "deploy/opensearch", "-n", "data", "-c", "opensearch", "--"]
    curl = ["curl", "-sf", f"http://localhost:9200{path}"]
    if method != "GET":
        curl += ["-X", method]
    if data is not None:
        curl += ["-H", "Content-Type: application/json", "-d", data]
    return kube_out(*cmd, *curl)


def _ensure_opensearch_ml():
    """Idempotently configure OpenSearch ML Commons for neural search.

    1. Sets cluster settings to allow ML on data nodes.
    2. Registers and deploys all-mpnet-base-v2 (pre-trained, 384-dim).
    3. Creates ingest + search pipelines for hybrid BM25+neural scoring.
    """
    import json, time

    # Check OpenSearch is reachable.
    if not _os_api("/_cluster/health"):
        warn("OpenSearch not reachable — skipping ML setup.")
        return

    # 1. Ensure ML Commons cluster settings (idempotent PUT).
    _os_api("/_cluster/settings", "PUT", json.dumps({"persistent": {
        "plugins.ml_commons.only_run_on_ml_node": False,
        "plugins.ml_commons.native_memory_threshold": 90,
        "plugins.ml_commons.model_access_control_enabled": False,
        "plugins.ml_commons.allow_registering_model_via_url": True,
    }}))

    # 2. Check if model already registered and deployed.
    search_resp = _os_api("/_plugins/_ml/models/_search", "POST",
                          '{"query":{"match":{"name":"huggingface/sentence-transformers/all-mpnet-base-v2"}}}')
    if not search_resp:
        warn("OpenSearch ML search API failed — skipping ML setup.")
        return

    resp = json.loads(search_resp)
    hits = resp.get("hits", {}).get("hits", [])
    model_id = None

    for hit in hits:
        state = hit.get("_source", {}).get("model_state", "")
        if state == "DEPLOYED":
            model_id = hit["_id"]
            break
        elif state in ("REGISTERED", "DEPLOYING"):
            model_id = hit["_id"]

    if model_id and any(h["_source"].get("model_state") == "DEPLOYED" for h in hits):
        pass  # Already deployed, skip to pipelines.
    elif model_id:
        # Registered but not deployed — deploy it.
        ok("Deploying OpenSearch ML model...")
        _os_api(f"/_plugins/_ml/models/{model_id}/_deploy", "POST")
        for _ in range(30):
            time.sleep(5)
            r = _os_api(f"/_plugins/_ml/models/{model_id}")
            if r and '"DEPLOYED"' in r:
                break
    else:
        # Register from pre-trained hub.
        ok("Registering OpenSearch ML model (all-mpnet-base-v2)...")
        reg_resp = _os_api("/_plugins/_ml/models/_register", "POST", json.dumps({
            "name": "huggingface/sentence-transformers/all-mpnet-base-v2",
            "version": "1.0.1",
            "model_format": "TORCH_SCRIPT",
        }))
        if not reg_resp:
            warn("Failed to register ML model — skipping.")
            return
        task_id = json.loads(reg_resp).get("task_id", "")
        if not task_id:
            warn("No task_id from model registration — skipping.")
            return

        # Wait for registration.
        ok("Waiting for model registration...")
        for _ in range(60):
            time.sleep(10)
            task_resp = _os_api(f"/_plugins/_ml/tasks/{task_id}")
            if not task_resp:
                continue
            task = json.loads(task_resp)
            state = task.get("state", "")
            if state == "COMPLETED":
                model_id = task.get("model_id", "")
                break
            if state == "FAILED":
                warn(f"ML model registration failed: {task_resp}")
                return

        if not model_id:
            warn("ML model registration timed out.")
            return

        # Deploy.
        ok("Deploying ML model...")
        _os_api(f"/_plugins/_ml/models/{model_id}/_deploy", "POST")
        for _ in range(30):
            time.sleep(5)
            r = _os_api(f"/_plugins/_ml/models/{model_id}")
            if r and '"DEPLOYED"' in r:
                break

    if not model_id:
        warn("No ML model available — skipping pipeline setup.")
        return

    # 3. Create/update ingest pipeline (PUT is idempotent).
    _os_api("/_ingest/pipeline/tuwunel_embedding_pipeline", "PUT", json.dumps({
        "description": "Tuwunel message embedding pipeline",
        "processors": [{"text_embedding": {
            "model_id": model_id,
            "field_map": {"body": "embedding"},
        }}],
    }))

    # 4. Create/update search pipeline (PUT is idempotent).
    _os_api("/_search/pipeline/tuwunel_hybrid_pipeline", "PUT", json.dumps({
        "description": "Tuwunel hybrid BM25+neural search pipeline",
        "phase_results_processors": [{"normalization-processor": {
            "normalization": {"technique": "min_max"},
            "combination": {"technique": "arithmetic_mean", "parameters": {"weights": [0.3, 0.7]}},
        }}],
    }))

    ok(f"OpenSearch ML ready (model: {model_id}).")
    return model_id


def _inject_opensearch_model_id():
    """Read deployed ML model_id from OpenSearch, write to ConfigMap in matrix ns.

    The tuwunel deployment reads TUWUNEL_SEARCH_OPENSEARCH_MODEL_ID from this
    ConfigMap. Creates or updates the ConfigMap idempotently.

    Reads the model_id from the ingest pipeline (which _ensure_opensearch_ml
    already configured with the correct model_id).
    """
    import json

    # Read model_id from the ingest pipeline that _ensure_opensearch_ml created.
    pipe_resp = _os_api("/_ingest/pipeline/tuwunel_embedding_pipeline")
    if not pipe_resp:
        warn("OpenSearch ingest pipeline not found — skipping model_id injection. "
             "Run 'sunbeam apply data' first.")
        return

    pipe = json.loads(pipe_resp)
    processors = (pipe.get("tuwunel_embedding_pipeline", {})
                      .get("processors", []))
    model_id = None
    for proc in processors:
        model_id = proc.get("text_embedding", {}).get("model_id")
        if model_id:
            break

    if not model_id:
        warn("No model_id in ingest pipeline — tuwunel hybrid search will be unavailable.")
        return

    # Check if ConfigMap already has this value.
    current = kube_out("get", "configmap", "opensearch-ml-config", "-n", "matrix",
                       "-o=jsonpath={.data.model_id}", "--ignore-not-found")
    if current == model_id:
        return

    cm = json.dumps({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {"name": "opensearch-ml-config", "namespace": "matrix"},
        "data": {"model_id": model_id},
    })
    kube("apply", "--server-side", "-f", "-", input=cm)
    ok(f"Injected OpenSearch model_id ({model_id}) into matrix/opensearch-ml-config.")


def cmd_apply(env: str = "local", domain: str = "", email: str = "", namespace: str = ""):
    """Build kustomize overlay for env, substitute domain/email, kubectl apply.

    Runs a second convergence pass if cert-manager is present in the overlay —
    cert-manager registers a ValidatingWebhook that must be running before
    ClusterIssuer / Certificate resources can be created.
    """
    # Fall back to config for ACME email if not provided via CLI flag.
    if not email:
        from sunbeam.config import load_config
        email = load_config().acme_email

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

    # Post-apply hooks for namespaces that need runtime patching.
    if not namespace or namespace == "matrix":
        _patch_tuwunel_oauth2_redirect(domain)
        _inject_opensearch_model_id()
    if not namespace or namespace == "data":
        _ensure_opensearch_ml()

    ok("Applied.")
