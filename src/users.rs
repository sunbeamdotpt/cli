//! User management -- Kratos identity operations via port-forwarded admin API.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Write;

use crate::output::{ok, step, table, warn};

const SMTP_LOCAL_PORT: u16 = 10025;

// ---------------------------------------------------------------------------
// Port-forward helper
// ---------------------------------------------------------------------------

/// Spawn a kubectl port-forward process and return (child, base_url).
/// The caller **must** kill the child when done.
fn spawn_port_forward(
    ns: &str,
    svc: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<(std::process::Child, String)> {
    let ctx = crate::kube::context();
    let child = std::process::Command::new("kubectl")
        .arg(format!("--context={ctx}"))
        .args([
            "-n",
            ns,
            "port-forward",
            &format!("svc/{svc}"),
            &format!("{local_port}:{remote_port}"),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn port-forward to {ns}/svc/{svc}"))?;

    // Give the port-forward time to bind
    std::thread::sleep(std::time::Duration::from_millis(1500));

    Ok((child, format!("http://localhost:{local_port}")))
}

/// RAII guard that terminates the port-forward on drop.
struct PortForward {
    child: std::process::Child,
    pub base_url: String,
}

impl PortForward {
    fn new(ns: &str, svc: &str, local_port: u16, remote_port: u16) -> Result<Self> {
        let (child, base_url) = spawn_port_forward(ns, svc, local_port, remote_port)?;
        Ok(Self { child, base_url })
    }

    /// Convenience: Kratos admin (ory/kratos-admin 80 -> 4434).
    fn kratos() -> Result<Self> {
        Self::new("ory", "kratos-admin", 4434, 80)
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Make an HTTP request to an admin API endpoint.
fn api(
    base_url: &str,
    path: &str,
    method: &str,
    body: Option<&Value>,
    prefix: &str,
    ok_statuses: &[u16],
) -> Result<Option<Value>> {
    let url = format!("{base_url}{prefix}{path}");
    let client = reqwest::blocking::Client::new();

    let mut req = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "PATCH" => client.patch(&url),
        "DELETE" => client.delete(&url),
        _ => bail!("Unsupported HTTP method: {method}"),
    };

    req = req
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");

    if let Some(b) = body {
        req = req.json(b);
    }

    let resp = req.send().with_context(|| format!("HTTP {method} {url} failed"))?;
    let status = resp.status().as_u16();

    if !resp.status().is_success() {
        if ok_statuses.contains(&status) {
            return Ok(None);
        }
        let err_text = resp.text().unwrap_or_default();
        bail!("API error {status}: {err_text}");
    }

    let text = resp.text().unwrap_or_default();
    if text.is_empty() {
        return Ok(None);
    }
    let val: Value = serde_json::from_str(&text)
        .with_context(|| format!("Failed to parse API response as JSON: {text}"))?;
    Ok(Some(val))
}

/// Shorthand: Kratos admin API call (prefix = "/admin").
fn kratos_api(
    base_url: &str,
    path: &str,
    method: &str,
    body: Option<&Value>,
    ok_statuses: &[u16],
) -> Result<Option<Value>> {
    api(base_url, path, method, body, "/admin", ok_statuses)
}

// ---------------------------------------------------------------------------
// Identity helpers
// ---------------------------------------------------------------------------

/// Find identity by UUID or email search. Returns the identity JSON.
fn find_identity(base_url: &str, target: &str, required: bool) -> Result<Option<Value>> {
    // Looks like a UUID?
    if target.len() == 36 && target.chars().filter(|&c| c == '-').count() == 4 {
        let result = kratos_api(base_url, &format!("/identities/{target}"), "GET", None, &[])?;
        return Ok(result);
    }

    // Search by email
    let result = kratos_api(
        base_url,
        &format!("/identities?credentials_identifier={target}&page_size=1"),
        "GET",
        None,
        &[],
    )?;

    if let Some(Value::Array(arr)) = &result {
        if let Some(first) = arr.first() {
            return Ok(Some(first.clone()));
        }
    }

    if required {
        bail!("Identity not found: {target}");
    }
    Ok(None)
}

/// Build the PUT body for updating an identity, preserving all required fields.
fn identity_put_body(identity: &Value, state: Option<&str>, extra: Option<Value>) -> Value {
    let mut body = serde_json::json!({
        "schema_id": identity["schema_id"],
        "traits": identity["traits"],
        "state": state.unwrap_or_else(|| identity.get("state").and_then(|v| v.as_str()).unwrap_or("active")),
        "metadata_public": identity.get("metadata_public").cloned().unwrap_or(Value::Null),
        "metadata_admin": identity.get("metadata_admin").cloned().unwrap_or(Value::Null),
    });

    if let Some(extra_obj) = extra {
        if let (Some(base_map), Some(extra_map)) = (body.as_object_mut(), extra_obj.as_object()) {
            for (k, v) in extra_map {
                base_map.insert(k.clone(), v.clone());
            }
        }
    }

    body
}

/// Generate a 24h recovery code. Returns (link, code).
fn generate_recovery(base_url: &str, identity_id: &str) -> Result<(String, String)> {
    let body = serde_json::json!({
        "identity_id": identity_id,
        "expires_in": "24h",
    });

    let result = kratos_api(base_url, "/recovery/code", "POST", Some(&body), &[])?;

    let recovery = result.unwrap_or_default();
    let link = recovery
        .get("recovery_link")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let code = recovery
        .get("recovery_code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok((link, code))
}

/// Find the next sequential employee ID by scanning all employee identities.
fn next_employee_id(base_url: &str) -> Result<String> {
    let result = kratos_api(
        base_url,
        "/identities?page_size=200",
        "GET",
        None,
        &[],
    )?;

    let identities = match result {
        Some(Value::Array(arr)) => arr,
        _ => vec![],
    };

    let mut max_num: u64 = 0;
    for ident in &identities {
        if let Some(eid) = ident
            .get("traits")
            .and_then(|t| t.get("employee_id"))
            .and_then(|v| v.as_str())
        {
            if let Ok(n) = eid.parse::<u64>() {
                max_num = max_num.max(n);
            }
        }
    }

    Ok((max_num + 1).to_string())
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Extract a display name from identity traits (supports both default and employee schemas).
fn display_name(traits: &Value) -> String {
    let given = traits
        .get("given_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let family = traits
        .get("family_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !given.is_empty() || !family.is_empty() {
        return format!("{given} {family}").trim().to_string();
    }

    match traits.get("name") {
        Some(Value::Object(name_map)) => {
            let first = name_map
                .get("first")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let last = name_map
                .get("last")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{first} {last}").trim().to_string()
        }
        Some(name) => name.as_str().unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// Extract the short ID prefix (first 8 chars + "...").
fn short_id(id: &str) -> String {
    if id.len() >= 8 {
        format!("{}...", &id[..8])
    } else {
        id.to_string()
    }
}

/// Get identity ID as a string from a JSON value.
fn identity_id(identity: &Value) -> Result<String> {
    identity
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .context("Identity missing 'id' field")
}

// ---------------------------------------------------------------------------
// Public commands
// ---------------------------------------------------------------------------

pub async fn cmd_user_list(search: &str) -> Result<()> {
    step("Listing identities...");

    let pf = PortForward::kratos()?;
    let mut path = "/identities?page_size=20".to_string();
    if !search.is_empty() {
        path.push_str(&format!("&credentials_identifier={search}"));
    }
    let result = kratos_api(&pf.base_url, &path, "GET", None, &[])?;
    drop(pf);

    let identities = match result {
        Some(Value::Array(arr)) => arr,
        _ => vec![],
    };

    let rows: Vec<Vec<String>> = identities
        .iter()
        .map(|i| {
            let traits = i.get("traits").cloned().unwrap_or(Value::Object(Default::default()));
            let email = traits
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = display_name(&traits);
            let state = i
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("active")
                .to_string();
            let id = i
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            vec![short_id(id), email, name, state]
        })
        .collect();

    println!("{}", table(&rows, &["ID", "Email", "Name", "State"]));
    Ok(())
}

pub async fn cmd_user_get(target: &str) -> Result<()> {
    step(&format!("Getting identity: {target}"));

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    drop(pf);

    println!("{}", serde_json::to_string_pretty(&identity)?);
    Ok(())
}

pub async fn cmd_user_create(email: &str, name: &str, schema_id: &str) -> Result<()> {
    step(&format!("Creating identity: {email}"));

    let mut traits = serde_json::json!({ "email": email });
    if !name.is_empty() {
        let parts: Vec<&str> = name.splitn(2, ' ').collect();
        traits["name"] = serde_json::json!({
            "first": parts[0],
            "last": if parts.len() > 1 { parts[1] } else { "" },
        });
    }

    let body = serde_json::json!({
        "schema_id": schema_id,
        "traits": traits,
        "state": "active",
    });

    let pf = PortForward::kratos()?;
    let identity = kratos_api(&pf.base_url, "/identities", "POST", Some(&body), &[])?
        .context("Failed to create identity")?;

    let iid = identity_id(&identity)?;
    ok(&format!("Created identity: {iid}"));

    let (link, code) = generate_recovery(&pf.base_url, &iid)?;
    drop(pf);

    ok("Recovery link (valid 24h):");
    println!("{link}");
    ok("Recovery code (enter on the page above):");
    println!("{code}");
    Ok(())
}

pub async fn cmd_user_delete(target: &str) -> Result<()> {
    step(&format!("Deleting identity: {target}"));

    eprint!("Delete identity '{target}'? This cannot be undone. [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase() != "y" {
        ok("Cancelled.");
        return Ok(());
    }

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    let iid = identity_id(&identity)?;
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "DELETE",
        None,
        &[],
    )?;
    drop(pf);

    ok("Deleted.");
    Ok(())
}

pub async fn cmd_user_recover(target: &str) -> Result<()> {
    step(&format!("Generating recovery link for: {target}"));

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    let iid = identity_id(&identity)?;
    let (link, code) = generate_recovery(&pf.base_url, &iid)?;
    drop(pf);

    ok("Recovery link (valid 24h):");
    println!("{link}");
    ok("Recovery code (enter on the page above):");
    println!("{code}");
    Ok(())
}

pub async fn cmd_user_disable(target: &str) -> Result<()> {
    step(&format!("Disabling identity: {target}"));

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    let iid = identity_id(&identity)?;

    let put_body = identity_put_body(&identity, Some("inactive"), None);
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )?;
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}/sessions"),
        "DELETE",
        None,
        &[],
    )?;
    drop(pf);

    ok(&format!(
        "Identity {}... disabled and all Kratos sessions revoked.",
        &iid[..8.min(iid.len())]
    ));
    warn("App sessions (docs/people) expire within SESSION_COOKIE_AGE -- currently 1h.");
    Ok(())
}

pub async fn cmd_user_enable(target: &str) -> Result<()> {
    step(&format!("Enabling identity: {target}"));

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    let iid = identity_id(&identity)?;

    let put_body = identity_put_body(&identity, Some("active"), None);
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )?;
    drop(pf);

    ok(&format!("Identity {}... re-enabled.", short_id(&iid)));
    Ok(())
}

pub async fn cmd_user_set_password(target: &str, password: &str) -> Result<()> {
    step(&format!("Setting password for: {target}"));

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    let iid = identity_id(&identity)?;

    let extra = serde_json::json!({
        "credentials": {
            "password": {
                "config": {
                    "password": password,
                }
            }
        }
    });
    let put_body = identity_put_body(&identity, None, Some(extra));
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )?;
    drop(pf);

    ok(&format!("Password set for {}...", short_id(&iid)));
    Ok(())
}

// ---------------------------------------------------------------------------
// Onboard
// ---------------------------------------------------------------------------

/// Send a welcome email via cluster Postfix (port-forward to svc/postfix in lasuite).
fn send_welcome_email(
    domain: &str,
    email: &str,
    name: &str,
    recovery_link: &str,
    recovery_code: &str,
) -> Result<()> {
    let greeting = if name.is_empty() {
        "Hi".to_string()
    } else {
        format!("Hi {name}")
    };

    let body_text = format!(
        "{greeting},

Welcome to Sunbeam Studios! Your account has been created.

To set your password, open this link and enter the recovery code below:

  Link: {recovery_link}
  Code: {recovery_code}

This link expires in 24 hours.

Once signed in you will be prompted to set up 2FA (mandatory).

After that, head to https://auth.{domain}/settings to set up your
profile -- add your name, profile picture, and any other details.

Your services:
  Calendar:     https://cal.{domain}
  Drive:        https://drive.{domain}
  Mail:         https://mail.{domain}
  Meet:         https://meet.{domain}
  Projects:     https://projects.{domain}
  Source Code:  https://src.{domain}

Messages (Matrix):
  Download Element from https://element.io/download
  Open Element and sign in with a custom homeserver:
    Homeserver: https://messages.{domain}
  Use \"Sign in with Sunbeam Studios\" (SSO) to log in.

-- With Love & Warmth, Sunbeam Studios
"
    );

    use lettre::message::Mailbox;
    use lettre::{Message, SmtpTransport, Transport};

    let from: Mailbox = format!("Sunbeam Studios <noreply@{domain}>")
        .parse()
        .context("Invalid from address")?;
    let to: Mailbox = email.parse().context("Invalid recipient address")?;

    let message = Message::builder()
        .from(from)
        .to(to)
        .subject("Welcome to Sunbeam Studios -- Set Your Password")
        .body(body_text)
        .context("Failed to build email message")?;

    let _pf = PortForward::new("lasuite", "postfix", SMTP_LOCAL_PORT, 25)?;

    let mailer = SmtpTransport::builder_dangerous("localhost")
        .port(SMTP_LOCAL_PORT)
        .build();

    mailer
        .send(&message)
        .context("Failed to send welcome email via SMTP")?;

    ok(&format!("Welcome email sent to {email}"));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_user_onboard(
    email: &str,
    name: &str,
    schema_id: &str,
    send_email: bool,
    notify: &str,
    job_title: &str,
    department: &str,
    office_location: &str,
    hire_date: &str,
    manager: &str,
) -> Result<()> {
    step(&format!("Onboarding: {email}"));

    let pf = PortForward::kratos()?;

    let (iid, recovery_link, recovery_code) = {
        let existing = find_identity(&pf.base_url, email, false)?;

        if let Some(existing) = existing {
            let iid = identity_id(&existing)?;
            warn(&format!("Identity already exists: {}...", short_id(&iid)));
            step("Generating fresh recovery link...");
            let (link, code) = generate_recovery(&pf.base_url, &iid)?;
            (iid, link, code)
        } else {
            let mut traits = serde_json::json!({ "email": email });
            if !name.is_empty() {
                let parts: Vec<&str> = name.splitn(2, ' ').collect();
                traits["given_name"] = Value::String(parts[0].to_string());
                traits["family_name"] =
                    Value::String(if parts.len() > 1 { parts[1] } else { "" }.to_string());
            }

            let mut employee_id = String::new();
            if schema_id == "employee" {
                employee_id = next_employee_id(&pf.base_url)?;
                traits["employee_id"] = Value::String(employee_id.clone());
                if !job_title.is_empty() {
                    traits["job_title"] = Value::String(job_title.to_string());
                }
                if !department.is_empty() {
                    traits["department"] = Value::String(department.to_string());
                }
                if !office_location.is_empty() {
                    traits["office_location"] = Value::String(office_location.to_string());
                }
                if !hire_date.is_empty() {
                    traits["hire_date"] = Value::String(hire_date.to_string());
                }
                if !manager.is_empty() {
                    traits["manager"] = Value::String(manager.to_string());
                }
            }

            let body = serde_json::json!({
                "schema_id": schema_id,
                "traits": traits,
                "state": "active",
                "verifiable_addresses": [{
                    "value": email,
                    "verified": true,
                    "via": "email",
                }],
            });

            let identity = kratos_api(&pf.base_url, "/identities", "POST", Some(&body), &[])?
                .context("Failed to create identity")?;

            let iid = identity_id(&identity)?;
            ok(&format!("Created identity: {iid}"));
            if !employee_id.is_empty() {
                ok(&format!("Employee #{employee_id}"));
            }

            // Kratos ignores verifiable_addresses on POST -- PATCH to mark verified
            let patch_body = serde_json::json!([
                {"op": "replace", "path": "/verifiable_addresses/0/verified", "value": true},
                {"op": "replace", "path": "/verifiable_addresses/0/status", "value": "completed"},
            ]);
            kratos_api(
                &pf.base_url,
                &format!("/identities/{iid}"),
                "PATCH",
                Some(&patch_body),
                &[],
            )?;

            let (link, code) = generate_recovery(&pf.base_url, &iid)?;
            (iid, link, code)
        }
    };

    drop(pf);

    if send_email {
        let domain = crate::kube::get_domain().await?;
        let recipient = if notify.is_empty() { email } else { notify };
        send_welcome_email(&domain, recipient, name, &recovery_link, &recovery_code)?;
    }

    ok(&format!("Identity ID: {iid}"));
    ok("Recovery link (valid 24h):");
    println!("{recovery_link}");
    ok("Recovery code:");
    println!("{recovery_code}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Offboard
// ---------------------------------------------------------------------------

pub async fn cmd_user_offboard(target: &str) -> Result<()> {
    step(&format!("Offboarding: {target}"));

    eprint!("Offboard '{target}'? This will disable the account and revoke all sessions. [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase() != "y" {
        ok("Cancelled.");
        return Ok(());
    }

    let pf = PortForward::kratos()?;
    let identity = find_identity(&pf.base_url, target, true)?
        .context("Identity not found")?;
    let iid = identity_id(&identity)?;

    step("Disabling identity...");
    let put_body = identity_put_body(&identity, Some("inactive"), None);
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )?;
    ok(&format!("Identity {}... disabled.", short_id(&iid)));

    step("Revoking Kratos sessions...");
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}/sessions"),
        "DELETE",
        None,
        &[404],
    )?;
    ok("Kratos sessions revoked.");

    step("Revoking Hydra consent sessions...");
    {
        let hydra_pf = PortForward::new("ory", "hydra-admin", 14445, 4445)?;
        api(
            &hydra_pf.base_url,
            &format!("/oauth2/auth/sessions/consent?subject={iid}&all=true"),
            "DELETE",
            None,
            "/admin",
            &[404],
        )?;
    }
    ok("Hydra consent sessions revoked.");

    drop(pf);

    ok(&format!("Offboarding complete for {}...", short_id(&iid)));
    warn("Existing access tokens expire within ~1h (Hydra TTL).");
    warn("App sessions (docs/people) expire within SESSION_COOKIE_AGE (~1h).");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_name_employee_schema() {
        let traits = serde_json::json!({
            "email": "test@example.com",
            "given_name": "Alice",
            "family_name": "Smith",
        });
        assert_eq!(display_name(&traits), "Alice Smith");
    }

    #[test]
    fn test_display_name_default_schema() {
        let traits = serde_json::json!({
            "email": "test@example.com",
            "name": { "first": "Bob", "last": "Jones" },
        });
        assert_eq!(display_name(&traits), "Bob Jones");
    }

    #[test]
    fn test_display_name_empty() {
        let traits = serde_json::json!({ "email": "test@example.com" });
        assert_eq!(display_name(&traits), "");
    }

    #[test]
    fn test_display_name_given_only() {
        let traits = serde_json::json!({
            "given_name": "Alice",
        });
        assert_eq!(display_name(&traits), "Alice");
    }

    #[test]
    fn test_short_id() {
        assert_eq!(
            short_id("12345678-abcd-1234-abcd-123456789012"),
            "12345678..."
        );
    }

    #[test]
    fn test_short_id_short() {
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn test_identity_put_body_preserves_fields() {
        let identity = serde_json::json!({
            "schema_id": "employee",
            "traits": { "email": "a@b.com" },
            "state": "active",
            "metadata_public": null,
            "metadata_admin": null,
        });

        let body = identity_put_body(&identity, Some("inactive"), None);
        assert_eq!(body["state"], "inactive");
        assert_eq!(body["schema_id"], "employee");
        assert_eq!(body["traits"]["email"], "a@b.com");
    }

    #[test]
    fn test_identity_put_body_with_extra() {
        let identity = serde_json::json!({
            "schema_id": "default",
            "traits": { "email": "a@b.com" },
            "state": "active",
        });

        let extra = serde_json::json!({
            "credentials": {
                "password": { "config": { "password": "s3cret" } }
            }
        });
        let body = identity_put_body(&identity, None, Some(extra));
        assert_eq!(body["state"], "active");
        assert!(body["credentials"]["password"]["config"]["password"] == "s3cret");
    }

    #[test]
    fn test_identity_put_body_default_state() {
        let identity = serde_json::json!({
            "schema_id": "default",
            "traits": {},
            "state": "inactive",
        });
        let body = identity_put_body(&identity, None, None);
        assert_eq!(body["state"], "inactive");
    }

    #[test]
    fn test_identity_id_extraction() {
        let identity = serde_json::json!({ "id": "12345678-abcd-1234-abcd-123456789012" });
        assert_eq!(
            identity_id(&identity).unwrap(),
            "12345678-abcd-1234-abcd-123456789012"
        );
    }

    #[test]
    fn test_identity_id_missing() {
        let identity = serde_json::json!({});
        assert!(identity_id(&identity).is_err());
    }
}
