//! User provisioning -- onboarding and offboarding workflows.

use serde_json::Value;
use std::io::Write;

use crate::error::{Result, ResultExt, SunbeamError};
use crate::output::{ok, step, warn};

use super::{
    api, find_identity, generate_recovery, identity_id, identity_put_body, kratos_api,
    next_employee_id, short_id, PortForward, SMTP_LOCAL_PORT,
};

// ---------------------------------------------------------------------------
// Onboard
// ---------------------------------------------------------------------------

/// Send a welcome email via cluster Stalwart (port-forward to svc/stalwart in stalwart ns).
async fn send_welcome_email(
    domain: &str,
    email: &str,
    name: &str,
    recovery_link: &str,
    recovery_code: &str,
    job_title: &str,
    department: &str,
) -> Result<()> {
    let greeting = if name.is_empty() {
        "Hi".to_string()
    } else {
        format!("Hi {name}")
    };

    let joining_line = if !job_title.is_empty() && !department.is_empty() {
        format!(
            " You're joining as {job_title} in the {department} department."
        )
    } else {
        String::new()
    };

    let body_text = format!(
        "{greeting},

Welcome to Sunbeam Studios!{joining_line} Your account has been created.

To set your password, open this link and enter the recovery code below:

  Link: {recovery_link}
  Code: {recovery_code}

This link expires in 24 hours.

Once signed in you will be prompted to set up 2FA (mandatory).

After that, head to https://auth.{domain}/settings to set up your
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
        .map_err(|e| SunbeamError::Other(format!("Invalid from address: {e}")))?;
    let to: Mailbox = email
        .parse()
        .map_err(|e| SunbeamError::Other(format!("Invalid recipient address: {e}")))?;

    let message = Message::builder()
        .from(from)
        .to(to)
        .subject("Welcome to Sunbeam Studios -- Set Your Password")
        .body(body_text)
        .ctx("Failed to build email message")?;

    let _pf = PortForward::new("stalwart", "stalwart", SMTP_LOCAL_PORT, 25).await?;

    let mailer = SmtpTransport::builder_dangerous("localhost")
        .port(SMTP_LOCAL_PORT)
        .build();

    tokio::task::spawn_blocking(move || {
        mailer
            .send(&message)
            .map_err(|e| SunbeamError::Other(format!("Failed to send welcome email via SMTP: {e}")))
    })
    .await
    .map_err(|e| SunbeamError::Other(format!("Email send task panicked: {e}")))??;

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

    let pf = PortForward::kratos().await?;

    let (iid, recovery_link, recovery_code, _is_new) = {
        let existing = find_identity(&pf.base_url, email, false).await?;

        if let Some(existing) = existing {
            let iid = identity_id(&existing)?;
            warn(&format!("Identity already exists: {}...", short_id(&iid)));
            step("Generating fresh recovery link...");
            let (link, code) = generate_recovery(&pf.base_url, &iid).await?;
            (iid, link, code, false)
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
                employee_id = next_employee_id(&pf.base_url).await?;
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

            let identity = kratos_api(&pf.base_url, "/identities", "POST", Some(&body), &[])
                .await?
                .ok_or_else(|| SunbeamError::identity("Failed to create identity"))?;

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
            )
            .await?;

            let (link, code) = generate_recovery(&pf.base_url, &iid).await?;
            (iid, link, code, true)
        }
    };

    drop(pf);

    if send_email {
        let domain = crate::kube::get_domain().await?;
        let recipient = if notify.is_empty() { email } else { notify };
        send_welcome_email(
            &domain, recipient, name, &recovery_link, &recovery_code,
            job_title, department,
        )
        .await?;
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

    let pf = PortForward::kratos().await?;
    let identity = find_identity(&pf.base_url, target, true)
        .await?
        .ok_or_else(|| SunbeamError::identity("Identity not found"))?;
    let iid = identity_id(&identity)?;

    step("Disabling identity...");
    let put_body = identity_put_body(&identity, Some("inactive"), None);
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}"),
        "PUT",
        Some(&put_body),
        &[],
    )
    .await?;
    ok(&format!("Identity {}... disabled.", short_id(&iid)));

    step("Revoking Kratos sessions...");
    kratos_api(
        &pf.base_url,
        &format!("/identities/{iid}/sessions"),
        "DELETE",
        None,
        &[404],
    )
    .await?;
    ok("Kratos sessions revoked.");

    step("Revoking Hydra consent sessions...");
    {
        let hydra_pf = PortForward::new("ory", "hydra-admin", 14445, 4445).await?;
        api(
            &hydra_pf.base_url,
            &format!("/oauth2/auth/sessions/consent?subject={iid}&all=true"),
            "DELETE",
            None,
            "/admin",
            &[404],
        )
        .await?;
    }
    ok("Hydra consent sessions revoked.");

    drop(pf);

    ok(&format!("Offboarding complete for {}...", short_id(&iid)));
    warn("Existing access tokens expire within ~1h (Hydra TTL).");
    warn("App sessions expire within SESSION_COOKIE_AGE (~1h).");
    Ok(())
}
