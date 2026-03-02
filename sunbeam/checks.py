"""Service-level health checks — functional probes beyond pod readiness."""
import base64
import json
import ssl
import subprocess
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from sunbeam.kube import get_domain, kube_exec, kube_out, parse_target
from sunbeam.output import ok, step, warn


@dataclass
class CheckResult:
    name: str
    ns: str
    svc: str
    passed: bool
    detail: str = ""


def _ssl_ctx() -> ssl.SSLContext:
    """Return an SSL context that trusts the mkcert local CA if available."""
    ctx = ssl.create_default_context()
    try:
        r = subprocess.run(["mkcert", "-CAROOT"], capture_output=True, text=True)
        if r.returncode == 0:
            ca_file = Path(r.stdout.strip()) / "rootCA.pem"
            if ca_file.exists():
                ctx.load_verify_locations(cafile=str(ca_file))
    except FileNotFoundError:
        pass
    return ctx


def _kube_secret(ns: str, name: str, key: str) -> str:
    """Read a base64-encoded K8s secret value and return the decoded string."""
    raw = kube_out("get", "secret", name, "-n", ns, f"-o=jsonpath={{.data.{key}}}")
    if not raw:
        return ""
    try:
        return base64.b64decode(raw + "==").decode()
    except Exception:
        return ""


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """Prevent urllib from following redirects so we can inspect the status code."""
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def _opener(ssl_ctx: ssl.SSLContext) -> urllib.request.OpenerDirector:
    return urllib.request.build_opener(
        _NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl_ctx),
    )


def _http_get(url: str, opener: urllib.request.OpenerDirector, *,
              headers: dict | None = None, timeout: int = 5) -> tuple[int, bytes]:
    """Return (status_code, body). Redirects are not followed.

    Any network/SSL error (including TimeoutError) is re-raised as URLError
    so callers only need to catch urllib.error.URLError.
    """
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with opener.open(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, b""
    except urllib.error.URLError:
        raise
    except OSError as e:
        # TimeoutError and other socket/SSL errors don't always get wrapped
        # in URLError by Python's urllib — normalize them here.
        raise urllib.error.URLError(e) from e


# ── Individual checks ─────────────────────────────────────────────────────────

def check_gitea_version(domain: str, opener) -> CheckResult:
    """GET /api/v1/version -> JSON with version field."""
    url = f"https://src.{domain}/api/v1/version"
    try:
        status, body = _http_get(url, opener)
        if status == 200:
            ver = json.loads(body).get("version", "?")
            return CheckResult("gitea-version", "devtools", "gitea", True, f"v{ver}")
        return CheckResult("gitea-version", "devtools", "gitea", False, f"HTTP {status}")
    except urllib.error.URLError as e:
        return CheckResult("gitea-version", "devtools", "gitea", False, str(e.reason))


def check_gitea_auth(domain: str, opener) -> CheckResult:
    """GET /api/v1/user with admin credentials -> 200 and login field."""
    username = _kube_secret("devtools", "gitea-admin-credentials", "admin-username") or "gitea_admin"
    password = _kube_secret("devtools", "gitea-admin-credentials", "admin-password")
    if not password:
        return CheckResult("gitea-auth", "devtools", "gitea", False,
                           "admin-password not found in secret")
    creds = base64.b64encode(f"{username}:{password}".encode()).decode()
    url = f"https://src.{domain}/api/v1/user"
    try:
        status, body = _http_get(url, opener, headers={"Authorization": f"Basic {creds}"})
        if status == 200:
            login = json.loads(body).get("login", "?")
            return CheckResult("gitea-auth", "devtools", "gitea", True, f"user={login}")
        return CheckResult("gitea-auth", "devtools", "gitea", False, f"HTTP {status}")
    except urllib.error.URLError as e:
        return CheckResult("gitea-auth", "devtools", "gitea", False, str(e.reason))


def check_postgres(domain: str, opener) -> CheckResult:
    """CNPG Cluster readyInstances == instances."""
    ready = kube_out("get", "cluster", "postgres", "-n", "data",
                     "-o=jsonpath={.status.readyInstances}")
    total = kube_out("get", "cluster", "postgres", "-n", "data",
                     "-o=jsonpath={.status.instances}")
    if ready and total and ready == total:
        return CheckResult("postgres", "data", "postgres", True, f"{ready}/{total} ready")
    detail = (f"{ready or '?'}/{total or '?'} ready"
              if (ready or total) else "cluster not found")
    return CheckResult("postgres", "data", "postgres", False, detail)


def check_valkey(domain: str, opener) -> CheckResult:
    """kubectl exec valkey pod -- valkey-cli ping -> PONG."""
    pod = kube_out("get", "pods", "-n", "data", "-l", "app=valkey",
                   "--no-headers", "-o=custom-columns=NAME:.metadata.name")
    pod = pod.splitlines()[0].strip() if pod else ""
    if not pod:
        return CheckResult("valkey", "data", "valkey", False, "no valkey pod")
    _, out = kube_exec("data", pod, "valkey-cli", "ping")
    return CheckResult("valkey", "data", "valkey", out == "PONG", out or "no response")


def check_openbao(domain: str, opener) -> CheckResult:
    """kubectl exec openbao-0 -- bao status -format=json -> initialized + unsealed."""
    rc, out = kube_exec("data", "openbao-0", "bao", "status", "-format=json")
    if not out:
        return CheckResult("openbao", "data", "openbao", False, "no response")
    try:
        data = json.loads(out)
        init = data.get("initialized", False)
        sealed = data.get("sealed", True)
        return CheckResult("openbao", "data", "openbao", init and not sealed,
                           f"init={init}, sealed={sealed}")
    except json.JSONDecodeError:
        return CheckResult("openbao", "data", "openbao", False, out[:80])


def check_seaweedfs(domain: str, opener) -> CheckResult:
    """GET https://s3.{domain}/ -> any response from the S3 API (< 500)."""
    url = f"https://s3.{domain}/"
    try:
        status, _ = _http_get(url, opener)
        # Unauthenticated S3 returns 403 (expected); 200 also ok; 5xx = problem.
        return CheckResult("seaweedfs", "storage", "seaweedfs", status < 500, f"HTTP {status}")
    except urllib.error.URLError as e:
        return CheckResult("seaweedfs", "storage", "seaweedfs", False, str(e.reason))


def check_kratos(domain: str, opener) -> CheckResult:
    """GET /kratos/health/ready -> 200."""
    url = f"https://auth.{domain}/kratos/health/ready"
    try:
        status, body = _http_get(url, opener)
        ok_flag = status == 200
        detail = f"HTTP {status}"
        if not ok_flag and body:
            detail += f": {body.decode(errors='replace')[:80]}"
        return CheckResult("kratos", "ory", "kratos", ok_flag, detail)
    except urllib.error.URLError as e:
        return CheckResult("kratos", "ory", "kratos", False, str(e.reason))


def check_hydra_oidc(domain: str, opener) -> CheckResult:
    """GET /.well-known/openid-configuration -> 200 with issuer field."""
    url = f"https://auth.{domain}/.well-known/openid-configuration"
    try:
        status, body = _http_get(url, opener)
        if status == 200:
            issuer = json.loads(body).get("issuer", "?")
            return CheckResult("hydra-oidc", "ory", "hydra", True, f"issuer={issuer}")
        return CheckResult("hydra-oidc", "ory", "hydra", False, f"HTTP {status}")
    except urllib.error.URLError as e:
        return CheckResult("hydra-oidc", "ory", "hydra", False, str(e.reason))


def check_people(domain: str, opener) -> CheckResult:
    """GET https://people.{domain}/ -> any response < 500 (302 to OIDC is fine)."""
    url = f"https://people.{domain}/"
    try:
        status, _ = _http_get(url, opener)
        return CheckResult("people", "lasuite", "people", status < 500, f"HTTP {status}")
    except urllib.error.URLError as e:
        return CheckResult("people", "lasuite", "people", False, str(e.reason))


def check_people_api(domain: str, opener) -> CheckResult:
    """GET /api/v1.0/config/ -> any response < 500 (401 auth-required is fine)."""
    url = f"https://people.{domain}/api/v1.0/config/"
    try:
        status, _ = _http_get(url, opener)
        return CheckResult("people-api", "lasuite", "people", status < 500, f"HTTP {status}")
    except urllib.error.URLError as e:
        return CheckResult("people-api", "lasuite", "people", False, str(e.reason))


def check_livekit(domain: str, opener) -> CheckResult:
    """kubectl exec livekit-server pod -- wget localhost:7880/ -> rc 0."""
    pod = kube_out("get", "pods", "-n", "media", "-l", "app.kubernetes.io/name=livekit-server",
                   "--no-headers", "-o=custom-columns=NAME:.metadata.name")
    pod = pod.splitlines()[0].strip() if pod else ""
    if not pod:
        return CheckResult("livekit", "media", "livekit", False, "no livekit pod")
    rc, _ = kube_exec("media", pod, "wget", "-qO-", "http://localhost:7880/")
    if rc == 0:
        return CheckResult("livekit", "media", "livekit", True, "server responding")
    return CheckResult("livekit", "media", "livekit", False, "server not responding")


# ── Check registry ────────────────────────────────────────────────────────────

CHECKS: list[tuple[Any, str, str]] = [
    (check_gitea_version, "devtools", "gitea"),
    (check_gitea_auth,    "devtools", "gitea"),
    (check_postgres,      "data",     "postgres"),
    (check_valkey,        "data",     "valkey"),
    (check_openbao,       "data",     "openbao"),
    (check_seaweedfs,     "storage",  "seaweedfs"),
    (check_kratos,        "ory",      "kratos"),
    (check_hydra_oidc,    "ory",      "hydra"),
    (check_people,        "lasuite",  "people"),
    (check_people_api,    "lasuite",  "people"),
    (check_livekit,       "media",    "livekit"),
]


def _run_one(fn, domain: str, op, ns: str, svc: str) -> CheckResult:
    try:
        return fn(domain, op)
    except Exception as e:
        return CheckResult(fn.__name__.replace("check_", ""), ns, svc, False, str(e)[:80])


def cmd_check(target: str | None) -> None:
    """Run service-level health checks, optionally scoped to a namespace or service."""
    step("Service health checks...")

    domain = get_domain()
    ssl_ctx = _ssl_ctx()
    op = _opener(ssl_ctx)

    ns_filter, svc_filter = parse_target(target) if target else (None, None)
    selected = [
        (fn, ns, svc) for fn, ns, svc in CHECKS
        if (ns_filter is None or ns == ns_filter)
        and (svc_filter is None or svc == svc_filter)
    ]

    if not selected:
        warn(f"No checks match target: {target}")
        return

    # Run all checks concurrently — total time ≈ slowest single check.
    with ThreadPoolExecutor(max_workers=len(selected)) as pool:
        futures = [pool.submit(_run_one, fn, domain, op, ns, svc)
                   for fn, ns, svc in selected]
        results = [f.result() for f in futures]

    # Print grouped by namespace (mirrors sunbeam status layout).
    name_w = max(len(r.name) for r in results)
    cur_ns = None
    for r in results:
        if r.ns != cur_ns:
            print(f"  {r.ns}:")
            cur_ns = r.ns
        icon = "\u2713" if r.passed else "\u2717"
        detail = f"  {r.detail}" if r.detail else ""
        print(f"    {icon} {r.name:<{name_w}}{detail}")

    print()
    failed = [r for r in results if not r.passed]
    if failed:
        warn(f"{len(failed)} check(s) failed.")
    else:
        ok(f"All {len(results)} check(s) passed.")
