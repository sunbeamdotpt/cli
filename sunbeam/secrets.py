"""Secrets management — OpenBao KV seeding, DB engine config, VSO verification."""
import base64
import json
import secrets as _secrets
import subprocess
import time
import urllib.error
import urllib.request
from contextlib import contextmanager
from pathlib import Path

from sunbeam.kube import kube, kube_out, kube_ok, kube_apply, ensure_ns, create_secret, get_domain, context_arg
from sunbeam.output import step, ok, warn, die

ADMIN_USERNAME = "estudio-admin"


def _gen_fernet_key() -> str:
    """Generate a Fernet-compatible key (32 random bytes, URL-safe base64)."""
    return base64.urlsafe_b64encode(_secrets.token_bytes(32)).decode()


def _gen_dkim_key_pair() -> tuple[str, str]:
    """Generate an RSA 2048-bit DKIM key pair using openssl.

    Returns (private_pem_pkcs8, public_pem). Returns ("", "") on failure.
    """
    try:
        r1 = subprocess.run(["openssl", "genrsa", "2048"], capture_output=True, text=True)
        if r1.returncode != 0:
            warn(f"openssl genrsa failed: {r1.stderr.strip()}")
            return ("", "")
        # Convert to PKCS8 (format expected by rspamd)
        r2 = subprocess.run(["openssl", "pkcs8", "-topk8", "-nocrypt"],
                            input=r1.stdout, capture_output=True, text=True)
        private_pem = r2.stdout.strip() if r2.returncode == 0 else r1.stdout.strip()
        # Extract public key from the original RSA key
        r3 = subprocess.run(["openssl", "rsa", "-pubout"],
                            input=r1.stdout, capture_output=True, text=True)
        if r3.returncode != 0:
            warn(f"openssl rsa -pubout failed: {r3.stderr.strip()}")
            return (private_pem, "")
        return (private_pem, r3.stdout.strip())
    except FileNotFoundError:
        warn("openssl not found -- skipping DKIM key generation.")
        return ("", "")

LIMA_VM = "sunbeam"
GITEA_ADMIN_USER = "gitea_admin"
PG_USERS = [
    "kratos", "hydra", "gitea", "hive",
    "docs", "meet", "drive", "messages", "conversations",
    "people", "find", "calendars", "projects",
]


# ---------------------------------------------------------------------------
# OpenBao KV seeding
# ---------------------------------------------------------------------------

def _seed_openbao() -> dict:
    """Initialize/unseal OpenBao, generate/read credentials idempotently, configure VSO auth.

    Returns a dict of all generated credentials. Values are read from existing
    OpenBao KV entries when present -- re-running never rotates credentials.
    """
    ob_pod = kube_out(
        "-n", "data", "get", "pods",
        "-l=app.kubernetes.io/name=openbao,component=server",
        "-o=jsonpath={.items[0].metadata.name}",
    )
    if not ob_pod:
        ok("OpenBao pod not found -- skipping.")
        return {}

    ok(f"OpenBao ({ob_pod})...")
    kube("wait", "-n", "data", f"pod/{ob_pod}",
         "--for=jsonpath={.status.phase}=Running", "--timeout=120s", check=False)

    def bao(cmd):
        r = subprocess.run(
            ["kubectl", context_arg(), "-n", "data", "exec", ob_pod, "-c", "openbao",
             "--", "sh", "-c", cmd],
            capture_output=True, text=True,
        )
        return r.stdout.strip()

    def bao_status():
        out = bao("bao status -format=json 2>/dev/null || echo '{}'")
        try:
            return json.loads(out)
        except json.JSONDecodeError:
            return {}

    unseal_key = ""
    root_token = ""

    status = bao_status()
    already_initialized = status.get("initialized", False)
    if not already_initialized:
        existing_key = kube_out("-n", "data", "get", "secret", "openbao-keys",
                                "-o=jsonpath={.data.key}")
        already_initialized = bool(existing_key)

    if not already_initialized:
        ok("Initializing OpenBao...")
        init_json = bao("bao operator init -key-shares=1 -key-threshold=1 -format=json 2>/dev/null || echo '{}'")
        try:
            init = json.loads(init_json)
            unseal_key = init["unseal_keys_b64"][0]
            root_token = init["root_token"]
            create_secret("data", "openbao-keys",
                          key=unseal_key, **{"root-token": root_token})
            ok("Initialized -- keys stored in secret/openbao-keys.")
        except (json.JSONDecodeError, KeyError):
            warn("Init failed -- resetting OpenBao storage for local dev...")
            kube("delete", "pvc", "data-openbao-0", "-n", "data", "--ignore-not-found", check=False)
            kube("delete", "pod", ob_pod, "-n", "data", "--ignore-not-found", check=False)
            warn("OpenBao storage reset. Run --seed again after the pod restarts.")
            return {}
    else:
        ok("Already initialized.")
        existing_key = kube_out("-n", "data", "get", "secret", "openbao-keys",
                                "-o=jsonpath={.data.key}")
        if existing_key:
            unseal_key = base64.b64decode(existing_key).decode()
        root_token_enc = kube_out("-n", "data", "get", "secret", "openbao-keys",
                                  "-o=jsonpath={.data.root-token}")
        if root_token_enc:
            root_token = base64.b64decode(root_token_enc).decode()

    if bao_status().get("sealed", False) and unseal_key:
        ok("Unsealing...")
        bao(f"bao operator unseal '{unseal_key}' 2>/dev/null")

    if not root_token:
        warn("No root token available -- skipping KV seeding.")
        return {}

    # Read-or-generate helper: preserves existing KV values; only generates missing ones.
    # Tracks which paths had new values so we only write back when necessary.
    _dirty_paths: set = set()

    def get_or_create(path, **fields):
        raw = bao(
            f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
            f"bao kv get -format=json secret/{path} 2>/dev/null || echo '{{}}'"
        )
        existing = {}
        try:
            existing = json.loads(raw).get("data", {}).get("data", {})
        except (json.JSONDecodeError, AttributeError):
            pass
        result = {}
        for key, default_fn in fields.items():
            val = existing.get(key)
            if val:
                result[key] = val
            else:
                result[key] = default_fn()
                _dirty_paths.add(path)
        return result

    def rand():
        return _secrets.token_urlsafe(32)

    ok("Seeding KV (idempotent -- existing values preserved)...")

    bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
        f"bao secrets enable -path=secret -version=2 kv 2>/dev/null || true")

    # DB passwords removed -- OpenBao database secrets engine manages them via static roles.
    hydra = get_or_create("hydra",
        **{"system-secret": rand,
           "cookie-secret": rand,
           "pairwise-salt": rand})

    SMTP_URI = "smtp://postfix.lasuite.svc.cluster.local:25/?skip_ssl_verify=true"
    kratos = get_or_create("kratos",
        **{"secrets-default":       rand,
           "secrets-cookie":        rand,
           "smtp-connection-uri":   lambda: SMTP_URI})

    seaweedfs = get_or_create("seaweedfs",
        **{"access-key": rand, "secret-key": rand})

    gitea = get_or_create("gitea",
        **{"admin-username": lambda: GITEA_ADMIN_USER,
           "admin-password": rand})

    hive = get_or_create("hive",
        **{"oidc-client-id":    lambda: "hive-local",
           "oidc-client-secret": rand})

    livekit = get_or_create("livekit",
        **{"api-key":    lambda: "devkey",
           "api-secret": rand})

    people = get_or_create("people",
        **{"django-secret-key": rand})

    login_ui = get_or_create("login-ui",
        **{"cookie-secret":      rand,
           "csrf-cookie-secret": rand})

    kratos_admin = get_or_create("kratos-admin",
        **{"cookie-secret":      rand,
           "csrf-cookie-secret": rand,
           "admin-identity-ids": lambda: "",
           "s3-access-key":      lambda: seaweedfs["access-key"],
           "s3-secret-key":      lambda: seaweedfs["secret-key"]})

    docs = get_or_create("docs",
        **{"django-secret-key":      rand,
           "collaboration-secret":   rand})

    meet = get_or_create("meet",
        **{"django-secret-key":            rand,
           "application-jwt-secret-key":   rand})

    drive = get_or_create("drive",
        **{"django-secret-key": rand})

    projects = get_or_create("projects",
        **{"secret-key": rand})

    calendars = get_or_create("calendars",
        **{"django-secret-key":       lambda: _secrets.token_urlsafe(50),
           "salt-key":                rand,
           "caldav-inbound-api-key":  rand,
           "caldav-outbound-api-key": rand,
           "caldav-internal-api-key": rand})

    # DKIM key pair -- generated together since private and public keys are coupled.
    # Read existing keys first; only generate a new pair when absent.
    existing_messages_raw = bao(
        f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
        f"bao kv get -format=json secret/messages 2>/dev/null || echo '{{}}'"
    )
    existing_messages = {}
    try:
        existing_messages = json.loads(existing_messages_raw).get("data", {}).get("data", {})
    except (json.JSONDecodeError, AttributeError):
        pass

    if existing_messages.get("dkim-private-key"):
        _dkim_private = existing_messages["dkim-private-key"]
        _dkim_public = existing_messages.get("dkim-public-key", "")
    else:
        _dkim_private, _dkim_public = _gen_dkim_key_pair()

    messages = get_or_create("messages",
        **{"django-secret-key":        rand,
           "salt-key":                 rand,
           "mda-api-secret":           rand,
           "oidc-refresh-token-key":   _gen_fernet_key,
           "dkim-private-key":         lambda: _dkim_private,
           "dkim-public-key":          lambda: _dkim_public,
           "rspamd-password":          rand,
           "socks-proxy-users":        lambda: f"sunbeam:{rand()}",
           "mta-out-smtp-username":    lambda: "sunbeam",
           "mta-out-smtp-password":    rand})

    collabora = get_or_create("collabora",
        **{"username": lambda: "admin",
           "password": rand})

    tuwunel = get_or_create("tuwunel",
        **{"oidc-client-id":      lambda: "",
           "oidc-client-secret":  lambda: "",
           "turn-secret":         lambda: "",
           "registration-token":  rand})

    # Scaleway S3 credentials for CNPG barman backups.
    # Read from `scw config` at seed time; falls back to empty string (operator must fill in).
    def _scw_config(key):
        try:
            r = subprocess.run(["scw", "config", "get", key],
                               capture_output=True, text=True, timeout=5)
            return r.stdout.strip() if r.returncode == 0 else ""
        except (FileNotFoundError, subprocess.TimeoutExpired):
            return ""

    grafana = get_or_create("grafana",
        **{"admin-password": rand})

    scaleway_s3 = get_or_create("scaleway-s3",
        **{"access-key-id":     lambda: _scw_config("access-key"),
           "secret-access-key": lambda: _scw_config("secret-key")})

    # Only write secrets to OpenBao KV for paths that have new/missing values.
    # This avoids unnecessary KV version bumps which trigger VSO re-syncs and
    # rollout restarts across the cluster.
    if not _dirty_paths:
        ok("All OpenBao KV secrets already present -- skipping writes.")
    else:
        ok(f"Writing new secrets to OpenBao KV ({', '.join(sorted(_dirty_paths))})...")

        def _kv_put(path, **kv):
            pairs = " ".join(f'{k}="{v}"' for k, v in kv.items())
            bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
                f"bao kv put secret/{path} {pairs}")

        if "messages" in _dirty_paths:
            _kv_put("messages",
                **{"django-secret-key":      messages["django-secret-key"],
                   "salt-key":               messages["salt-key"],
                   "mda-api-secret":         messages["mda-api-secret"],
                   "oidc-refresh-token-key": messages["oidc-refresh-token-key"],
                   "rspamd-password":        messages["rspamd-password"],
                   "socks-proxy-users":      messages["socks-proxy-users"],
                   "mta-out-smtp-username":  messages["mta-out-smtp-username"],
                   "mta-out-smtp-password":  messages["mta-out-smtp-password"]})
            # DKIM keys stored separately (large PEM values)
            dkim_priv_b64 = base64.b64encode(messages['dkim-private-key'].encode()).decode()
            dkim_pub_b64  = base64.b64encode(messages['dkim-public-key'].encode()).decode()
            bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' sh -c '"
                f"echo {dkim_priv_b64} | base64 -d > /tmp/dkim_priv.pem && "
                f"echo {dkim_pub_b64}  | base64 -d > /tmp/dkim_pub.pem && "
                f"bao kv patch secret/messages"
                f" dkim-private-key=\"$(cat /tmp/dkim_priv.pem)\""
                f" dkim-public-key=\"$(cat /tmp/dkim_pub.pem)\" && "
                f"rm /tmp/dkim_priv.pem /tmp/dkim_pub.pem"
                f"'")
        if "hydra" in _dirty_paths:
            _kv_put("hydra", **{"system-secret": hydra["system-secret"],
                                "cookie-secret": hydra["cookie-secret"],
                                "pairwise-salt": hydra["pairwise-salt"]})
        if "kratos" in _dirty_paths:
            _kv_put("kratos", **{"secrets-default": kratos["secrets-default"],
                                 "secrets-cookie": kratos["secrets-cookie"],
                                 "smtp-connection-uri": kratos["smtp-connection-uri"]})
        if "gitea" in _dirty_paths:
            _kv_put("gitea", **{"admin-username": gitea["admin-username"],
                                "admin-password": gitea["admin-password"]})
        if "seaweedfs" in _dirty_paths:
            _kv_put("seaweedfs", **{"access-key": seaweedfs["access-key"],
                                    "secret-key": seaweedfs["secret-key"]})
        if "hive" in _dirty_paths:
            _kv_put("hive", **{"oidc-client-id": hive["oidc-client-id"],
                               "oidc-client-secret": hive["oidc-client-secret"]})
        if "livekit" in _dirty_paths:
            _kv_put("livekit", **{"api-key": livekit["api-key"],
                                  "api-secret": livekit["api-secret"]})
        if "people" in _dirty_paths:
            _kv_put("people", **{"django-secret-key": people["django-secret-key"]})
        if "login-ui" in _dirty_paths:
            _kv_put("login-ui", **{"cookie-secret": login_ui["cookie-secret"],
                                   "csrf-cookie-secret": login_ui["csrf-cookie-secret"]})
        if "kratos-admin" in _dirty_paths:
            _kv_put("kratos-admin", **{"cookie-secret": kratos_admin["cookie-secret"],
                                       "csrf-cookie-secret": kratos_admin["csrf-cookie-secret"],
                                       "admin-identity-ids": kratos_admin["admin-identity-ids"],
                                       "s3-access-key": kratos_admin["s3-access-key"],
                                       "s3-secret-key": kratos_admin["s3-secret-key"]})
        if "docs" in _dirty_paths:
            _kv_put("docs", **{"django-secret-key": docs["django-secret-key"],
                               "collaboration-secret": docs["collaboration-secret"]})
        if "meet" in _dirty_paths:
            _kv_put("meet", **{"django-secret-key": meet["django-secret-key"],
                               "application-jwt-secret-key": meet["application-jwt-secret-key"]})
        if "drive" in _dirty_paths:
            _kv_put("drive", **{"django-secret-key": drive["django-secret-key"]})
        if "projects" in _dirty_paths:
            _kv_put("projects", **{"secret-key": projects["secret-key"]})
        if "calendars" in _dirty_paths:
            _kv_put("calendars", **{"django-secret-key": calendars["django-secret-key"],
                                    "salt-key": calendars["salt-key"],
                                    "caldav-inbound-api-key": calendars["caldav-inbound-api-key"],
                                    "caldav-outbound-api-key": calendars["caldav-outbound-api-key"],
                                    "caldav-internal-api-key": calendars["caldav-internal-api-key"]})
        if "collabora" in _dirty_paths:
            _kv_put("collabora", **{"username": collabora["username"],
                                    "password": collabora["password"]})
        if "grafana" in _dirty_paths:
            _kv_put("grafana", **{"admin-password": grafana["admin-password"]})
        if "scaleway-s3" in _dirty_paths:
            _kv_put("scaleway-s3", **{"access-key-id": scaleway_s3["access-key-id"],
                                      "secret-access-key": scaleway_s3["secret-access-key"]})
        if "tuwunel" in _dirty_paths:
            _kv_put("tuwunel", **{"oidc-client-id": tuwunel["oidc-client-id"],
                                  "oidc-client-secret": tuwunel["oidc-client-secret"],
                                  "turn-secret": tuwunel["turn-secret"],
                                  "registration-token": tuwunel["registration-token"]})

    # Configure Kubernetes auth method so VSO can authenticate with OpenBao
    ok("Configuring Kubernetes auth for VSO...")
    bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
        f"bao auth enable kubernetes 2>/dev/null; true")
    bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
        f"bao write auth/kubernetes/config "
        f"kubernetes_host=https://kubernetes.default.svc.cluster.local")

    policy_hcl = (
        'path "secret/data/*" { capabilities = ["read"] }\n'
        'path "secret/metadata/*" { capabilities = ["read", "list"] }\n'
        'path "database/static-creds/*" { capabilities = ["read"] }\n'
    )
    policy_b64 = base64.b64encode(policy_hcl.encode()).decode()
    bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
        f"sh -c 'echo {policy_b64} | base64 -d | bao policy write vso-reader -'")

    bao(f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}' "
        f"bao write auth/kubernetes/role/vso "
        f"bound_service_account_names=default "
        f"bound_service_account_namespaces=ory,devtools,storage,lasuite,matrix,media,data,monitoring "
        f"policies=vso-reader "
        f"ttl=1h")

    return {
        "hydra-system-secret":     hydra["system-secret"],
        "hydra-cookie-secret":     hydra["cookie-secret"],
        "hydra-pairwise-salt":     hydra["pairwise-salt"],
        "kratos-secrets-default":  kratos["secrets-default"],
        "kratos-secrets-cookie":   kratos["secrets-cookie"],
        "s3-access-key":           seaweedfs["access-key"],
        "s3-secret-key":           seaweedfs["secret-key"],
        "gitea-admin-password":    gitea["admin-password"],
        "hive-oidc-client-id":     hive["oidc-client-id"],
        "hive-oidc-client-secret": hive["oidc-client-secret"],
        "people-django-secret":    people["django-secret-key"],
        "livekit-api-key":         livekit["api-key"],
        "livekit-api-secret":      livekit["api-secret"],
        "kratos-admin-cookie-secret": kratos_admin["cookie-secret"],
        "messages-dkim-public-key": messages.get("dkim-public-key", ""),
        "_ob_pod":                 ob_pod,
        "_root_token":             root_token,
    }


# ---------------------------------------------------------------------------
# Database secrets engine
# ---------------------------------------------------------------------------

def _configure_db_engine(ob_pod, root_token, pg_user, pg_pass):
    """Enable OpenBao database secrets engine and create PostgreSQL static roles.

    Static roles cause OpenBao to immediately set (and later rotate) each service
    user's password via ALTER USER, eliminating hardcoded DB passwords.
    Idempotent: bao write overwrites existing config/roles safely.

    The `vault` PG user is created here (if absent) and used as the DB engine
    connection user. pg_user/pg_pass (the CNPG superuser) are kept for potential
    future use but are no longer used for the connection URL.
    """
    ok("Configuring OpenBao database secrets engine...")
    pg_rw = "postgres-rw.data.svc.cluster.local:5432"
    bao_env = f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}'"

    def bao(cmd, check=True):
        r = subprocess.run(
            ["kubectl", context_arg(), "-n", "data", "exec", ob_pod, "-c", "openbao",
             "--", "sh", "-c", cmd],
            capture_output=True, text=True,
        )
        if check and r.returncode != 0:
            raise RuntimeError(f"bao command failed (exit {r.returncode}):\n{r.stderr.strip()}")
        return r.stdout.strip()

    # Enable database secrets engine -- tolerate "already enabled" error via || true.
    bao(f"{bao_env} bao secrets enable database 2>/dev/null || true", check=False)

    # -- vault PG user setup ---------------------------------------------------
    # Locate the CNPG primary pod for psql exec (peer auth -- no password needed).
    cnpg_pod = kube_out(
        "-n", "data", "get", "pods",
        "-l=cnpg.io/cluster=postgres,role=primary",
        "-o=jsonpath={.items[0].metadata.name}",
    )
    if not cnpg_pod:
        raise RuntimeError("Could not find CNPG primary pod for vault user setup.")

    def psql(sql):
        r = subprocess.run(
            ["kubectl", context_arg(), "-n", "data", "exec", cnpg_pod, "-c", "postgres",
             "--", "psql", "-U", "postgres", "-c", sql],
            capture_output=True, text=True,
        )
        if r.returncode != 0:
            raise RuntimeError(f"psql failed: {r.stderr.strip()}")
        return r.stdout.strip()

    # Read existing vault pg-password from OpenBao KV, or generate a new one.
    existing_vault_pass = bao(
        f"{bao_env} bao kv get -field=pg-password secret/vault 2>/dev/null || true",
        check=False,
    )
    vault_pg_pass = existing_vault_pass.strip() if existing_vault_pass.strip() else _secrets.token_urlsafe(32)

    # Store vault pg-password in OpenBao KV (idempotent).
    bao(f"{bao_env} bao kv put secret/vault pg-password=\"{vault_pg_pass}\"")
    ok("vault KV entry written.")

    # Create vault PG user if absent, set its password, grant ADMIN OPTION on all service users.
    create_vault_sql = (
        f"DO $$ BEGIN "
        f"IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'vault') THEN "
        f"CREATE USER vault WITH LOGIN CREATEROLE; "
        f"END IF; "
        f"END $$;"
    )
    psql(create_vault_sql)
    psql(f"ALTER USER vault WITH PASSWORD '{vault_pg_pass}';")
    for user in PG_USERS:
        psql(f"GRANT {user} TO vault WITH ADMIN OPTION;")
    ok("vault PG user configured with ADMIN OPTION on all service roles.")

    # -- DB engine connection config (uses vault user) -------------------------
    conn_url = (
        "postgresql://{{username}}:{{password}}"
        f"@{pg_rw}/postgres?sslmode=disable"
    )
    bao(
        f"{bao_env} bao write database/config/cnpg-postgres"
        f" plugin_name=postgresql-database-plugin"
        f" allowed_roles='*'"
        f" connection_url='{conn_url}'"
        f" username='vault'"
        f" password='{vault_pg_pass}'"
    )
    ok("DB engine connection configured (vault user).")

    # Encode the rotation statement to avoid shell quoting issues with inner quotes.
    rotation_b64 = base64.b64encode(
        b"ALTER USER \"{{name}}\" WITH PASSWORD '{{password}}';"
    ).decode()

    for user in PG_USERS:
        bao(
            f"{bao_env} sh -c '"
            f"bao write database/static-roles/{user}"
            f" db_name=cnpg-postgres"
            f" username={user}"
            f" rotation_period=86400"
            f" \"rotation_statements=$(echo {rotation_b64} | base64 -d)\"'"
        )
        ok(f"  static-role/{user}")

    ok("Database secrets engine configured.")


# ---------------------------------------------------------------------------
# cmd_seed — main entry point
# ---------------------------------------------------------------------------

@contextmanager
def _kratos_admin_pf(local_port=14434):
    """Port-forward directly to the Kratos admin API."""
    proc = subprocess.Popen(
        ["kubectl", context_arg(), "-n", "ory", "port-forward",
         "svc/kratos-admin", f"{local_port}:80"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    time.sleep(1.5)
    try:
        yield f"http://localhost:{local_port}"
    finally:
        proc.terminate()
        proc.wait()


def _kratos_api(base, path, method="GET", body=None):
    url = f"{base}/admin{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        url, data=data,
        headers={"Content-Type": "application/json", "Accept": "application/json"},
        method=method,
    )
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"Kratos API {method} {url} → {e.code}: {e.read().decode()}")


def _seed_kratos_admin_identity(ob_pod: str, root_token: str) -> tuple[str, str]:
    """Ensure estudio-admin@<domain> exists in Kratos and is the only admin identity.

    Returns (recovery_link, recovery_code), or ("", "") if Kratos is unreachable.
    Idempotent: if the identity already exists, skips creation and just returns
    a fresh recovery link+code.
    """
    domain = get_domain()
    admin_email = f"{ADMIN_USERNAME}@{domain}"

    ok(f"Ensuring Kratos admin identity ({admin_email})...")
    try:
        with _kratos_admin_pf() as base:
            # Check if the identity already exists by searching by email
            result = _kratos_api(base, f"/identities?credentials_identifier={admin_email}&page_size=1")
            existing = result[0] if isinstance(result, list) and result else None

            if existing:
                identity_id = existing["id"]
                ok(f"  admin identity exists ({identity_id[:8]}...)")
            else:
                identity = _kratos_api(base, "/identities", method="POST", body={
                    "schema_id": "employee",
                    "traits": {"email": admin_email},
                    "state": "active",
                })
                identity_id = identity["id"]
                ok(f"  created admin identity ({identity_id[:8]}...)")

            # Generate fresh recovery code + link
            recovery = _kratos_api(base, "/recovery/code", method="POST", body={
                "identity_id": identity_id,
                "expires_in": "24h",
            })
            recovery_link = recovery.get("recovery_link", "") if recovery else ""
            recovery_code = recovery.get("recovery_code", "") if recovery else ""
    except Exception as exc:
        warn(f"Could not seed Kratos admin identity (Kratos may not be ready): {exc}")
        return ("", "")

    # Update admin-identity-ids in OpenBao KV so kratos-admin-ui enforces access
    bao_env = f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}'"

    def _bao(cmd):
        return subprocess.run(
            ["kubectl", context_arg(), "-n", "data", "exec", ob_pod, "-c", "openbao",
             "--", "sh", "-c", cmd],
            capture_output=True, text=True,
        )

    _bao(f"{bao_env} bao kv patch secret/kratos-admin admin-identity-ids=\"{admin_email}\"")
    ok(f"  ADMIN_IDENTITY_IDS set to {admin_email}")
    return (recovery_link, recovery_code)


def cmd_seed() -> dict:
    """Seed OpenBao KV with crypto-random credentials, then mirror to K8s Secrets.

    Returns a dict of credentials for use by callers (gitea admin pass, etc.).
    Idempotent: reads existing OpenBao values before generating; never rotates.
    """
    step("Seeding secrets...")

    creds = _seed_openbao()

    ob_pod     = creds.pop("_ob_pod", "")
    root_token = creds.pop("_root_token", "")

    s3_access_key        = creds.get("s3-access-key", "")
    s3_secret_key        = creds.get("s3-secret-key", "")
    hydra_system         = creds.get("hydra-system-secret", "")
    hydra_cookie         = creds.get("hydra-cookie-secret", "")
    hydra_pairwise       = creds.get("hydra-pairwise-salt", "")
    kratos_secrets_default = creds.get("kratos-secrets-default", "")
    kratos_secrets_cookie  = creds.get("kratos-secrets-cookie", "")
    hive_oidc_id         = creds.get("hive-oidc-client-id", "hive-local")
    hive_oidc_sec        = creds.get("hive-oidc-client-secret", "")
    django_secret        = creds.get("people-django-secret", "")
    gitea_admin_pass     = creds.get("gitea-admin-password", "")

    ok("Waiting for postgres cluster...")
    pg_pod = ""
    for _ in range(60):
        phase = kube_out("-n", "data", "get", "cluster", "postgres",
                         "-o=jsonpath={.status.phase}")
        if phase == "Cluster in healthy state":
            pg_pod = kube_out("-n", "data", "get", "pods",
                              "-l=cnpg.io/cluster=postgres,role=primary",
                              "-o=jsonpath={.items[0].metadata.name}")
            ok(f"Postgres ready ({pg_pod}).")
            break
        time.sleep(5)
    else:
        warn("Postgres not ready after 5 min -- continuing anyway.")

    if pg_pod:
        ok("Ensuring postgres roles and databases exist...")
        db_map = {
            "kratos": "kratos_db", "hydra": "hydra_db", "gitea": "gitea_db",
            "hive": "hive_db", "docs": "docs_db", "meet": "meet_db",
            "drive": "drive_db", "messages": "messages_db",
            "conversations": "conversations_db",
            "people": "people_db", "find": "find_db",
            "calendars": "calendars_db", "projects": "projects_db",
        }
        for user in PG_USERS:
            # Only CREATE if missing -- passwords are managed by OpenBao static roles.
            ensure_sql = (
                f"DO $$ BEGIN "
                f"IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='{user}') "
                f"THEN EXECUTE 'CREATE USER {user}'; END IF; END $$;"
            )
            kube("exec", "-n", "data", pg_pod, "-c", "postgres", "--",
                 "psql", "-U", "postgres", "-c", ensure_sql, check=False)
            db = db_map.get(user, f"{user}_db")
            kube("exec", "-n", "data", pg_pod, "-c", "postgres", "--",
                 "psql", "-U", "postgres", "-c",
                 f"CREATE DATABASE {db} OWNER {user};", check=False)

        # Read CNPG superuser credentials and configure database secrets engine.
        # CNPG creates secret named "{cluster}-app" (not "{cluster}-superuser")
        # when owner is specified without an explicit secret field.
        pg_user_b64 = kube_out("-n", "data", "get", "secret", "postgres-app",
                               "-o=jsonpath={.data.username}")
        pg_pass_b64 = kube_out("-n", "data", "get", "secret", "postgres-app",
                               "-o=jsonpath={.data.password}")
        pg_user = base64.b64decode(pg_user_b64).decode() if pg_user_b64 else "postgres"
        pg_pass = base64.b64decode(pg_pass_b64).decode() if pg_pass_b64 else ""

        if ob_pod and root_token and pg_pass:
            try:
                _configure_db_engine(ob_pod, root_token, pg_user, pg_pass)
            except Exception as exc:
                warn(f"DB engine config failed: {exc}")
        else:
            warn("Skipping DB engine config -- missing ob_pod, root_token, or pg_pass.")

    ok("Creating K8s secrets (VSO will overwrite on next sync)...")

    ensure_ns("ory")
    # Hydra app secrets -- DSN comes from VaultDynamicSecret hydra-db-creds.
    create_secret("ory", "hydra",
        secretsSystem=hydra_system,
        secretsCookie=hydra_cookie,
        **{"pairwise-salt": hydra_pairwise},
    )
    # Kratos non-rotating encryption keys -- DSN comes from VaultDynamicSecret kratos-db-creds.
    create_secret("ory", "kratos-app-secrets",
        secretsDefault=kratos_secrets_default,
        secretsCookie=kratos_secrets_cookie,
    )

    ensure_ns("devtools")
    # gitea-db-credentials comes from VaultDynamicSecret (static-creds/gitea).
    create_secret("devtools", "gitea-s3-credentials",
                  **{"access-key": s3_access_key, "secret-key": s3_secret_key})
    create_secret("devtools", "gitea-admin-credentials",
                  username=GITEA_ADMIN_USER, password=gitea_admin_pass)

    # Sync Gitea admin password to Gitea's own DB (Gitea's existingSecret only
    # applies on first run — subsequent K8s secret updates are not picked up
    # automatically by Gitea).
    if gitea_admin_pass:
        gitea_pod = kube_out(
            "-n", "devtools", "get", "pods",
            "-l=app.kubernetes.io/name=gitea",
            "-o=jsonpath={.items[0].metadata.name}",
        )
        if gitea_pod:
            r = subprocess.run(
                ["kubectl", context_arg(), "-n", "devtools", "exec", gitea_pod,
                 "--", "gitea", "admin", "user", "change-password",
                 "--username", GITEA_ADMIN_USER, "--password", gitea_admin_pass,
                 "--must-change-password=false"],
                capture_output=True, text=True,
            )
            if r.returncode == 0:
                ok(f"Gitea admin password synced to Gitea DB.")
            else:
                warn(f"Could not sync Gitea admin password: {r.stderr.strip()}")
        else:
            warn("Gitea pod not found — admin password NOT synced to Gitea DB. Run seed again after Gitea is deployed.")

    ensure_ns("storage")
    s3_json = (
        '{"identities":[{"name":"seaweed","credentials":[{"accessKey":"'
        + s3_access_key + '","secretKey":"' + s3_secret_key
        + '"}],"actions":["Admin","Read","Write","List","Tagging"]}]}'
    )
    create_secret("storage", "seaweedfs-s3-credentials",
                  S3_ACCESS_KEY=s3_access_key, S3_SECRET_KEY=s3_secret_key)
    create_secret("storage", "seaweedfs-s3-json", **{"s3.json": s3_json})

    ensure_ns("lasuite")
    create_secret("lasuite", "seaweedfs-s3-credentials",
                  S3_ACCESS_KEY=s3_access_key, S3_SECRET_KEY=s3_secret_key)
    # hive-db-url and people-db-credentials come from VaultDynamicSecrets.
    create_secret("lasuite", "hive-oidc",
                  **{"client-id": hive_oidc_id, "client-secret": hive_oidc_sec})
    create_secret("lasuite", "people-django-secret",
                  DJANGO_SECRET_KEY=django_secret)

    ensure_ns("matrix")

    ensure_ns("media")
    ensure_ns("monitoring")

    # Ensure the Kratos admin identity exists and ADMIN_IDENTITY_IDS is set.
    # This runs after all other secrets are in place (Kratos must be up).
    recovery_link, recovery_code = _seed_kratos_admin_identity(ob_pod, root_token)
    if recovery_link:
        ok("Admin recovery link (valid 24h):")
        print(f"  {recovery_link}")
    if recovery_code:
        ok("Admin recovery code (enter on the page above):")
        print(f"  {recovery_code}")

    dkim_pub = creds.get("messages-dkim-public-key", "")
    if dkim_pub:
        b64_key = "".join(
            dkim_pub.replace("-----BEGIN PUBLIC KEY-----", "")
                     .replace("-----END PUBLIC KEY-----", "")
                     .split()
        )
        domain = get_domain()
        ok("DKIM DNS record (add to DNS at your registrar):")
        print(f"  default._domainkey.{domain}  TXT  \"v=DKIM1; k=rsa; p={b64_key}\"")

    ok("All secrets seeded.")
    return creds


# ---------------------------------------------------------------------------
# cmd_verify — VSO E2E verification
# ---------------------------------------------------------------------------

def cmd_verify():
    """End-to-end test of VSO -> OpenBao integration.

    1. Writes a random value to OpenBao KV at secret/vso-test.
    2. Creates a VaultAuth + VaultStaticSecret in the 'ory' namespace
       (already bound to the 'vso' Kubernetes auth role).
    3. Polls until VSO syncs the K8s Secret (up to 60s).
    4. Reads and base64-decodes the K8s Secret; compares to the expected value.
    5. Cleans up all test resources in a finally block.
    """
    step("Verifying VSO -> OpenBao integration (E2E)...")

    ob_pod = kube_out(
        "-n", "data", "get", "pods",
        "-l=app.kubernetes.io/name=openbao,component=server",
        "-o=jsonpath={.items[0].metadata.name}",
    )
    if not ob_pod:
        die("OpenBao pod not found -- run full bring-up first.")

    root_token_enc = kube_out(
        "-n", "data", "get", "secret", "openbao-keys",
        "-o=jsonpath={.data.root-token}",
    )
    if not root_token_enc:
        die("Could not read openbao-keys secret.")
    root_token = base64.b64decode(root_token_enc).decode()

    bao_env = f"BAO_ADDR=http://127.0.0.1:8200 BAO_TOKEN='{root_token}'"

    def bao(cmd, *, check=True):
        r = subprocess.run(
            ["kubectl", context_arg(), "-n", "data", "exec", ob_pod, "-c", "openbao",
             "--", "sh", "-c", cmd],
            capture_output=True, text=True,
        )
        if check and r.returncode != 0:
            raise RuntimeError(f"bao failed (exit {r.returncode}): {r.stderr.strip()}")
        return r.stdout.strip()

    test_value = _secrets.token_urlsafe(16)
    test_ns    = "ory"
    test_name  = "vso-verify"

    def cleanup():
        ok("Cleaning up test resources...")
        kube("delete", "vaultstaticsecret", test_name, f"-n={test_ns}",
             "--ignore-not-found", check=False)
        kube("delete", "vaultauth", test_name, f"-n={test_ns}",
             "--ignore-not-found", check=False)
        kube("delete", "secret", test_name, f"-n={test_ns}",
             "--ignore-not-found", check=False)
        bao(f"{bao_env} bao kv delete secret/vso-test 2>/dev/null || true", check=False)

    try:
        # 1. Write test value to OpenBao KV
        ok(f"Writing test sentinel to OpenBao secret/vso-test ...")
        bao(f"{bao_env} bao kv put secret/vso-test test-key='{test_value}'")

        # 2. Create VaultAuth in ory (already in vso role's bound namespaces)
        ok(f"Creating VaultAuth {test_ns}/{test_name} ...")
        kube_apply(f"""
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultAuth
metadata:
  name: {test_name}
  namespace: {test_ns}
spec:
  method: kubernetes
  mount: kubernetes
  kubernetes:
    role: vso
    serviceAccount: default
""")

        # 3. Create VaultStaticSecret pointing at our test KV path
        ok(f"Creating VaultStaticSecret {test_ns}/{test_name} ...")
        kube_apply(f"""
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultStaticSecret
metadata:
  name: {test_name}
  namespace: {test_ns}
spec:
  vaultAuthRef: {test_name}
  mount: secret
  type: kv-v2
  path: vso-test
  refreshAfter: 10s
  destination:
    name: {test_name}
    create: true
    overwrite: true
""")

        # 4. Poll until VSO sets secretMAC (= synced)
        ok("Waiting for VSO to sync (up to 60s) ...")
        deadline = time.time() + 60
        synced   = False
        while time.time() < deadline:
            mac = kube_out(
                "get", "vaultstaticsecret", test_name, f"-n={test_ns}",
                "-o=jsonpath={.status.secretMAC}", "--ignore-not-found",
            )
            if mac and mac not in ("<none>", ""):
                synced = True
                break
            time.sleep(3)

        if not synced:
            msg = kube_out(
                "get", "vaultstaticsecret", test_name, f"-n={test_ns}",
                "-o=jsonpath={.status.conditions[0].message}", "--ignore-not-found",
            )
            raise RuntimeError(f"VSO did not sync within 60s. Last status: {msg or 'unknown'}")

        # 5. Read and verify the K8s Secret value
        ok("Verifying K8s Secret contents ...")
        raw = kube_out(
            "get", "secret", test_name, f"-n={test_ns}",
            "-o=jsonpath={.data.test-key}", "--ignore-not-found",
        )
        if not raw:
            raise RuntimeError(
                f"K8s Secret {test_ns}/{test_name} not found or missing key 'test-key'."
            )
        actual = base64.b64decode(raw).decode()
        if actual != test_value:
            raise RuntimeError(
                f"Value mismatch!\n  expected: {test_value!r}\n  got:      {actual!r}"
            )

        ok(f"Sentinel value matches -- VSO -> OpenBao integration is working.")

    except Exception as exc:
        cleanup()
        die(f"VSO verification FAILED: {exc}")

    cleanup()
    ok("VSO E2E verification passed.")
