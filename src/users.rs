//! User management — sso-gateway identity operations via ConnectRPC.
//!
//! All identity CRUD runs against `iam.v1.IdentityService` on the sso-gateway
//! (`https://sso.{domain}`) with the logged-in SSO token; the tenant is
//! resolved server-side from the token subject. The Kratos admin HTTP API and
//! the port-forward-to-kratos pattern are gone: the gateway owns the identity
//! trait model and exposes it over ConnectRPC.
//!
//! Surface changes vs the Kratos-backed v2 implementation:
//! - `disable`/`enable` are gone — the sso-gateway IAM has no identity-state
//!   RPC (tenant membership state exists in the gateway DB but is not exposed
//!   on any service). Lockout = `offboard` (revoke sessions + delete).
//! - `set-password` is gone — `UpdateIdentity` is traits-only and there is no
//!   admin credential-set RPC. Password resets go through `recover`.
//! - `recover`/`create`/`onboard` use `CreateRecoveryLink`, which returns a
//!   gateway-hosted recovery link plus a recovery token (replaces the Kratos
//!   recovery link + code pair).

use serde_json::Value;
use std::io::Write;

use crate::output::table;
use sdk::auth::{AuthClient, v1 as iam};
use sdk::error::{Result, ResultExt, SunbeamError};
use sdk::kanban::prelude::buffa::MessageField;
use sdk::kanban::prelude::buffa_types::google::protobuf::Struct;

// ---------------------------------------------------------------------------
// Port-forward helper (Stalwart SMTP only)
// ---------------------------------------------------------------------------

/// Kube-rs port-forward guard serving a loopback TCP listener until dropped.
struct PortForward {
    _guard: sdk::secrets::PortForwardGuard,
    pub base_url: String,
}

impl PortForward {
    /// Stalwart SMTP (stalwart/stalwart pod, remote port 25).
    async fn stalwart_smtp() -> Result<Self> {
        let guard =
            sdk::secrets::port_forward_svc("stalwart", "app.kubernetes.io/name=stalwart", 25)
                .await?;
        let local_port = guard.local_port;
        Ok(Self {
            _guard: guard,
            base_url: format!("http://127.0.0.1:{local_port}"),
        })
    }
}

// ---------------------------------------------------------------------------
// Identity helpers
// ---------------------------------------------------------------------------

/// Authenticated sso-gateway client for the active context.
async fn identity_client() -> Result<AuthClient> {
    crate::auth::authenticated_auth_client().await
}

/// Build a protobuf `Struct` from a JSON object of identity traits.
fn traits_struct(traits: Value) -> MessageField<Struct> {
    match serde_json::from_value(traits) {
        Ok(s) => MessageField::some(s),
        Err(_) => MessageField::none(),
    }
}

/// Serialize an identity's traits back to a JSON value (empty object when unset).
fn traits_json(identity: &iam::Identity) -> Value {
    identity
        .traits
        .as_option()
        .and_then(|t| serde_json::to_value(t).ok())
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// Find an identity by email or ID, erroring when absent.
async fn require_identity(client: &AuthClient, target: &str) -> Result<iam::Identity> {
    crate::auth::find_identity(client, target)
        .await?
        .ok_or_else(|| SunbeamError::identity(format!("Identity not found: {target}")))
}

/// Create a 24h recovery link for an identity.
async fn generate_recovery(client: &AuthClient, identity_id: &str) -> Result<iam::RecoveryLink> {
    let link = client
        .identity()
        .create_recovery_link(iam::CreateRecoveryLinkRequest {
            identity_id: identity_id.to_string(),
            expires_in_seconds: 24 * 3600,
            ..Default::default()
        })
        .await?
        .into_owned();
    Ok(link)
}

/// Print the recovery link + token to the user.
fn print_recovery(link: &iam::RecoveryLink) {
    tracing::info!("Recovery link (valid 24h):");
    println!("{}", link.recovery_link);
    tracing::info!("Recovery token (enter on the page above):");
    println!("{}", link.recovery_token);
}

/// Find the next sequential employee ID by scanning all employee identities.
///
/// Pages the full tenant directory (there is no server-side trait filter) and
/// takes the max numeric `employee_id` trait + 1.
async fn next_employee_id(client: &AuthClient) -> Result<String> {
    let identities = crate::auth::list_all_identities(client).await?;
    let mut max_num: u64 = 0;
    for ident in &identities {
        if let Some(eid) = traits_json(ident)
            .get("employee_id")
            .and_then(|v| v.as_str())
            && let Ok(n) = eid.parse::<u64>()
        {
            max_num = max_num.max(n);
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
            let first = name_map.get("first").and_then(|v| v.as_str()).unwrap_or("");
            let last = name_map.get("last").and_then(|v| v.as_str()).unwrap_or("");
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

/// Split "First Last" into the name trait object.
fn name_traits(name: &str) -> Value {
    let parts: Vec<&str> = name.splitn(2, ' ').collect();
    serde_json::json!({
        "first": parts[0],
        "last": if parts.len() > 1 { parts[1] } else { "" },
    })
}

// ---------------------------------------------------------------------------
// Public commands
// ---------------------------------------------------------------------------

/// Cmd user list.
#[tracing::instrument(skip(search))]
pub async fn cmd_user_list(search: &str) -> Result<()> {
    tracing::info!("Listing identities...");

    let client = identity_client().await?;
    let identities = crate::auth::list_all_identities(&client).await?;

    let search = search.to_lowercase();
    let rows: Vec<Vec<String>> = identities
        .iter()
        .filter(|i| {
            search.is_empty()
                || crate::auth::identity_email(i)
                    .to_lowercase()
                    .contains(&search)
        })
        .map(|i| {
            let traits = traits_json(i);
            let email = crate::auth::identity_email(i);
            let name = display_name(&traits);
            vec![short_id(&i.id), email, name, i.schema_id.clone()]
        })
        .collect();

    println!("{}", table(&rows, &["ID", "Email", "Name", "Schema"]));
    Ok(())
}

/// Cmd user get.
#[tracing::instrument(skip(target))]
pub async fn cmd_user_get(target: &str) -> Result<()> {
    tracing::info!("Getting identity: {target}");

    let client = identity_client().await?;
    let identity = require_identity(&client, target).await?;

    println!("{}", serde_json::to_string_pretty(&identity)?);
    Ok(())
}

/// Cmd user create.
#[tracing::instrument]
pub async fn cmd_user_create(email: &str, name: &str, schema_id: &str) -> Result<()> {
    tracing::info!("Creating identity: {email}");

    let mut traits = serde_json::json!({ "email": email });
    if !name.is_empty() {
        traits["name"] = name_traits(name);
    }

    let client = identity_client().await?;
    let identity = client
        .identity()
        .create_identity(iam::CreateIdentityRequest {
            schema_id: schema_id.to_string(),
            traits: traits_struct(traits),
            ..Default::default()
        })
        .await?
        .into_owned();

    let iid = identity.id.clone();
    tracing::info!("Created identity: {iid}");

    let link = generate_recovery(&client, &iid).await?;
    print_recovery(&link);
    Ok(())
}

/// Delete an identity by email or ID.
async fn delete_identity(client: &AuthClient, target: &str) -> Result<()> {
    let identity = require_identity(client, target).await?;
    client
        .identity()
        .delete_identity(iam::DeleteIdentityRequest {
            id: identity.id.clone(),
            ..Default::default()
        })
        .await?;
    Ok(())
}

/// Cmd user delete.
#[tracing::instrument(skip(target))]
pub async fn cmd_user_delete(target: &str) -> Result<()> {
    tracing::info!("Deleting identity: {target}");

    eprint!("Delete identity '{target}'? This cannot be undone. [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase() != "y" {
        tracing::info!("Cancelled.");
        return Ok(());
    }

    let client = identity_client().await?;
    delete_identity(&client, target).await?;

    tracing::info!("Deleted.");
    Ok(())
}

/// Cmd user recover.
#[tracing::instrument(skip(target))]
pub async fn cmd_user_recover(target: &str) -> Result<()> {
    tracing::info!("Generating recovery link for: {target}");

    let client = identity_client().await?;
    let identity = require_identity(&client, target).await?;
    let link = generate_recovery(&client, &identity.id).await?;
    print_recovery(&link);
    Ok(())
}

/// Revoke every active session of an identity.
async fn revoke_all_sessions(client: &AuthClient, identity_id: &str) -> Result<usize> {
    let mut revoked = 0;
    let mut page_token = String::new();
    loop {
        let resp = client
            .identity()
            .list_sessions(iam::ListSessionsRequest {
                identity_id: identity_id.to_string(),
                page: MessageField::some(iam::PageRequest {
                    page_size: 200,
                    page_token: page_token.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?
            .into_owned();
        for session in &resp.sessions {
            client
                .identity()
                .delete_session(iam::DeleteSessionRequest {
                    id: session.id.clone(),
                    ..Default::default()
                })
                .await?;
            revoked += 1;
        }
        let next = resp
            .page
            .as_option()
            .map(|p| p.next_page_token.clone())
            .unwrap_or_default();
        if next.is_empty() {
            return Ok(revoked);
        }
        page_token = next;
    }
}

// ---------------------------------------------------------------------------
// Onboard
// ---------------------------------------------------------------------------

/// Send a welcome email via cluster Stalwart SMTP.
async fn send_welcome_email(
    domain: &str,
    email: &str,
    name: &str,
    recovery_link: &str,
    recovery_token: &str,
    job_title: &str,
    department: &str,
) -> Result<()> {
    let greeting = if name.is_empty() {
        "Hi".to_string()
    } else {
        format!("Hi {name}")
    };

    let joining_line = if !job_title.is_empty() && !department.is_empty() {
        format!(" You're joining as {job_title} in the {department} department.")
    } else {
        String::new()
    };

    let body_text = format!(
        "{greeting},

Welcome to Sunbeam Studios!{joining_line} Your account has been created.

To set your password, open this link and enter the recovery token below:

  Link: {recovery_link}
  Token: {recovery_token}

This link expires in 24 hours.

Once signed in you will be prompted to set up 2FA (mandatory).

After that, head to https://sso.{domain}/identity/settings to set up your
profile -- add your name, profile picture, and any other details.

Your services:
  Mail:         https://mail.{domain}
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
        .ctx("Invalid from address")?;
    let to: Mailbox = email.parse().ctx("Invalid recipient address")?;

    let message = Message::builder()
        .from(from)
        .to(to)
        .subject("Welcome to Sunbeam Studios -- Set Your Password")
        .body(body_text)
        .ctx("Failed to build email message")?;

    let pf = PortForward::stalwart_smtp().await?;
    // pf.base_url is "http://127.0.0.1:<port>" — strip to just the port.
    let smtp_port: u16 = pf
        .base_url
        .rsplit(':')
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| SunbeamError::Other("Could not parse SMTP port".into()))?;

    let mailer = SmtpTransport::builder_dangerous("127.0.0.1")
        .port(smtp_port)
        .build();

    tokio::task::spawn_blocking(move || {
        mailer
            .send(&message)
            .ctx("Failed to send welcome email via SMTP")
    })
    .await
    .map_err(|e| SunbeamError::Other(format!("Email send task panicked: {e}")))??;

    tracing::info!("Welcome email sent to {email}");
    Ok(())
}

/// Create the identity (when missing) and mint a fresh recovery link.
/// Returns `(identity_id, recovery_link, is_new)`.
#[allow(clippy::too_many_arguments)]
async fn onboard_identity(
    client: &AuthClient,
    email: &str,
    name: &str,
    schema_id: &str,
    job_title: &str,
    department: &str,
    office_location: &str,
    hire_date: &str,
    manager: &str,
) -> Result<(String, iam::RecoveryLink, bool)> {
    if let Some(existing) = crate::auth::find_identity(client, email).await? {
        let iid = existing.id.clone();
        tracing::info!("Identity already exists: {}...", short_id(&iid));
        tracing::info!("Generating fresh recovery link...");
        let link = generate_recovery(client, &iid).await?;
        return Ok((iid, link, false));
    }

    let mut traits = serde_json::json!({ "email": email });
    if !name.is_empty() {
        let parts: Vec<&str> = name.splitn(2, ' ').collect();
        traits["given_name"] = Value::String(parts[0].to_string());
        traits["family_name"] =
            Value::String(if parts.len() > 1 { parts[1] } else { "" }.to_string());
    }

    let mut employee_id = String::new();
    if schema_id == "employee" {
        employee_id = next_employee_id(client).await?;
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

    let identity = client
        .identity()
        .create_identity(iam::CreateIdentityRequest {
            schema_id: schema_id.to_string(),
            traits: traits_struct(traits),
            ..Default::default()
        })
        .await?
        .into_owned();

    let iid = identity.id.clone();
    tracing::info!("Created identity: {iid}");
    if !employee_id.is_empty() {
        tracing::info!("Employee #{employee_id}");
    }

    let link = generate_recovery(client, &iid).await?;
    Ok((iid, link, true))
}

#[allow(clippy::too_many_arguments)]
/// Cmd user onboard.
#[tracing::instrument]
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
    tracing::info!("Onboarding: {email}");

    let client = identity_client().await?;
    let (iid, link, _is_new) = onboard_identity(
        &client,
        email,
        name,
        schema_id,
        job_title,
        department,
        office_location,
        hire_date,
        manager,
    )
    .await?;

    if send_email {
        let domain = sdk::kube::get_domain().await?;
        let recipient = if notify.is_empty() { email } else { notify };
        send_welcome_email(
            &domain,
            recipient,
            name,
            &link.recovery_link,
            &link.recovery_token,
            job_title,
            department,
        )
        .await?;
    }

    tracing::info!("Identity ID: {iid}");
    print_recovery(&link);
    Ok(())
}

// ---------------------------------------------------------------------------
// Offboard
// ---------------------------------------------------------------------------

/// Cmd user offboard.
///
/// The sso-gateway IAM has no identity-state (disable) RPC, so offboarding is
/// destructive: all sessions are revoked and the identity is deleted.
#[tracing::instrument(skip(target))]
pub async fn cmd_user_offboard(target: &str) -> Result<()> {
    tracing::info!("Offboarding: {target}");

    eprint!("Offboard '{target}'? This revokes all sessions and deletes the identity. [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase() != "y" {
        tracing::info!("Cancelled.");
        return Ok(());
    }

    let client = identity_client().await?;
    let identity = require_identity(&client, target).await?;

    tracing::info!("Revoking sessions...");
    let revoked = revoke_all_sessions(&client, &identity.id).await?;
    tracing::info!("Revoked {revoked} session(s).");

    tracing::info!("Deleting identity...");
    client
        .identity()
        .delete_identity(iam::DeleteIdentityRequest {
            id: identity.id.clone(),
            ..Default::default()
        })
        .await?;

    tracing::info!("Offboarding complete for {}...", short_id(&identity.id));
    tracing::info!("Existing access tokens expire at their issuer TTL.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use chrono::{Duration, Utc};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const ULID: &str = "01HZY9JTKKHK3Y6XJJYHZ9Q5TV";

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
    fn test_display_name_string_name() {
        let traits = serde_json::json!({ "name": "Madonna" });
        assert_eq!(display_name(&traits), "Madonna");
    }

    #[test]
    fn test_short_id() {
        assert_eq!(short_id("01HZY9JTKKHK3Y6XJJYHZ9Q5TV"), "01HZY9JT...");
    }

    #[test]
    fn test_short_id_short() {
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn test_name_traits_split() {
        let t = name_traits("Alice Smith");
        assert_eq!(t["first"], "Alice");
        assert_eq!(t["last"], "Smith");
        let single = name_traits("Madonna");
        assert_eq!(single["first"], "Madonna");
        assert_eq!(single["last"], "");
    }

    #[test]
    fn test_traits_struct_roundtrip() {
        let field = traits_struct(serde_json::json!({ "email": "a@b.com" }));
        let s = field.as_option().expect("traits present");
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(v["email"], "a@b.com");

        // Invalid input (not an object) degrades to none.
        let bad = traits_struct(serde_json::json!("nope"));
        assert!(bad.as_option().is_none());
    }

    // ---------------------------------------------------------------------
    // ConnectRPC-level tests (wiremock)
    // ---------------------------------------------------------------------

    /// Point the CLI at a temp HOME with a valid cached token and the
    /// SUNBEAM_SSO_URL override aimed at the mock server.
    fn setup_session(server: &MockServer) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var(crate::auth::SSO_URL_ENV, server.uri());
        }
        sdk::config::set_active_context(sdk::config::Context {
            domain: "example.com".to_string(),
            ..Default::default()
        });
        sdk::config::set_auth_tokens(
            "example.com",
            &sdk::config::AuthTokens {
                access_token: "test-access".to_string(),
                refresh_token: "test-refresh".to_string(),
                expires_at: Utc::now() + Duration::hours(1),
                id_token: None,
            },
        )
        .unwrap();
        dir
    }

    fn identity(id: &str, email: &str) -> iam::Identity {
        iam::Identity {
            id: id.to_string(),
            tenant_id: "tenant-1".to_string(),
            schema_id: "default".to_string(),
            traits: traits_struct(serde_json::json!({ "email": email })),
            ..Default::default()
        }
    }

    fn employee(id: &str, email: &str, employee_id: &str) -> iam::Identity {
        iam::Identity {
            id: id.to_string(),
            tenant_id: "tenant-1".to_string(),
            schema_id: "employee".to_string(),
            traits: traits_struct(serde_json::json!({
                "email": email,
                "given_name": "Alice",
                "family_name": "Smith",
                "employee_id": employee_id,
            })),
            ..Default::default()
        }
    }

    fn list_response(identities: Vec<iam::Identity>) -> iam::ListIdentitiesResponse {
        iam::ListIdentitiesResponse {
            identities,
            ..Default::default()
        }
    }

    fn recovery_link() -> iam::RecoveryLink {
        iam::RecoveryLink {
            recovery_link: "https://sso.example.com/identity/recovery?flow=f1".to_string(),
            recovery_token: "tok-123".to_string(),
            flow: "f1".to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_cmd_user_list_renders_and_filters() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![
                identity("id-1", "alice@example.com"),
                employee("id-2", "bob@example.com", "7"),
            ])))
            .mount(&server)
            .await;
        let _home = setup_session(&server);

        cmd_user_list("").await.unwrap();
        cmd_user_list("alice").await.unwrap();
        cmd_user_list("no-match").await.unwrap();
    }

    #[tokio::test]
    async fn test_cmd_user_get_by_email() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![identity(
                "id-1",
                "alice@example.com",
            )])))
            .mount(&server)
            .await;
        let _home = setup_session(&server);

        cmd_user_get("alice@example.com").await.unwrap();
    }

    #[tokio::test]
    async fn test_cmd_user_get_not_found_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![])))
            .mount(&server)
            .await;
        let _home = setup_session(&server);

        let err = cmd_user_get("ghost@example.com").await.unwrap_err();
        assert!(err.to_string().contains("Identity not found"), "err: {err}");
    }

    #[tokio::test]
    async fn test_cmd_user_create_mints_recovery_link() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/CreateIdentity"))
            .respond_with(testutil::proto_response(&identity(ULID, "new@example.com")))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/CreateRecoveryLink"))
            .respond_with(testutil::proto_response(&recovery_link()))
            .mount(&server)
            .await;
        let _home = setup_session(&server);

        cmd_user_create("new@example.com", "New User", "default")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_delete_identity_by_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::proto_response(&identity(ULID, "a@example.com")))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/DeleteIdentity"))
            .respond_with(testutil::proto_response(
                &sdk::kanban::prelude::buffa_types::google::protobuf::Empty::default(),
            ))
            .expect(1)
            .mount(&server)
            .await;

        let client = crate::auth::build_auth_client(&server.uri(), Some("t")).unwrap();
        delete_identity(&client, ULID).await.unwrap();
    }

    #[tokio::test]
    async fn test_cmd_user_delete_cancelled_on_empty_stdin() {
        // No mocks at all: the confirmation prompt reads EOF from stdin and
        // cancels before any network call.
        let server = MockServer::start().await;
        let _home = setup_session(&server);
        cmd_user_delete("a@example.com").await.unwrap();
    }

    #[tokio::test]
    async fn test_cmd_user_recover_prints_link() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::proto_response(&identity(ULID, "a@example.com")))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/CreateRecoveryLink"))
            .respond_with(testutil::proto_response(&recovery_link()))
            .expect(1)
            .mount(&server)
            .await;
        let _home = setup_session(&server);

        cmd_user_recover(ULID).await.unwrap();
    }

    #[tokio::test]
    async fn test_revoke_all_sessions_deletes_each() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListSessions"))
            .respond_with(testutil::proto_response(&iam::ListSessionsResponse {
                sessions: vec![
                    iam::Session {
                        id: "s1".to_string(),
                        identity_id: ULID.to_string(),
                        active: true,
                        ..Default::default()
                    },
                    iam::Session {
                        id: "s2".to_string(),
                        identity_id: ULID.to_string(),
                        active: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/DeleteSession"))
            .respond_with(testutil::proto_response(
                &sdk::kanban::prelude::buffa_types::google::protobuf::Empty::default(),
            ))
            .expect(2)
            .mount(&server)
            .await;

        let client = crate::auth::build_auth_client(&server.uri(), Some("t")).unwrap();
        let revoked = revoke_all_sessions(&client, ULID).await.unwrap();
        assert_eq!(revoked, 2);
    }

    #[tokio::test]
    async fn test_next_employee_id_scans_and_increments() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![
                employee("a", "a@x.com", "7"),
                employee("b", "b@x.com", "42"),
                employee("c", "c@x.com", "not-a-number"),
                identity("d", "no-eid@x.com"),
            ])))
            .mount(&server)
            .await;

        let client = crate::auth::build_auth_client(&server.uri(), Some("t")).unwrap();
        let next = next_employee_id(&client).await.unwrap();
        assert_eq!(next, "43");
    }

    #[tokio::test]
    async fn test_next_employee_id_empty_directory_starts_at_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![])))
            .mount(&server)
            .await;

        let client = crate::auth::build_auth_client(&server.uri(), Some("t")).unwrap();
        let next = next_employee_id(&client).await.unwrap();
        assert_eq!(next, "1");
    }

    #[tokio::test]
    async fn test_onboard_identity_creates_employee_and_recovers() {
        let server = MockServer::start().await;
        // find_identity (email lookup) → empty list; next_employee_id → list
        // with employee 41.
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![employee(
                "e1",
                "existing@x.com",
                "41",
            )])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/CreateIdentity"))
            .respond_with(testutil::proto_response(&employee(ULID, "new@x.com", "42")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/CreateRecoveryLink"))
            .respond_with(testutil::proto_response(&recovery_link()))
            .expect(1)
            .mount(&server)
            .await;

        let client = crate::auth::build_auth_client(&server.uri(), Some("t")).unwrap();
        let (iid, link, is_new) = onboard_identity(
            &client,
            "new@x.com",
            "New Person",
            "employee",
            "Dev",
            "Engineering",
            "",
            "",
            "",
        )
        .await
        .unwrap();
        assert_eq!(iid, ULID);
        assert!(is_new);
        assert_eq!(link.recovery_token, "tok-123");
    }

    #[tokio::test]
    async fn test_onboard_identity_existing_refreshes_recovery() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/ListIdentities"))
            .respond_with(testutil::proto_response(&list_response(vec![identity(
                ULID,
                "dup@x.com",
            )])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/CreateRecoveryLink"))
            .respond_with(testutil::proto_response(&recovery_link()))
            .expect(1)
            .mount(&server)
            .await;

        let client = crate::auth::build_auth_client(&server.uri(), Some("t")).unwrap();
        let (iid, _link, is_new) =
            onboard_identity(&client, "dup@x.com", "", "default", "", "", "", "", "")
                .await
                .unwrap();
        assert_eq!(iid, ULID);
        assert!(!is_new);
    }

    #[tokio::test]
    async fn test_cmd_user_offboard_cancelled_on_empty_stdin() {
        let server = MockServer::start().await;
        let _home = setup_session(&server);
        cmd_user_offboard("a@example.com").await.unwrap();
    }
}
