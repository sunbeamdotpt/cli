//! Shared test helpers for integration tests.

#![allow(dead_code)]

/// Poll a URL until it returns 200, or panic after `timeout`.
pub async fn wait_for_healthy(url: &str, timeout: std::time::Duration) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() > deadline {
            panic!("Service at {url} did not become healthy within {timeout:?}");
        }
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// Gitea bootstrap constants
pub const GITEA_ADMIN_USER: &str = "testadmin";
pub const GITEA_ADMIN_PASS: &str = "testpass123";
pub const GITEA_ADMIN_EMAIL: &str = "admin@test.local";

/// Bootstrap Gitea admin user + PAT. Returns the PAT string.
pub async fn setup_gitea_pat() -> String {
    wait_for_healthy("http://localhost:3000/api/v1/version", TIMEOUT).await;
    let http = reqwest::Client::new();

    // Register user via public API
    let _ = http
        .post("http://localhost:3000/user/sign_up")
        .form(&[
            ("user_name", GITEA_ADMIN_USER),
            ("password", GITEA_ADMIN_PASS),
            ("retype", GITEA_ADMIN_PASS),
            ("email", GITEA_ADMIN_EMAIL),
        ])
        .send()
        .await;

    // Create PAT using basic auth
    let resp = http
        .post(format!(
            "http://localhost:3000/api/v1/users/{GITEA_ADMIN_USER}/tokens"
        ))
        .basic_auth(GITEA_ADMIN_USER, Some(GITEA_ADMIN_PASS))
        .json(&serde_json::json!({
            "name": format!("test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()),
            "scopes": ["all"]
        }))
        .send()
        .await
        .unwrap();

    if !resp.status().is_success() {
        panic!(
            "PAT creation failed: {}",
            resp.text().await.unwrap_or_default()
        );
    }

    let body: serde_json::Value = resp.json().await.unwrap();
    body["sha1"]
        .as_str()
        .or_else(|| body["token"].as_str())
        .expect("PAT response missing sha1/token field")
        .to_string()
}

/// Generate a LiveKit JWT for testing.
pub fn livekit_test_token() -> String {
    use sunbeam_sdk::media::types::VideoGrants;
    use sunbeam_sdk::media::LiveKitClient;
    let grants = VideoGrants {
        room_create: Some(true),
        room_list: Some(true),
        room_join: Some(true),
        can_publish: Some(true),
        can_subscribe: Some(true),
        can_publish_data: Some(true),
        room_admin: Some(true),
        room_record: Some(true),
        room: None,
    };
    LiveKitClient::generate_access_token("devkey", "devsecret", "test-user", &grants, 600)
        .expect("JWT generation failed")
}

/// Generate a unique name for test resources to avoid collisions.
pub fn unique_name(prefix: &str) -> String {
    format!(
        "{}-{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            % 100000
    )
}
