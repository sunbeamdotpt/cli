"""User management — Kratos identity operations via port-forwarded admin API."""
import json
import subprocess
import sys
import time
import urllib.request
import urllib.error
from contextlib import contextmanager

import sunbeam.kube as _kube_mod
from sunbeam.output import step, ok, warn, die, table


@contextmanager
def _port_forward(ns="ory", svc="kratos-admin", local_port=4434, remote_port=80):
    """Port-forward directly to the Kratos admin HTTP API and yield the local URL."""
    proc = subprocess.Popen(
        ["kubectl", _kube_mod.context_arg(), "-n", ns, "port-forward",
         f"svc/{svc}", f"{local_port}:{remote_port}"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    # Wait for port-forward to be ready
    time.sleep(1.5)
    try:
        yield f"http://localhost:{local_port}"
    finally:
        proc.terminate()
        proc.wait()


def _api(base_url, path, method="GET", body=None):
    """Make a request to the Kratos admin API via port-forward."""
    url = f"{base_url}/admin{path}"
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as resp:
            body = resp.read()
            return json.loads(body) if body else None
    except urllib.error.HTTPError as e:
        body_text = e.read().decode()
        die(f"API error {e.code}: {body_text}")


def _find_identity(base_url, target):
    """Find identity by email or ID. Returns identity dict."""
    # Try as ID first
    if len(target) == 36 and target.count("-") == 4:
        return _api(base_url, f"/identities/{target}")
    # Search by email
    result = _api(base_url, f"/identities?credentials_identifier={target}&page_size=1")
    if isinstance(result, list) and result:
        return result[0]
    die(f"Identity not found: {target}")


def cmd_user_list(search=""):
    step("Listing identities...")
    with _port_forward() as base:
        path = f"/identities?page_size=20"
        if search:
            path += f"&credentials_identifier={search}"
        identities = _api(base, path)

    rows = []
    for i in identities or []:
        traits = i.get("traits", {})
        email = traits.get("email", "")
        name = traits.get("name", {})
        if isinstance(name, dict):
            display_name = f"{name.get('first', '')} {name.get('last', '')}".strip()
        else:
            display_name = str(name) if name else ""
        rows.append([i["id"][:8] + "...", email, display_name, i.get("state", "active")])

    print(table(rows, ["ID", "Email", "Name", "State"]))


def cmd_user_get(target):
    step(f"Getting identity: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
    print(json.dumps(identity, indent=2))


def cmd_user_create(email, name="", schema_id="default"):
    step(f"Creating identity: {email}")
    traits = {"email": email}
    if name:
        parts = name.split(" ", 1)
        traits["name"] = {"first": parts[0], "last": parts[1] if len(parts) > 1 else ""}

    body = {
        "schema_id": schema_id,
        "traits": traits,
        "state": "active",
    }

    with _port_forward() as base:
        identity = _api(base, "/identities", method="POST", body=body)
        ok(f"Created identity: {identity['id']}")

        # Generate recovery code (link is deprecated in Kratos v1.x)
        recovery = _api(base, "/recovery/code", method="POST", body={
            "identity_id": identity["id"],
            "expires_in": "24h",
        })

    ok("Recovery link (valid 24h):")
    print(recovery.get("recovery_link", ""))
    ok("Recovery code (enter on the page above):")
    print(recovery.get("recovery_code", ""))


def cmd_user_delete(target):
    step(f"Deleting identity: {target}")

    confirm = input(f"Delete identity '{target}'? This cannot be undone. [y/N] ").strip().lower()
    if confirm != "y":
        ok("Cancelled.")
        return

    with _port_forward() as base:
        identity = _find_identity(base, target)
        _api(base, f"/identities/{identity['id']}", method="DELETE")
    ok(f"Deleted.")


def cmd_user_recover(target):
    step(f"Generating recovery link for: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
        recovery = _api(base, "/recovery/code", method="POST", body={
            "identity_id": identity["id"],
            "expires_in": "24h",
        })
    ok("Recovery link (valid 24h):")
    print(recovery.get("recovery_link", ""))
    ok("Recovery code (enter on the page above):")
    print(recovery.get("recovery_code", ""))


def cmd_user_disable(target):
    """Disable identity + revoke all Kratos sessions (emergency lockout).

    After this:
    - No new logins possible.
    - Existing Hydra OAuth2 tokens are revoked.
    - Django app sessions expire within SESSION_COOKIE_AGE (1h).
    """
    step(f"Disabling identity: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
        iid = identity["id"]
        _api(base, f"/identities/{iid}", method="PUT", body={
            "schema_id": identity["schema_id"],
            "traits": identity["traits"],
            "state": "inactive",
            "metadata_public": identity.get("metadata_public"),
            "metadata_admin": identity.get("metadata_admin"),
        })
        _api(base, f"/identities/{iid}/sessions", method="DELETE")
    ok(f"Identity {iid[:8]}... disabled and all Kratos sessions revoked.")
    warn("App sessions (docs/people) expire within SESSION_COOKIE_AGE — currently 1h.")


def cmd_user_set_password(target, password):
    """Set (or reset) the password credential for an identity."""
    step(f"Setting password for: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
        iid = identity["id"]
        _api(base, f"/identities/{iid}", method="PUT", body={
            "schema_id": identity["schema_id"],
            "traits": identity["traits"],
            "state": identity.get("state", "active"),
            "metadata_public": identity.get("metadata_public"),
            "metadata_admin": identity.get("metadata_admin"),
            "credentials": {
                "password": {
                    "config": {"password": password},
                },
            },
        })
    ok(f"Password set for {iid[:8]}...")


def cmd_user_enable(target):
    """Re-enable a previously disabled identity."""
    step(f"Enabling identity: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
        iid = identity["id"]
        _api(base, f"/identities/{iid}", method="PUT", body={
            "schema_id": identity["schema_id"],
            "traits": identity["traits"],
            "state": "active",
            "metadata_public": identity.get("metadata_public"),
            "metadata_admin": identity.get("metadata_admin"),
        })
    ok(f"Identity {iid[:8]}... re-enabled.")
