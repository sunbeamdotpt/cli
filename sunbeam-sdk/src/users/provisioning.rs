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
// App-level provisioning (best-effort)
// ---------------------------------------------------------------------------

/// Resolve a deployment to the name of a running pod.
async fn pod_for_deployment(ns: &str, deployment: &str) -> Result<String> {
    let client = crate::kube::get_client().await?;
    let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
        kube::Api::namespaced(client.clone(), ns);

    let label = format!("app.kubernetes.io/name={deployment}");
    let lp = kube::api::ListParams::default().labels(&label);
    let pod_list = pods
        .list(&lp)
        .await
        .with_ctx(|| format!("Failed to list pods for deployment {deployment} in {ns}"))?;

    for pod in &pod_list.items {
        if let Some(status) = &pod.status {
            let phase = status.phase.as_deref().unwrap_or("");
            if phase == "Running" {
                if let Some(name) = &pod.metadata.name {
                    return Ok(name.clone());
                }
            }
        }
    }

    // Fallback: try with app= label
    let label2 = format!("app={deployment}");
    let lp2 = kube::api::ListParams::default().labels(&label2);
    let pod_list2 = match pods.list(&lp2).await {
        Ok(list) => list,
        Err(_) => {
            return Err(SunbeamError::kube(format!(
                "No running pod found for deployment {deployment} in {ns}"
            )));
        }
    };

    for pod in &pod_list2.items {
        if let Some(status) = &pod.status {
            let phase = status.phase.as_deref().unwrap_or("");
            if phase == "Running" {
                if let Some(name) = &pod.metadata.name {
                    return Ok(name.clone());
                }
            }
        }
    }

    Err(SunbeamError::kube(format!(
        "No running pod found for deployment {deployment} in {ns}"
    )))
}

/// Create a mailbox in Messages via kubectl exec into the backend.
async fn create_mailbox(email: &str, name: &str) {
    let parts: Vec<&str> = email.splitn(2, '@').collect();
    if parts.len() != 2 {
        warn(&format!("Invalid email for mailbox creation: {email}"));
        return;
    }
    let local_part = parts[0];
    let domain_part = parts[1];
    let display_name = if name.is_empty() { local_part } else { name };
    let _ = display_name; // used in Python for future features; kept for parity

    step(&format!("Creating mailbox: {email}"));

    let pod = match pod_for_deployment("lasuite", "messages-backend").await {
        Ok(p) => p,
        Err(e) => {
            warn(&format!("Could not find messages-backend pod: {e}"));
            return;
        }
    };

    let script = format!(
        "mb, created = Mailbox.objects.get_or_create(\n    local_part=\"{}\",\n    domain=MailDomain.objects.get(name=\"{}\"),\n)\nprint(\"created\" if created else \"exists\")\n",
        local_part, domain_part,
    );

    let cmd: Vec<&str> = vec!["python", "manage.py", "shell", "-c", &script];
    match crate::kube::kube_exec("lasuite", &pod, &cmd, Some("messages-backend")).await {
        Ok((0, output)) if output.contains("created") => {
            ok(&format!("Mailbox {email} created."));
        }
        Ok((0, output)) if output.contains("exists") => {
            ok(&format!("Mailbox {email} already exists."));
        }
        Ok((_, output)) => {
            warn(&format!(
                "Could not create mailbox (Messages backend may not be running): {output}"
            ));
        }
        Err(e) => {
            warn(&format!("Could not create mailbox: {e}"));
        }
    }
}

/// Delete a mailbox and associated Django user in Messages.
async fn delete_mailbox(email: &str) {
    let parts: Vec<&str> = email.splitn(2, '@').collect();
    if parts.len() != 2 {
        warn(&format!("Invalid email for mailbox deletion: {email}"));
        return;
    }
    let local_part = parts[0];
    let domain_part = parts[1];

    step(&format!("Cleaning up mailbox: {email}"));

    let pod = match pod_for_deployment("lasuite", "messages-backend").await {
        Ok(p) => p,
        Err(e) => {
            warn(&format!("Could not find messages-backend pod: {e}"));
            return;
        }
    };

    let script = format!(
        "from django.contrib.auth import get_user_model\nUser = get_user_model()\ndeleted = 0\nfor mb in Mailbox.objects.filter(local_part=\"{local_part}\", domain__name=\"{domain_part}\"):\n    mb.delete()\n    deleted += 1\ntry:\n    u = User.objects.get(email=\"{email}\")\n    u.delete()\n    deleted += 1\nexcept User.DoesNotExist:\n    pass\nprint(f\"deleted {{deleted}}\")\n",
    );

    let cmd: Vec<&str> = vec!["python", "manage.py", "shell", "-c", &script];
    match crate::kube::kube_exec("lasuite", &pod, &cmd, Some("messages-backend")).await {
        Ok((0, output)) if output.contains("deleted") => {
            ok("Mailbox and user cleaned up.");
        }
        Ok((_, output)) => {
            warn(&format!("Could not clean up mailbox: {output}"));
        }
        Err(e) => {
            warn(&format!("Could not clean up mailbox: {e}"));
        }
    }
}

/// Create a Projects (Planka) user and add them as manager of the Default project.
async fn setup_projects_user(email: &str, name: &str) {
    step(&format!("Setting up Projects user: {email}"));

    let pod = match pod_for_deployment("lasuite", "projects").await {
        Ok(p) => p,
        Err(e) => {
            warn(&format!("Could not find projects pod: {e}"));
            return;
        }
    };

    let js = format!(
        "const knex = require('knex')({{client: 'pg', connection: process.env.DATABASE_URL}});\nasync function go() {{\n  let user = await knex('user_account').where({{email: '{email}'}}).first();\n  if (!user) {{\n    const id = Date.now().toString();\n    await knex('user_account').insert({{\n      id, email: '{email}', name: '{name}', password: '',\n      is_admin: true, is_sso: true, language: 'en-US',\n      created_at: new Date(), updated_at: new Date()\n    }});\n    user = {{id}};\n    console.log('user_created');\n  }} else {{\n    console.log('user_exists');\n  }}\n  const project = await knex('project').where({{name: 'Default'}}).first();\n  if (project) {{\n    const exists = await knex('project_manager').where({{project_id: project.id, user_id: user.id}}).first();\n    if (!exists) {{\n      await knex('project_manager').insert({{\n        id: (Date.now()+1).toString(), project_id: project.id,\n        user_id: user.id, created_at: new Date()\n      }});\n      console.log('manager_added');\n    }} else {{\n      console.log('manager_exists');\n    }}\n  }} else {{\n    console.log('no_default_project');\n  }}\n}}\ngo().then(() => process.exit(0)).catch(e => {{ console.error(e.message); process.exit(1); }});\n",
    );

    let cmd: Vec<&str> = vec!["node", "-e", &js];
    match crate::kube::kube_exec("lasuite", &pod, &cmd, Some("projects")).await {
        Ok((0, output))
            if output.contains("manager_added") || output.contains("manager_exists") =>
        {
            ok("Projects user ready.");
        }
        Ok((0, output)) if output.contains("no_default_project") => {
            warn("No Default project found in Projects -- skip.");
        }
        Ok((_, output)) => {
            warn(&format!("Could not set up Projects user: {output}"));
        }
        Err(e) => {
            warn(&format!("Could not set up Projects user: {e}"));
        }
    }
}

/// Remove a user from Projects (Planka) -- delete memberships and soft-delete user.
async fn cleanup_projects_user(email: &str) {
    step(&format!("Cleaning up Projects user: {email}"));

    let pod = match pod_for_deployment("lasuite", "projects").await {
        Ok(p) => p,
        Err(e) => {
            warn(&format!("Could not find projects pod: {e}"));
            return;
        }
    };

    let js = format!(
        "const knex = require('knex')({{client: 'pg', connection: process.env.DATABASE_URL}});\nasync function go() {{\n  const user = await knex('user_account').where({{email: '{email}'}}).first();\n  if (!user) {{ console.log('not_found'); return; }}\n  await knex('board_membership').where({{user_id: user.id}}).del();\n  await knex('project_manager').where({{user_id: user.id}}).del();\n  await knex('user_account').where({{id: user.id}}).update({{deleted_at: new Date()}});\n  console.log('cleaned');\n}}\ngo().then(() => process.exit(0)).catch(e => {{ console.error(e.message); process.exit(1); }});\n",
    );

    let cmd: Vec<&str> = vec!["node", "-e", &js];
    match crate::kube::kube_exec("lasuite", &pod, &cmd, Some("projects")).await {
        Ok((0, output)) if output.contains("cleaned") => {
            ok("Projects user cleaned up.");
        }
        Ok((_, output)) => {
            warn(&format!("Could not clean up Projects user: {output}"));
        }
        Err(e) => {
            warn(&format!("Could not clean up Projects user: {e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Onboard
// ---------------------------------------------------------------------------

/// Send a welcome email via cluster Postfix (port-forward to svc/postfix in lasuite).
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

    let _pf = PortForward::new("lasuite", "postfix", SMTP_LOCAL_PORT, 25).await?;

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

    let (iid, recovery_link, recovery_code, is_new) = {
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

    // Provision app-level accounts for new users
    if is_new {
        create_mailbox(email, name).await;
        setup_projects_user(email, name).await;
    }

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

    // Clean up Messages mailbox and Projects user
    let email = identity
        .get("traits")
        .and_then(|t| t.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !email.is_empty() {
        delete_mailbox(email).await;
        cleanup_projects_user(email).await;
    }

    ok(&format!("Offboarding complete for {}...", short_id(&iid)));
    warn("Existing access tokens expire within ~1h (Hydra TTL).");
    warn("App sessions (docs/people) expire within SESSION_COOKIE_AGE (~1h).");
    Ok(())
}
