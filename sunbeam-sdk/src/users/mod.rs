//! User management -- Kratos identity operations via port-forwarded admin API.

mod provisioning;
pub use provisioning::{cmd_user_onboard, cmd_user_offboard};

use serde_json::Value;
use std::io::Write;

use crate::error::{Result, ResultExt, SunbeamError};
use crate::output::{ok, step, table, warn};

const SMTP_LOCAL_PORT: u16 = 10025;

// ---------------------------------------------------------------------------
// Port-forward helper
// ---------------------------------------------------------------------------

/// RAII guard that terminates the port-forward on drop.
struct PortForward {
    child: tokio::process::Child,
    pub base_url: String,
}

impl PortForward {
    async fn new(ns: &str, svc: &str, local_port: u16, remote_port: u16) -> Result<Self> {
        let ctx = crate::kube::context();
        let child = tokio::process::Command::new("kubectl")
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
            .with_ctx(|| format!("Failed to spawn port-forward to {ns}/svc/{svc}"))?;

        // Give the port-forward time to bind
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        Ok(Self {
            child,
            base_url: format!("http://localhost:{local_port}"),
        })
    }

    /// Convenience: Kratos admin (ory/kratos-admin 80 -> 4434).
    async fn kratos() -> Result<Self> {
        Self::new("ory", "kratos-admin", 4434, 80).await
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Make an HTTP request to an admin API endpoint.
async fn api(
    base_url: &str,
    path: &str,
    method: &str,
    body: Option<&Value>,
    prefix: &str,
    ok_statuses: &[u16],
) -> Result<Option<Value>> {
    let url = format!("{base_url}{prefix}{path}");
    let client = reqwest::Client::new();

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

    let resp = req
        .send()
        .await
        .with_ctx(|| format!("HTTP {method} {url} failed"))?;
    let status = resp.status().as_u16();

    if !resp.status().is_success() {
        if ok_statuses.contains(&status) {
            return Ok(None);
        }
        let err_text = resp.text().await.unwrap_or_default();
        bail!("API error {status}: {err_text}");
    }

    let text = resp.text().await.unwrap_or_default();
    if text.is_empty() {
        return Ok(None);
    }
    let val: Value = serde_json::from_str(&text)
        .with_ctx(|| format!("Failed to parse API response as JSON: {text}"))?;
    Ok(Some(val))
}

/// Shorthand: Kratos admin API call (prefix = "/admin").
async fn kratos_api(
    base_url: &str,
    path: &str,
    method: &str,
    body: Option<&Value>,
    ok_statuses: &[u16],
) -> Result<Option<Value>> {
    api(base_url, path, method, body, "/admin", ok_statuses).await
}

// ---------------------------------------------------------------------------
// Identity helpers
// ---------------------------------------------------------------------------

/// Find identity by UUID or email search. Returns the identity JSON.
async fn find_identity(base_url: &str, target: &str, required: bool) -> Result<Option<Value>> {
    // Looks like a UUID?
    if target.len() == 36 && target.chars().filter(|&c| c == '-').count() == 4 {
        let result = kratos_api(base_url, &format!("/identities/{target}"), "GET", None, &[]).await?;
        return Ok(result);
    }

    // Search by email
    let result = kratos_api(
        base_url,
        &format!("/identities?credentials_identifier={target}&page_size=1"),
        "GET",
        None,
        &[],
    )
    .await?;

    if let Some(Value::Array(arr)) = &result {
        if let Some(first) = arr.first() {
            return Ok(Some(first.clone()));
        }
    }

    if required {
        return Err(SunbeamError::identity(format!("Identity not found: {target}")));
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
async fn generate_recovery(base_url: &str, identity_id: &str) -> Result<(String, String)> {
    let body = serde_json::json!({
        "identity_id": identity_id,
        "expires_in": "24h",
    });

    let result = kratos_api(base_url, "/recovery/code", "POST", Some(&body), &[]).await?;

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
///
/// Paginates through all identities using `page` and `page_size` params to
/// avoid missing employee IDs when there are more than 200 identities.
async fn next_employee_id(base_url: &str) -> Result<String> {
    let mut max_num: u64 = 0;
    let mut page = 1;
    loop {
        let result = kratos_api(
            base_url,
            &format!("/identities?page_size=200&page={page}"),
            "GET",
            None,
            &[],
        )
        .await?;

        let identities = match result {
            Some(Value::Array(arr)) if !arr.is_empty() => arr,
            _ => break,
        };

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

        if identities.len() < 200 {
            break; // last page
        }
        page += 1;
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
        .ok_or_else(|| SunbeamError::identity("Identity missing 'id' field"))
}

// ---------------------------------------------------------------------------
// Public commands
// ---------------------------------------------------------------------------

pub async fn cmd_user_list(search: &str) -> Result<()> {
    step("Listing identities...");

    let pf = PortForward::kratos().await?;
    let mut path = "/identities?page_size=20".to_string();
    if !search.is_empty() {
        path.push_str(&format!("&credentials_identifier={search}"));
    }
    let result = kratos_api(&pf.base_url, &path, "GET", None, &[]).await?;
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

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
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

    let pf = PortForward::kratos().await?;
    let identity = kratos_api(&pf.base_url, "/identities", "POST", Some(&body), &[])
        .await?
        .ok_or_else(|| SunbeamError::identity("Failed to create identity"))?;

    let iid = identity_id(&identity)?;
    ok(&format!("Created identity: {iid}"));

    let (link, code) = generate_recovery(&pf.base_url, &iid).await?;
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

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
    let iid = identity_id(&identity)?;
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "DELETE",
        None,
        &[],
    )
    .await?;
    drop(pf);

    ok("Deleted.");
    Ok(())
}

pub async fn cmd_user_recover(target: &str) -> Result<()> {
    step(&format!("Generating recovery link for: {target}"));

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
    let iid = identity_id(&identity)?;
    let (link, code) = generate_recovery(&pf.base_url, &iid).await?;
    drop(pf);

    ok("Recovery link (valid 24h):");
    println!("{link}");
    ok("Recovery code (enter on the page above):");
    println!("{code}");
    Ok(())
}

pub async fn cmd_user_disable(target: &str) -> Result<()> {
    step(&format!("Disabling identity: {target}"));

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
    let iid = identity_id(&identity)?;

    let put_body = identity_put_body(&identity, Some("inactive"), None);
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )
    .await?;
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}/sessions"),
        "DELETE",
        None,
        &[],
    )
    .await?;
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

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
    let iid = identity_id(&identity)?;

    let put_body = identity_put_body(&identity, Some("active"), None);
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )
    .await?;
    drop(pf);

    ok(&format!("Identity {}... re-enabled.", short_id(&iid)));
    Ok(())
}

pub async fn cmd_user_set_password(target: &str, password: &str) -> Result<()> {
    step(&format!("Setting password for: {target}"));

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
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
    )
    .await?;
    drop(pf);

    ok(&format!("Password set for {}...", short_id(&iid)));
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
