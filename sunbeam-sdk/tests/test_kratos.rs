#![cfg(feature = "integration")]
mod helpers;
use helpers::*;
use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::identity::KratosClient;
use sunbeam_sdk::identity::types::*;

const KRATOS_URL: &str = "http://localhost:4434";
const HEALTH_URL: &str = "http://localhost:4434/health/alive";

fn kratos_client() -> KratosClient {
    KratosClient::from_parts(KRATOS_URL.into(), AuthMethod::None)
}

fn make_create_body(email: &str) -> CreateIdentityBody {
    CreateIdentityBody {
        schema_id: "default".into(),
        traits: serde_json::json!({ "email": email }),
        state: Some("active".into()),
        metadata_public: None,
        metadata_admin: None,
        credentials: None,
        verifiable_addresses: None,
        recovery_addresses: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let alive = client.alive().await.expect("alive failed");
    assert_eq!(alive.status, "ok");

    let ready = client.ready().await.expect("ready failed");
    assert_eq!(ready.status, "ok");
}

// ---------------------------------------------------------------------------
// 2. Identity CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identity_crud() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();
    let email = format!("{}@test.local", unique_name("crud"));

    // Create
    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create_identity failed");
    assert_eq!(created.schema_id, "default");
    assert_eq!(created.traits["email"], email);
    let id = created.id.clone();

    // Get
    let fetched = client.get_identity(&id).await.expect("get_identity failed");
    assert_eq!(fetched.id, id);
    assert_eq!(fetched.traits["email"], email);

    // List (should contain our identity)
    let list = client
        .list_identities(Some(1), Some(100))
        .await
        .expect("list_identities failed");
    assert!(
        list.iter().any(|i| i.id == id),
        "created identity not found in list"
    );

    // Update (full replace)
    let new_email = format!("{}@test.local", unique_name("updated"));
    let update_body = UpdateIdentityBody {
        schema_id: "default".into(),
        traits: serde_json::json!({ "email": new_email }),
        state: "active".into(),
        metadata_public: None,
        metadata_admin: None,
        credentials: None,
    };
    let updated = client
        .update_identity(&id, &update_body)
        .await
        .expect("update_identity failed");
    assert_eq!(updated.traits["email"], new_email);

    // Patch (partial update via JSON Patch)
    let patch_email = format!("{}@test.local", unique_name("patched"));
    let patches = vec![serde_json::json!({
        "op": "replace",
        "path": "/traits/email",
        "value": patch_email,
    })];
    let patched = client
        .patch_identity(&id, &patches)
        .await
        .expect("patch_identity failed");
    assert_eq!(patched.traits["email"], patch_email);

    // Delete
    client
        .delete_identity(&id)
        .await
        .expect("delete_identity failed");

    // Confirm deletion
    let err = client.get_identity(&id).await;
    assert!(err.is_err(), "expected 404 after deletion");
}

// ---------------------------------------------------------------------------
// 3. Identity by credential identifier
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identity_by_credential() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();
    let email = format!("{}@test.local", unique_name("cred"));

    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");
    let id = created.id.clone();

    let results = client
        .get_by_credential_identifier(&email)
        .await
        .expect("get_by_credential_identifier failed");
    assert!(
        results.iter().any(|i| i.id == id),
        "identity not found by credential identifier"
    );

    // Cleanup
    client.delete_identity(&id).await.expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 4. Delete credential (OIDC — may 404, which is fine)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identity_delete_credential() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();
    let email = format!("{}@test.local", unique_name("delcred"));

    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");
    let id = created.id.clone();

    // Attempt to delete an OIDC credential — the identity has none, so a 404
    // or similar error is acceptable.
    let result = client.delete_credential(&id, "oidc").await;
    // We don't assert success; a 404 is the expected outcome for identities
    // without OIDC credentials.
    if let Err(ref e) = result {
        let msg = format!("{e}");
        assert!(
            msg.contains("404") || msg.contains("Not Found") || msg.contains("does not have"),
            "unexpected error: {msg}"
        );
    }

    // Cleanup
    client.delete_identity(&id).await.expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 5. Batch patch identities
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_patch() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let email_a = format!("{}@test.local", unique_name("batch-a"));
    let email_b = format!("{}@test.local", unique_name("batch-b"));

    let body = BatchPatchIdentitiesBody {
        identities: vec![
            BatchPatchEntry {
                create: Some(make_create_body(&email_a)),
                patch_id: Some("00000000-0000-0000-0000-000000000001".into()),
            },
            BatchPatchEntry {
                create: Some(make_create_body(&email_b)),
                patch_id: Some("00000000-0000-0000-0000-000000000002".into()),
            },
        ],
    };

    let result = client
        .batch_patch_identities(&body)
        .await
        .expect("batch_patch_identities failed");
    assert_eq!(result.identities.len(), 2, "expected 2 results");

    // Cleanup created identities
    for entry in &result.identities {
        if let Some(ref identity_id) = entry.identity {
            let _ = client.delete_identity(identity_id).await;
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Sessions (global list — may be empty)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sessions() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // Create an identity so at least the endpoint is exercised.
    let email = format!("{}@test.local", unique_name("sess"));
    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");

    let sessions = client
        .list_sessions(Some(10), None, None)
        .await
        .expect("list_sessions failed");
    // An empty list is acceptable — no sessions exist until a login flow runs.
    assert!(sessions.len() <= 10);

    // Cleanup
    client
        .delete_identity(&created.id)
        .await
        .expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 7. Identity sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identity_sessions() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let email = format!("{}@test.local", unique_name("idsess"));
    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");
    let id = created.id.clone();

    // List sessions for this identity — expect empty.
    let sessions = client
        .list_identity_sessions(&id)
        .await;
    // Kratos may return 404 when there are no sessions, or an empty list.
    match sessions {
        Ok(list) => assert!(list.is_empty(), "expected no sessions for new identity"),
        Err(ref e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains("404") || msg.contains("Not Found"),
                "unexpected error listing identity sessions: {msg}"
            );
        }
    }

    // Delete sessions for this identity (no-op if none exist, may 404).
    let del = client.delete_identity_sessions(&id).await;
    if let Err(ref e) = del {
        let msg = format!("{e}");
        assert!(
            msg.contains("404") || msg.contains("Not Found"),
            "unexpected error deleting identity sessions: {msg}"
        );
    }

    // Cleanup
    client.delete_identity(&id).await.expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 8. Recovery (code + link)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let email = format!("{}@test.local", unique_name("recov"));
    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");
    let id = created.id.clone();

    // Recovery code
    let code_result = client
        .create_recovery_code(&id, Some("1h"))
        .await
        .expect("create_recovery_code failed");
    assert!(
        !code_result.recovery_link.is_empty(),
        "recovery_link should not be empty"
    );
    assert!(
        !code_result.recovery_code.is_empty(),
        "recovery_code should not be empty"
    );

    // Recovery link (may be disabled in dev mode — handle gracefully)
    match client.create_recovery_link(&id, Some("1h")).await {
        Ok(link_result) => {
            assert!(!link_result.recovery_link.is_empty());
        }
        Err(_) => {
            // Endpoint disabled in dev mode — acceptable
        }
    }

    // Cleanup
    client.delete_identity(&id).await.expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 9. Schemas
// ---------------------------------------------------------------------------

#[tokio::test]
async fn schemas() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // List schemas
    let schemas = client.list_schemas().await.expect("list_schemas failed");
    assert!(!schemas.is_empty(), "expected at least one schema");
    assert!(
        schemas.iter().any(|s| s.id == "default"),
        "expected a 'default' schema in the list"
    );

    // Get specific schema
    let schema = client
        .get_schema("default")
        .await
        .expect("get_schema(\"default\") failed");
    // The schema is a JSON Schema document; verify it has basic structure.
    assert!(
        schema.get("properties").is_some() || schema.get("type").is_some(),
        "schema JSON should contain 'properties' or 'type'"
    );
}

// ---------------------------------------------------------------------------
// 10. Courier messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn courier() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // List courier messages (may be empty).
    let messages = client
        .list_courier_messages(Some(10), None)
        .await
        .expect("list_courier_messages failed");

    // If there are messages, fetch the first one by ID.
    if let Some(first) = messages.first() {
        let msg = client
            .get_courier_message(&first.id)
            .await
            .expect("get_courier_message failed");
        assert_eq!(msg.id, first.id);
    }
}

// ---------------------------------------------------------------------------
// 11. Get identity — nonexistent ID
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_identity_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let result = client
        .get_identity("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(result.is_err(), "expected error for nonexistent identity");
}

// ---------------------------------------------------------------------------
// 12. Extend session — nonexistent session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extend_session_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // Extending a session that doesn't exist should fail
    let result = client
        .extend_session("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(result.is_err(), "expected error for nonexistent session");
}

// ---------------------------------------------------------------------------
// 13. Disable session — nonexistent session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disable_session_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let result = client
        .disable_session("00000000-0000-0000-0000-000000000000")
        .await;
    // Kratos may return 404 or similar for nonexistent sessions
    assert!(result.is_err(), "expected error for nonexistent session");
}

// ---------------------------------------------------------------------------
// 14. Get session — nonexistent session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_session_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let result = client
        .get_session("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(result.is_err(), "expected error for nonexistent session");
}

// ---------------------------------------------------------------------------
// 15. List sessions with active filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_sessions_active_filter() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // List sessions filtering by active=true
    let sessions = client
        .list_sessions(Some(10), None, Some(true))
        .await
        .expect("list_sessions with active=true failed");
    assert!(sessions.len() <= 10);

    // List sessions filtering by active=false
    let inactive = client
        .list_sessions(Some(10), None, Some(false))
        .await
        .expect("list_sessions with active=false failed");
    assert!(inactive.len() <= 10);
}

// ---------------------------------------------------------------------------
// 16. Patch identity — add metadata
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_identity_metadata() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();
    let email = format!("{}@test.local", unique_name("patchmeta"));

    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");
    let id = created.id.clone();

    // Patch: add metadata_public
    let patches = vec![serde_json::json!({
        "op": "add",
        "path": "/metadata_public",
        "value": { "role": "admin" },
    })];
    let patched = client
        .patch_identity(&id, &patches)
        .await
        .expect("patch_identity with metadata failed");
    assert_eq!(patched.metadata_public.as_ref().unwrap()["role"], "admin");

    // Cleanup
    client.delete_identity(&id).await.expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 17. List identities with default pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_identities_defaults() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // Use None for both params to exercise default path
    let list = client
        .list_identities(None, None)
        .await
        .expect("list_identities with defaults failed");
    // Just verify it returns a valid vec
    let _ = list;
}

// ---------------------------------------------------------------------------
// 18. Recovery code with default expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery_code_default_expiry() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();
    let email = format!("{}@test.local", unique_name("recovdef"));

    let created = client
        .create_identity(&make_create_body(&email))
        .await
        .expect("create failed");
    let id = created.id.clone();

    // Recovery code with None expiry (exercises default path)
    let code_result = client
        .create_recovery_code(&id, None)
        .await
        .expect("create_recovery_code with default expiry failed");
    assert!(!code_result.recovery_link.is_empty());
    assert!(!code_result.recovery_code.is_empty());

    // Cleanup
    client.delete_identity(&id).await.expect("cleanup failed");
}

// ---------------------------------------------------------------------------
// 19. Get courier message — nonexistent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_courier_message_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let result = client
        .get_courier_message("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(result.is_err(), "expected error for nonexistent courier message");
}

// ---------------------------------------------------------------------------
// 20. Get schema — nonexistent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_schema_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let result = client.get_schema("nonexistent-schema-zzz").await;
    assert!(result.is_err(), "expected error for nonexistent schema");
}

// ---------------------------------------------------------------------------
// 21. Patch identity — nonexistent ID
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_identity_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let patches = vec![serde_json::json!({
        "op": "replace",
        "path": "/traits/email",
        "value": "gone@test.local",
    })];
    let result = client
        .patch_identity("00000000-0000-0000-0000-000000000000", &patches)
        .await;
    assert!(result.is_err(), "expected error for nonexistent identity");
}

// ---------------------------------------------------------------------------
// 22. Delete identity — nonexistent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_identity_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let result = client
        .delete_identity("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(result.is_err(), "expected error for nonexistent identity");
}

// ---------------------------------------------------------------------------
// 23. Update identity — nonexistent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_identity_nonexistent() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    let update_body = UpdateIdentityBody {
        schema_id: "default".into(),
        traits: serde_json::json!({ "email": "gone@test.local" }),
        state: "active".into(),
        metadata_public: None,
        metadata_admin: None,
        credentials: None,
    };
    let result = client
        .update_identity("00000000-0000-0000-0000-000000000000", &update_body)
        .await;
    assert!(result.is_err(), "expected error for nonexistent identity");
}

// ---------------------------------------------------------------------------
// 24. List courier messages with page_token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_courier_messages_with_token() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = kratos_client();

    // List with a page_token — Kratos expects base64 encoded tokens.
    // Use an invalid token to exercise the code path; expect an error.
    let result = client.list_courier_messages(Some(5), Some("invalid-token")).await;
    assert!(result.is_err(), "invalid page_token should produce an error");
}

// ---------------------------------------------------------------------------
// 25. Connect constructor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connect_constructor() {
    let client = KratosClient::connect("example.com");
    assert_eq!(client.base_url(), "https://id.example.com");
    assert_eq!(client.service_name(), "kratos");
}
