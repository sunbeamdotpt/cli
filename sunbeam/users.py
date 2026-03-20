"""User management — Kratos identity operations via port-forwarded admin API."""
import json
import smtplib
import subprocess
import sys
import time
import urllib.request
import urllib.error
from contextlib import contextmanager
from email.message import EmailMessage

import sunbeam.kube as _kube_mod
from sunbeam.output import step, ok, warn, die, table

_SMTP_LOCAL_PORT = 10025


@contextmanager
def _port_forward(ns="ory", svc="kratos-admin", local_port=4434, remote_port=80):
    """Port-forward to a cluster service and yield the local base URL."""
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


def _api(base_url, path, method="GET", body=None, prefix="/admin", ok_statuses=()):
    """Make a request to an admin API via port-forward."""
    url = f"{base_url}{prefix}{path}"
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as resp:
            resp_body = resp.read()
            return json.loads(resp_body) if resp_body else None
    except urllib.error.HTTPError as e:
        if e.code in ok_statuses:
            return None
        err_text = e.read().decode()
        die(f"API error {e.code}: {err_text}")


def _find_identity(base_url, target, required=True):
    """Find identity by email or ID. Returns identity dict or None if not required."""
    # Try as ID first
    if len(target) == 36 and target.count("-") == 4:
        return _api(base_url, f"/identities/{target}")
    # Search by email
    result = _api(base_url, f"/identities?credentials_identifier={target}&page_size=1")
    if isinstance(result, list) and result:
        return result[0]
    if required:
        die(f"Identity not found: {target}")
    return None


def _identity_put_body(identity, state=None, **extra):
    """Build the PUT body for updating an identity, preserving all required fields."""
    body = {
        "schema_id": identity["schema_id"],
        "traits": identity["traits"],
        "state": state or identity.get("state", "active"),
        "metadata_public": identity.get("metadata_public"),
        "metadata_admin": identity.get("metadata_admin"),
    }
    body.update(extra)
    return body


def _generate_recovery(base_url, identity_id):
    """Generate a 24h recovery code. Returns (link, code)."""
    recovery = _api(base_url, "/recovery/code", method="POST", body={
        "identity_id": identity_id,
        "expires_in": "24h",
    })
    return recovery.get("recovery_link", ""), recovery.get("recovery_code", "")


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
        # Support both employee (given_name/family_name) and default (name.first/last) schemas
        given = traits.get("given_name", "")
        family = traits.get("family_name", "")
        if given or family:
            display_name = f"{given} {family}".strip()
        else:
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
        link, code = _generate_recovery(base, identity["id"])

    ok("Recovery link (valid 24h):")
    print(link)
    ok("Recovery code (enter on the page above):")
    print(code)


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
        link, code = _generate_recovery(base, identity["id"])
    ok("Recovery link (valid 24h):")
    print(link)
    ok("Recovery code (enter on the page above):")
    print(code)


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
        _api(base, f"/identities/{iid}", method="PUT",
             body=_identity_put_body(identity, state="inactive"))
        _api(base, f"/identities/{iid}/sessions", method="DELETE")
    ok(f"Identity {iid[:8]}... disabled and all Kratos sessions revoked.")
    warn("App sessions (docs/people) expire within SESSION_COOKIE_AGE — currently 1h.")


def cmd_user_set_password(target, password):
    """Set (or reset) the password credential for an identity."""
    step(f"Setting password for: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
        iid = identity["id"]
        _api(base, f"/identities/{iid}", method="PUT",
             body=_identity_put_body(identity, credentials={
                 "password": {"config": {"password": password}},
             }))
    ok(f"Password set for {iid[:8]}...")


def cmd_user_enable(target):
    """Re-enable a previously disabled identity."""
    step(f"Enabling identity: {target}")
    with _port_forward() as base:
        identity = _find_identity(base, target)
        iid = identity["id"]
        _api(base, f"/identities/{iid}", method="PUT",
             body=_identity_put_body(identity, state="active"))
    ok(f"Identity {iid[:8]}... re-enabled.")


def _send_welcome_email(domain, email, name, recovery_link, recovery_code,
                        job_title="", department=""):
    """Send a welcome email via cluster Postfix (port-forward to svc/postfix in lasuite)."""
    greeting = f"Hi {name}" if name else "Hi"
    body_text = f"""{greeting},

Welcome to Sunbeam Studios!{f" You're joining as {job_title} in the {department} department." if job_title and department else ""} Your account has been created.

To set your password, open this link and enter the recovery code below:

  Link: {recovery_link}
  Code: {recovery_code}

This link expires in 24 hours.

Once signed in you will be prompted to set up 2FA (mandatory).

After that, head to https://auth.{domain}/settings to set up your
profile — add your name, profile picture, and any other details.

Your services:
  Calendar:     https://cal.{domain}
  Drive:        https://drive.{domain}
  Mail:         https://mail.{domain}
  Meet:         https://meet.{domain}
  Projects:     https://projects.{domain}
  Source Code:  https://src.{domain}

Messages (Matrix):
  Download Element for your platform:
    Desktop: https://element.io/download
    iOS:     https://apps.apple.com/app/element-messenger/id1083446067
    Android: https://play.google.com/store/apps/details?id=im.vector.app

  Setup:
    1. Open Element and tap "Sign in"
    2. Tap "Edit" next to the homeserver field (matrix.org)
    3. Enter: https://messages.{domain}
    4. Tap "Continue" — you'll be redirected to Sunbeam Studios SSO
    5. Sign in with your {domain} email and password

\u2014 With Love & Warmth, Sunbeam Studios
"""
    msg = EmailMessage()
    msg["Subject"] = "Welcome to Sunbeam Studios — Set Your Password"
    msg["From"] = f"Sunbeam Studios <noreply@{domain}>"
    msg["To"] = email
    msg.set_content(body_text)

    with _port_forward(ns="lasuite", svc="postfix", local_port=_SMTP_LOCAL_PORT, remote_port=25):
        with smtplib.SMTP("localhost", _SMTP_LOCAL_PORT) as smtp:
            smtp.send_message(msg)
    ok(f"Welcome email sent to {email}")


def _next_employee_id(base_url):
    """Find the next sequential employee ID by scanning all employee identities."""
    identities = _api(base_url, "/identities?page_size=200") or []
    max_num = 0
    for ident in identities:
        eid = ident.get("traits", {}).get("employee_id", "")
        if eid and eid.isdigit():
            max_num = max(max_num, int(eid))
    return str(max_num + 1)


def _create_mailbox(email, name=""):
    """Create a mailbox in Messages via kubectl exec into the backend."""
    local_part, domain_part = email.split("@", 1)
    display_name = name or local_part
    step(f"Creating mailbox: {email}")
    result = _kube_mod.kube_out(
        "exec", "deployment/messages-backend", "-n", "lasuite",
        "-c", "messages-backend", "--",
        "python", "manage.py", "shell", "-c",
        f"""
mb, created = Mailbox.objects.get_or_create(
    local_part="{local_part}",
    domain=MailDomain.objects.get(name="{domain_part}"),
)
print("created" if created else "exists")
""",
    )
    if "created" in (result or ""):
        ok(f"Mailbox {email} created.")
    elif "exists" in (result or ""):
        ok(f"Mailbox {email} already exists.")
    else:
        warn(f"Could not create mailbox (Messages backend may not be running): {result}")


def _delete_mailbox(email):
    """Delete a mailbox and associated Django user in Messages."""
    local_part, domain_part = email.split("@", 1)
    step(f"Cleaning up mailbox: {email}")
    result = _kube_mod.kube_out(
        "exec", "deployment/messages-backend", "-n", "lasuite",
        "-c", "messages-backend", "--",
        "python", "manage.py", "shell", "-c",
        f"""
from django.contrib.auth import get_user_model
User = get_user_model()
# Delete mailbox + access + contacts
deleted = 0
for mb in Mailbox.objects.filter(local_part="{local_part}", domain__name="{domain_part}"):
    mb.delete()
    deleted += 1
# Delete Django user
try:
    u = User.objects.get(email="{email}")
    u.delete()
    deleted += 1
except User.DoesNotExist:
    pass
print(f"deleted {{deleted}}")
""",
    )
    if "deleted" in (result or ""):
        ok(f"Mailbox and user cleaned up.")
    else:
        warn(f"Could not clean up mailbox: {result}")


def _setup_projects_user(email, name=""):
    """Create a Projects (Planka) user and add them as manager of the Default project."""
    step(f"Setting up Projects user: {email}")
    js = f"""
const knex = require('knex')({{client: 'pg', connection: process.env.DATABASE_URL}});
async function go() {{
  // Create or find user
  let user = await knex('user_account').where({{email: '{email}'}}).first();
  if (!user) {{
    const id = Date.now().toString();
    await knex('user_account').insert({{
      id, email: '{email}', name: '{name}', password: '',
      is_admin: true, is_sso: true, language: 'en-US',
      created_at: new Date(), updated_at: new Date()
    }});
    user = {{id}};
    console.log('user_created');
  }} else {{
    console.log('user_exists');
  }}
  // Add to Default project
  const project = await knex('project').where({{name: 'Default'}}).first();
  if (project) {{
    const exists = await knex('project_manager').where({{project_id: project.id, user_id: user.id}}).first();
    if (!exists) {{
      await knex('project_manager').insert({{
        id: (Date.now()+1).toString(), project_id: project.id,
        user_id: user.id, created_at: new Date()
      }});
      console.log('manager_added');
    }} else {{
      console.log('manager_exists');
    }}
  }} else {{
    console.log('no_default_project');
  }}
}}
go().then(() => process.exit(0)).catch(e => {{ console.error(e.message); process.exit(1); }});
"""
    result = _kube_mod.kube_out(
        "exec", "deployment/projects", "-n", "lasuite",
        "-c", "projects", "--", "node", "-e", js,
    )
    if "manager_added" in (result or "") or "manager_exists" in (result or ""):
        ok(f"Projects user ready.")
    elif "no_default_project" in (result or ""):
        warn("No Default project found in Projects — skip.")
    else:
        warn(f"Could not set up Projects user: {result}")


def _cleanup_projects_user(email):
    """Remove a user from Projects (Planka) — delete memberships and user record."""
    step(f"Cleaning up Projects user: {email}")
    js = f"""
const knex = require('knex')({{client: 'pg', connection: process.env.DATABASE_URL}});
async function go() {{
  const user = await knex('user_account').where({{email: '{email}'}}).first();
  if (!user) {{ console.log('not_found'); return; }}
  await knex('board_membership').where({{user_id: user.id}}).del();
  await knex('project_manager').where({{user_id: user.id}}).del();
  await knex('user_account').where({{id: user.id}}).update({{deleted_at: new Date()}});
  console.log('cleaned');
}}
go().then(() => process.exit(0)).catch(e => {{ console.error(e.message); process.exit(1); }});
"""
    result = _kube_mod.kube_out(
        "exec", "deployment/projects", "-n", "lasuite",
        "-c", "projects", "--", "node", "-e", js,
    )
    if "cleaned" in (result or ""):
        ok("Projects user cleaned up.")
    else:
        warn(f"Could not clean up Projects user: {result}")


def cmd_user_onboard(email, name="", schema_id="employee", send_email=True,
                     notify="", job_title="", department="", office_location="",
                     hire_date="", manager=""):
    """Onboard a new user: create identity, generate recovery link, optionally send welcome email."""
    step(f"Onboarding: {email}")

    with _port_forward() as base:
        existing = _find_identity(base, email, required=False)

        if existing:
            warn(f"Identity already exists: {existing['id'][:8]}...")
            step("Generating fresh recovery link...")
            iid = existing["id"]
            recovery_link, recovery_code = _generate_recovery(base, iid)
        else:
            traits = {"email": email}
            if name:
                parts = name.split(" ", 1)
                traits["given_name"] = parts[0]
                traits["family_name"] = parts[1] if len(parts) > 1 else ""

            # Auto-assign employee ID if not provided and using employee schema
            employee_id = ""
            if schema_id == "employee":
                employee_id = _next_employee_id(base)
                traits["employee_id"] = employee_id
                if job_title:
                    traits["job_title"] = job_title
                if department:
                    traits["department"] = department
                if office_location:
                    traits["office_location"] = office_location
                if hire_date:
                    traits["hire_date"] = hire_date
                if manager:
                    traits["manager"] = manager

            identity = _api(base, "/identities", method="POST", body={
                "schema_id": schema_id,
                "traits": traits,
                "state": "active",
                "verifiable_addresses": [{
                    "value": email,
                    "verified": True,
                    "via": "email",
                }],
            })
            iid = identity["id"]
            ok(f"Created identity: {iid}")
            if employee_id:
                ok(f"Employee #{employee_id}")

            # Kratos ignores verifiable_addresses on POST — PATCH is required
            _api(base, f"/identities/{iid}", method="PATCH", body=[
                {"op": "replace", "path": "/verifiable_addresses/0/verified", "value": True},
                {"op": "replace", "path": "/verifiable_addresses/0/status", "value": "completed"},
            ])

            recovery_link, recovery_code = _generate_recovery(base, iid)

    # Provision app-level accounts
    if not existing:
        _create_mailbox(email, name)
        _setup_projects_user(email, name)

    if send_email:
        domain = _kube_mod.get_domain()
        recipient = notify or email
        _send_welcome_email(domain, recipient, name, recovery_link, recovery_code,
                            job_title=job_title, department=department)

    ok(f"Identity ID: {iid}")
    ok("Recovery link (valid 24h):")
    print(recovery_link)
    ok("Recovery code:")
    print(recovery_code)


def cmd_user_offboard(target):
    """Offboard a user: disable identity, revoke all Kratos + Hydra sessions."""
    step(f"Offboarding: {target}")

    confirm = input(f"Offboard '{target}'? This will disable the account and revoke all sessions. [y/N] ").strip().lower()
    if confirm != "y":
        ok("Cancelled.")
        return

    with _port_forward() as base:
        identity = _find_identity(base, target)
        iid = identity["id"]

        step("Disabling identity...")
        _api(base, f"/identities/{iid}", method="PUT",
             body=_identity_put_body(identity, state="inactive"))
        ok(f"Identity {iid[:8]}... disabled.")

        step("Revoking Kratos sessions...")
        _api(base, f"/identities/{iid}/sessions", method="DELETE", ok_statuses=(404,))
        ok("Kratos sessions revoked.")

        step("Revoking Hydra consent sessions...")
        with _port_forward(svc="hydra-admin", local_port=14445, remote_port=4445) as hydra_base:
            _api(hydra_base, f"/oauth2/auth/sessions/consent?subject={iid}&all=true",
                 method="DELETE", prefix="/admin", ok_statuses=(404,))
        ok("Hydra consent sessions revoked.")

    # Clean up Messages Django user and mailbox
    email = identity.get("traits", {}).get("email", "")
    if email:
        _delete_mailbox(email)
        _cleanup_projects_user(email)

    ok(f"Offboarding complete for {iid[:8]}...")
    warn("Existing access tokens expire within ~1h (Hydra TTL).")
    warn("App sessions (docs/people) expire within SESSION_COOKIE_AGE (~1h).")
