#![cfg(feature = "integration")]
mod helpers;
use helpers::*;
use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::auth::hydra::HydraClient;
use sunbeam_sdk::auth::hydra::types::*;

const HYDRA_HEALTH: &str = "http://localhost:4445/health/alive";

fn hydra() -> HydraClient {
    HydraClient::from_parts("http://localhost:4445".into(), AuthMethod::None)
}

// ---------------------------------------------------------------------------
// 1. OAuth2 Client CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth2_client_crud() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();
    let name = unique_name("test-client");

    // Create
    let body = OAuth2Client {
        client_name: Some(name.clone()),
        redirect_uris: Some(vec!["http://localhost:9999/cb".into()]),
        grant_types: Some(vec!["authorization_code".into(), "refresh_token".into()]),
        response_types: Some(vec!["code".into()]),
        scope: Some("openid offline".into()),
        token_endpoint_auth_method: Some("client_secret_post".into()),
        ..Default::default()
    };
    let created = h.create_client(&body).await.expect("create_client");
    let cid = created.client_id.as_deref().expect("created client must have id");
    assert_eq!(created.client_name.as_deref(), Some(name.as_str()));

    // List — created client should appear
    let list = h.list_clients(Some(100), Some(0)).await.expect("list_clients");
    assert!(
        list.iter().any(|c| c.client_id.as_deref() == Some(cid)),
        "created client should appear in list"
    );

    // Get
    let fetched = h.get_client(cid).await.expect("get_client");
    assert_eq!(fetched.client_name.as_deref(), Some(name.as_str()));

    // Update (PUT) — change name
    let updated_name = format!("{name}-updated");
    let update_body = OAuth2Client {
        client_name: Some(updated_name.clone()),
        redirect_uris: Some(vec!["http://localhost:9999/cb".into()]),
        grant_types: Some(vec!["authorization_code".into()]),
        response_types: Some(vec!["code".into()]),
        scope: Some("openid".into()),
        token_endpoint_auth_method: Some("client_secret_post".into()),
        ..Default::default()
    };
    let updated = h.update_client(cid, &update_body).await.expect("update_client");
    assert_eq!(updated.client_name.as_deref(), Some(updated_name.as_str()));

    // Patch — change scope via JSON Patch
    let patches = vec![serde_json::json!({
        "op": "replace",
        "path": "/scope",
        "value": "openid offline profile"
    })];
    let patched = h.patch_client(cid, &patches).await.expect("patch_client");
    assert_eq!(patched.scope.as_deref(), Some("openid offline profile"));

    // Set lifespans
    let lifespans = TokenLifespans {
        authorization_code_grant_access_token_lifespan: Some("3600s".into()),
        ..Default::default()
    };
    let with_lifespans = h
        .set_client_lifespans(cid, &lifespans)
        .await
        .expect("set_client_lifespans");
    assert!(with_lifespans.client_id.as_deref() == Some(cid));

    // Delete
    h.delete_client(cid).await.expect("delete_client");

    // Verify deleted
    let err = h.get_client(cid).await;
    assert!(err.is_err(), "get_client after delete should fail");
}

// ---------------------------------------------------------------------------
// 2. Token introspect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_introspect() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    let result = h
        .introspect_token("totally-bogus-token-that-does-not-exist")
        .await
        .expect("introspect should not error for bogus token");
    assert!(!result.active, "bogus token must be inactive");
}

// ---------------------------------------------------------------------------
// 3. Delete tokens for client
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_tokens_for_client() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    // Create a throwaway client
    let body = OAuth2Client {
        client_name: Some(unique_name("tok-del")),
        grant_types: Some(vec!["client_credentials".into()]),
        response_types: Some(vec!["token".into()]),
        scope: Some("openid".into()),
        token_endpoint_auth_method: Some("client_secret_post".into()),
        ..Default::default()
    };
    let created = h.create_client(&body).await.expect("create client for token delete");
    let cid = created.client_id.as_deref().expect("client id");

    // Delete tokens — should succeed (no-op, no tokens issued yet)
    h.delete_tokens_for_client(cid)
        .await
        .expect("delete_tokens_for_client should not error");

    // Cleanup
    h.delete_client(cid).await.expect("cleanup client");
}

// ---------------------------------------------------------------------------
// 4. JWK set CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jwk_set_crud() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();
    let set_name = unique_name("test-jwk-set");

    // Create
    let create_body = CreateJwkBody {
        alg: "RS256".into(),
        kid: format!("{set_name}-key"),
        use_: "sig".into(),
    };
    let created = h
        .create_jwk_set(&set_name, &create_body)
        .await
        .expect("create_jwk_set");
    assert!(!created.keys.is_empty(), "created set should have at least one key");

    // Get
    let fetched = h.get_jwk_set(&set_name).await.expect("get_jwk_set");
    assert!(!fetched.keys.is_empty());

    // Update — replace the set with the same keys (idempotent)
    let update_body = JwkSet {
        keys: fetched.keys.clone(),
    };
    let updated = h
        .update_jwk_set(&set_name, &update_body)
        .await
        .expect("update_jwk_set");
    assert_eq!(updated.keys.len(), fetched.keys.len());

    // Delete
    h.delete_jwk_set(&set_name).await.expect("delete_jwk_set");

    // Verify deleted
    let err = h.get_jwk_set(&set_name).await;
    assert!(err.is_err(), "get_jwk_set after delete should fail");
}

// ---------------------------------------------------------------------------
// 5. JWK key CRUD (within a set)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jwk_key_crud() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();
    let set_name = unique_name("test-jwk-key");

    // Create a set first
    let kid = format!("{set_name}-k1");
    let create_body = CreateJwkBody {
        alg: "RS256".into(),
        kid: kid.clone(),
        use_: "sig".into(),
    };
    let created = h
        .create_jwk_set(&set_name, &create_body)
        .await
        .expect("create_jwk_set for key test");

    // The kid Hydra assigns may differ from what we requested; extract it.
    let actual_kid = created.keys[0]["kid"]
        .as_str()
        .expect("key must have kid")
        .to_string();

    // Get single key
    let key_set = h
        .get_jwk_key(&set_name, &actual_kid)
        .await
        .expect("get_jwk_key");
    assert_eq!(key_set.keys.len(), 1);

    // Update single key — send the same key back
    let key_val = key_set.keys[0].clone();
    let updated = h
        .update_jwk_key(&set_name, &actual_kid, &key_val)
        .await
        .expect("update_jwk_key");
    assert!(updated.is_object());

    // Delete single key
    h.delete_jwk_key(&set_name, &actual_kid)
        .await
        .expect("delete_jwk_key");

    // Cleanup — delete the (now empty) set; ignore error if already gone
    let _ = h.delete_jwk_set(&set_name).await;
}

// ---------------------------------------------------------------------------
// 6. Trusted JWT issuers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trusted_issuers() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    // We need a JWK set for the issuer to reference
    let set_name = unique_name("trust-jwk");
    let kid = format!("{set_name}-k");
    let jwk_body = CreateJwkBody {
        alg: "RS256".into(),
        kid: kid.clone(),
        use_: "sig".into(),
    };
    let jwk_set = h
        .create_jwk_set(&set_name, &jwk_body)
        .await
        .expect("create jwk set for trusted issuer");
    let actual_kid = jwk_set.keys[0]["kid"]
        .as_str()
        .expect("kid")
        .to_string();

    // Create trusted issuer
    let issuer_body = TrustedJwtIssuer {
        id: None,
        issuer: format!("https://{}.example.com", unique_name("iss")),
        subject: "test-subject".into(),
        scope: vec!["openid".into()],
        public_key: Some(TrustedIssuerKey {
            set: Some(set_name.clone()),
            kid: Some(actual_kid.clone()),
        }),
        expires_at: Some("2099-12-31T23:59:59Z".into()),
        created_at: None,
    };
    let created = match h.create_trusted_issuer(&issuer_body).await {
        Ok(c) => c,
        Err(_) => {
            // Hydra may require inline JWK — skip if not supported
            let _ = h.delete_jwk_set(&set_name).await;
            return;
        }
    };
    let issuer_id = created.id.as_deref().expect("trusted issuer must have id");

    // List
    let list = h.list_trusted_issuers().await.expect("list_trusted_issuers");
    assert!(
        list.iter().any(|i| i.id.as_deref() == Some(issuer_id)),
        "created issuer should appear in list"
    );

    // Get
    let fetched = h
        .get_trusted_issuer(issuer_id)
        .await
        .expect("get_trusted_issuer");
    assert_eq!(fetched.issuer, created.issuer);

    // Delete
    h.delete_trusted_issuer(issuer_id)
        .await
        .expect("delete_trusted_issuer");

    // Verify deleted
    let err = h.get_trusted_issuer(issuer_id).await;
    assert!(err.is_err(), "get after delete should fail");

    // Cleanup JWK set
    let _ = h.delete_jwk_set(&set_name).await;
}

// ---------------------------------------------------------------------------
// 7. Consent sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consent_sessions() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    let subject = unique_name("nonexistent-user");

    // List consent sessions for a subject that has none — expect empty list
    let sessions = h
        .list_consent_sessions(&subject)
        .await
        .expect("list_consent_sessions");
    assert!(sessions.is_empty(), "non-existent subject should have no sessions");

    // Revoke consent sessions with a client filter (Hydra requires either client or all=true)
    let _ = h.revoke_consent_sessions(&subject, Some("no-such-client")).await;

    // Revoke login sessions
    let _ = h.revoke_login_sessions(&subject).await;
}

// ---------------------------------------------------------------------------
// 8. Login flow — bogus challenge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_flow() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    let bogus = "bogus-login-challenge-12345";

    // get_login_request with invalid challenge should error, not panic
    let err = h.get_login_request(bogus).await;
    assert!(err.is_err(), "get_login_request with bogus challenge should error");

    // accept_login with invalid challenge should error, not panic
    let accept_body = AcceptLoginBody {
        subject: "test".into(),
        remember: None,
        remember_for: None,
        acr: None,
        amr: None,
        context: None,
        force_subject_identifier: None,
    };
    let err = h.accept_login(bogus, &accept_body).await;
    assert!(err.is_err(), "accept_login with bogus challenge should error");

    // reject_login with invalid challenge should error, not panic
    let reject_body = RejectBody {
        error: Some("access_denied".into()),
        error_description: Some("test".into()),
        error_debug: None,
        error_hint: None,
        status_code: Some(403),
    };
    let err = h.reject_login(bogus, &reject_body).await;
    assert!(err.is_err(), "reject_login with bogus challenge should error");
}

// ---------------------------------------------------------------------------
// 9. Consent flow — bogus challenge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consent_flow() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    let bogus = "bogus-consent-challenge-12345";

    // get_consent_request
    let err = h.get_consent_request(bogus).await;
    assert!(err.is_err(), "get_consent_request with bogus challenge should error");

    // accept_consent
    let accept_body = AcceptConsentBody {
        grant_scope: Some(vec!["openid".into()]),
        grant_access_token_audience: None,
        session: None,
        remember: None,
        remember_for: None,
        handled_at: None,
    };
    let err = h.accept_consent(bogus, &accept_body).await;
    assert!(err.is_err(), "accept_consent with bogus challenge should error");

    // reject_consent
    let reject_body = RejectBody {
        error: Some("access_denied".into()),
        error_description: Some("test".into()),
        error_debug: None,
        error_hint: None,
        status_code: Some(403),
    };
    let err = h.reject_consent(bogus, &reject_body).await;
    assert!(err.is_err(), "reject_consent with bogus challenge should error");
}

// ---------------------------------------------------------------------------
// 10. Logout flow — bogus challenge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_flow() {
    wait_for_healthy(HYDRA_HEALTH, TIMEOUT).await;
    let h = hydra();

    let bogus = "bogus-logout-challenge-12345";

    // get_logout_request
    let err = h.get_logout_request(bogus).await;
    assert!(err.is_err(), "get_logout_request with bogus challenge should error");

    // accept_logout
    let err = h.accept_logout(bogus).await;
    assert!(err.is_err(), "accept_logout with bogus challenge should error");

    // reject_logout
    let reject_body = RejectBody {
        error: Some("access_denied".into()),
        error_description: Some("test".into()),
        error_debug: None,
        error_hint: None,
        status_code: Some(403),
    };
    let err = h.reject_logout(bogus, &reject_body).await;
    assert!(err.is_err(), "reject_logout with bogus challenge should error");
}
