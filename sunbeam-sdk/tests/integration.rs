//! Integration tests for sunbeam-sdk service clients.
//!
//! Requires the test stack running:
//!   docker compose -f sunbeam-sdk/tests/docker-compose.yml up -d
//!
//! Run with:
//!   cargo test -p sunbeam-sdk --features integration --test integration
#![cfg(feature = "integration")]

use sunbeam_sdk::client::{AuthMethod, ServiceClient};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Poll a URL until it returns 200, or panic after `timeout`.
async fn wait_for_healthy(url: &str, timeout: std::time::Duration) {
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

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Kratos
// ---------------------------------------------------------------------------

mod kratos {
    use super::*;
    use sunbeam_sdk::identity::KratosClient;

    fn client() -> KratosClient {
        KratosClient::from_parts("http://localhost:4434".into(), AuthMethod::None)
    }

    #[tokio::test]
    async fn health() {
        wait_for_healthy("http://localhost:4434/health/alive", TIMEOUT).await;
        let c = client();
        let status = c.alive().await.unwrap();
        assert_eq!(status.status, "ok");
    }

    #[tokio::test]
    async fn identity_crud() {
        wait_for_healthy("http://localhost:4434/health/alive", TIMEOUT).await;
        let c = client();

        // Create
        let body = sunbeam_sdk::identity::types::CreateIdentityBody {
            schema_id: "default".into(),
            traits: serde_json::json!({"email": "integration-test@example.com"}),
            state: Some("active".into()),
            metadata_public: None,
            metadata_admin: None,
            credentials: None,
            verifiable_addresses: None,
            recovery_addresses: None,
        };
        let identity = c.create_identity(&body).await.unwrap();
        assert!(!identity.id.is_empty());
        let id = identity.id.clone();

        // Get
        let fetched = c.get_identity(&id).await.unwrap();
        assert_eq!(fetched.id, id);

        // List
        let list = c.list_identities(None, None).await.unwrap();
        assert!(list.iter().any(|i| i.id == id));

        // Update
        let update = sunbeam_sdk::identity::types::UpdateIdentityBody {
            schema_id: "default".into(),
            traits: serde_json::json!({"email": "updated@example.com"}),
            state: "active".into(),
            metadata_public: None,
            metadata_admin: None,
            credentials: None,
        };
        let updated = c.update_identity(&id, &update).await.unwrap();
        assert_eq!(updated.traits["email"], "updated@example.com");

        // Delete
        c.delete_identity(&id).await.unwrap();
        let list = c.list_identities(None, None).await.unwrap();
        assert!(!list.iter().any(|i| i.id == id));
    }

    #[tokio::test]
    async fn schemas() {
        wait_for_healthy("http://localhost:4434/health/alive", TIMEOUT).await;
        let c = client();
        let schemas = c.list_schemas().await.unwrap();
        assert!(!schemas.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Hydra
// ---------------------------------------------------------------------------

mod hydra {
    use super::*;
    use sunbeam_sdk::auth::hydra::HydraClient;
    use sunbeam_sdk::auth::hydra::types::OAuth2Client;

    fn client() -> HydraClient {
        HydraClient::from_parts("http://localhost:4445".into(), AuthMethod::None)
    }

    #[tokio::test]
    async fn oauth2_client_crud() {
        wait_for_healthy("http://localhost:4445/health/alive", TIMEOUT).await;
        let c = client();

        // Create
        let body = OAuth2Client {
            client_name: Some("test-client".into()),
            grant_types: Some(vec!["authorization_code".into()]),
            redirect_uris: Some(vec!["http://localhost:9876/callback".into()]),
            scope: Some("openid email".into()),
            token_endpoint_auth_method: Some("none".into()),
            ..Default::default()
        };
        let created = c.create_client(&body).await.unwrap();
        let cid = created.client_id.unwrap();
        assert!(!cid.is_empty());

        // Get
        let fetched = c.get_client(&cid).await.unwrap();
        assert_eq!(fetched.client_name, Some("test-client".into()));

        // List
        let list = c.list_clients(None, None).await.unwrap();
        assert!(list.iter().any(|cl| cl.client_id.as_deref() == Some(&cid)));

        // Update
        let mut updated_body = fetched.clone();
        updated_body.client_name = Some("renamed-client".into());
        let updated = c.update_client(&cid, &updated_body).await.unwrap();
        assert_eq!(updated.client_name, Some("renamed-client".into()));

        // Delete
        c.delete_client(&cid).await.unwrap();
        let list = c.list_clients(None, None).await.unwrap();
        assert!(!list.iter().any(|cl| cl.client_id.as_deref() == Some(&cid)));
    }

    #[tokio::test]
    async fn token_introspect_inactive() {
        wait_for_healthy("http://localhost:4445/health/alive", TIMEOUT).await;
        let c = client();
        let result = c.introspect_token("bogus-token").await.unwrap();
        assert!(!result.active);
    }
}

// ---------------------------------------------------------------------------
// Gitea
// ---------------------------------------------------------------------------

mod gitea {
    use super::*;
    use sunbeam_sdk::gitea::GiteaClient;

    const ADMIN_USER: &str = "testadmin";
    const ADMIN_PASS: &str = "testpass123";
    const ADMIN_EMAIL: &str = "admin@test.local";

    /// Bootstrap admin user + PAT. Returns the PAT string.
    async fn setup_gitea() -> String {
        wait_for_healthy("http://localhost:3000/api/v1/version", TIMEOUT).await;
        let http = reqwest::Client::new();

        // Register user via public API (DISABLE_REGISTRATION=false)
        let _ = http
            .post("http://localhost:3000/user/sign_up")
            .form(&[
                ("user_name", ADMIN_USER),
                ("password", ADMIN_PASS),
                ("retype", ADMIN_PASS),
                ("email", ADMIN_EMAIL),
            ])
            .send()
            .await;

        // Create PAT using basic auth
        let resp = http
            .post(format!(
                "http://localhost:3000/api/v1/users/{ADMIN_USER}/tokens"
            ))
            .basic_auth(ADMIN_USER, Some(ADMIN_PASS))
            .json(&serde_json::json!({
                "name": format!("test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()),
                "scopes": ["all"]
            }))
            .send()
            .await
            .unwrap();

        if !resp.status().is_success() {
            panic!("PAT creation failed: {}", resp.text().await.unwrap_or_default());
        }

        let body: serde_json::Value = resp.json().await.unwrap();
        body["sha1"]
            .as_str()
            .or_else(|| body["token"].as_str())
            .expect("PAT response missing sha1/token field")
            .to_string()
    }

    #[tokio::test]
    async fn repo_crud() {
        let pat = setup_gitea().await;
        let c = GiteaClient::from_parts(
            "http://localhost:3000/api/v1".into(),
            AuthMethod::Token(pat),
        );

        // Authenticated user
        let me = c.get_authenticated_user().await.unwrap();
        assert_eq!(me.login, ADMIN_USER);

        // Create repo
        let body = sunbeam_sdk::gitea::types::CreateRepoBody {
            name: "integration-test".into(),
            description: Some("test repo".into()),
            auto_init: Some(true),
            ..Default::default()
        };
        let repo = c.create_user_repo(&body).await.unwrap();
        assert_eq!(repo.name, "integration-test");

        // Get repo
        let fetched = c.get_repo(ADMIN_USER, "integration-test").await.unwrap();
        assert_eq!(fetched.full_name, format!("{ADMIN_USER}/integration-test"));

        // Search repos
        let results = c.search_repos("integration", None).await.unwrap();
        assert!(!results.data.is_empty());

        // Delete repo
        c.delete_repo(ADMIN_USER, "integration-test").await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// OpenSearch
// ---------------------------------------------------------------------------

mod opensearch {
    use super::*;
    use sunbeam_sdk::search::OpenSearchClient;

    fn client() -> OpenSearchClient {
        OpenSearchClient::from_parts("http://localhost:9200".into(), AuthMethod::None)
    }

    #[tokio::test]
    async fn cluster_health() {
        wait_for_healthy("http://localhost:9200/_cluster/health", TIMEOUT).await;
        let c = client();
        let health = c.cluster_health().await.unwrap();
        assert!(!health.cluster_name.is_empty());
    }

    #[tokio::test]
    async fn document_crud() {
        wait_for_healthy("http://localhost:9200/_cluster/health", TIMEOUT).await;
        let c = client();

        let idx = "integration-test";

        // Create index
        let _ = c
            .create_index(idx, &serde_json::json!({"settings": {"number_of_shards": 1, "number_of_replicas": 0}}))
            .await
            .unwrap();

        // Index a document
        let doc = serde_json::json!({"title": "Hello", "body": "World"});
        let resp = c.index_doc(idx, "doc-1", &doc).await.unwrap();
        assert_eq!(resp.result.as_deref(), Some("created"));

        // Refresh to make searchable (use a raw reqwest call)
        let _ = reqwest::Client::new()
            .post(format!("http://localhost:9200/{idx}/_refresh"))
            .send()
            .await;

        // Get document
        let got = c.get_doc(idx, "doc-1").await.unwrap();
        assert_eq!(got.source.as_ref().unwrap()["title"], "Hello");

        // Search
        let query = serde_json::json!({"query": {"match_all": {}}});
        let results = c.search(idx, &query).await.unwrap();
        assert!(results.hits.total.value > 0);

        // Delete document
        let del = c.delete_doc(idx, "doc-1").await.unwrap();
        assert_eq!(del.result.as_deref(), Some("deleted"));

        // Delete index
        c.delete_index(idx).await.unwrap();
    }

    #[tokio::test]
    async fn cat_indices() {
        wait_for_healthy("http://localhost:9200/_cluster/health", TIMEOUT).await;
        let c = client();
        let _indices = c.cat_indices().await.unwrap();
        // Just verify it parses without error
    }
}

// ---------------------------------------------------------------------------
// Prometheus
// ---------------------------------------------------------------------------

mod prometheus {
    use super::*;
    use sunbeam_sdk::monitoring::PrometheusClient;

    fn client() -> PrometheusClient {
        PrometheusClient::from_parts("http://localhost:9090/api/v1".into(), AuthMethod::None)
    }

    #[tokio::test]
    async fn query_up() {
        wait_for_healthy("http://localhost:9090/api/v1/status/buildinfo", TIMEOUT).await;
        let c = client();

        let result = c.build_info().await.unwrap();
        assert_eq!(result.status, "success");

        let result = c.query("up", None).await.unwrap();
        assert_eq!(result.status, "success");
    }

    #[tokio::test]
    async fn labels() {
        wait_for_healthy("http://localhost:9090/api/v1/status/buildinfo", TIMEOUT).await;
        let c = client();

        let result = c.labels(None, None).await.unwrap();
        assert_eq!(result.status, "success");
    }

    #[tokio::test]
    async fn targets() {
        wait_for_healthy("http://localhost:9090/api/v1/status/buildinfo", TIMEOUT).await;
        let c = client();

        let result = c.targets().await.unwrap();
        assert_eq!(result.status, "success");
    }
}

// ---------------------------------------------------------------------------
// Loki
// ---------------------------------------------------------------------------

mod loki {
    use super::*;
    use sunbeam_sdk::monitoring::LokiClient;

    fn client() -> LokiClient {
        LokiClient::from_parts("http://localhost:3100/loki/api/v1".into(), AuthMethod::None)
    }

    #[tokio::test]
    async fn ready_and_labels() {
        wait_for_healthy("http://localhost:3100/ready", TIMEOUT).await;
        let c = client();

        let _status = c.ready().await.unwrap();

        // Loki's ring needs time to settle — retry labels a few times
        for i in 0..10 {
            match c.labels(None, None).await {
                Ok(labels) => {
                    assert_eq!(labels.status, "success");
                    return;
                }
                Err(_) if i < 9 => {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(e) => panic!("loki labels failed after retries: {e}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana
// ---------------------------------------------------------------------------

mod grafana {
    use super::*;
    use sunbeam_sdk::monitoring::GrafanaClient;

    fn client() -> GrafanaClient {
        use base64::Engine;
        let creds = base64::engine::general_purpose::STANDARD.encode("admin:admin");
        GrafanaClient::from_parts(
            "http://localhost:3001/api".into(),
            AuthMethod::Header {
                name: "Authorization",
                value: format!("Basic {creds}"),
            },
        )
    }

    #[tokio::test]
    async fn org() {
        wait_for_healthy("http://localhost:3001/api/health", TIMEOUT).await;
        let c = client();
        let org = c.get_current_org().await.unwrap();
        assert!(!org.name.is_empty());
    }

    #[tokio::test]
    async fn folder_and_dashboard_crud() {
        wait_for_healthy("http://localhost:3001/api/health", TIMEOUT).await;
        let c = client();

        // Create folder
        let folder = c
            .create_folder(&serde_json::json!({"title": "Integration Tests"}))
            .await
            .unwrap();
        let folder_uid = folder.uid.clone();
        assert!(!folder_uid.is_empty());

        // Create dashboard in folder
        let dash_body = serde_json::json!({
            "dashboard": {
                "title": "Test Dashboard",
                "panels": [],
                "schemaVersion": 30,
            },
            "folderUid": folder_uid,
            "overwrite": false
        });
        let dash = c.create_dashboard(&dash_body).await.unwrap();
        let dash_uid = dash.uid.clone().unwrap();

        // List dashboards
        let list = c.list_dashboards().await.unwrap();
        assert!(list.iter().any(|d| d.uid == dash_uid));

        // Delete dashboard
        c.delete_dashboard(&dash_uid).await.unwrap();

        // Delete folder
        c.delete_folder(&folder_uid).await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// LiveKit
// ---------------------------------------------------------------------------

mod livekit {
    use super::*;
    use sunbeam_sdk::media::LiveKitClient;
    use sunbeam_sdk::media::types::VideoGrants;

    fn client() -> LiveKitClient {
        let grants = VideoGrants {
            room_create: Some(true),
            room_list: Some(true),
            room_join: Some(true),
            ..Default::default()
        };
        let token =
            LiveKitClient::generate_access_token("devkey", "devsecret", "test-user", &grants, 300)
                .expect("JWT generation failed");
        LiveKitClient::from_parts("http://localhost:7880".into(), AuthMethod::Bearer(token))
    }

    #[tokio::test]
    async fn room_crud() {
        wait_for_healthy("http://localhost:7880", TIMEOUT).await;
        let c = client();

        // List rooms (empty initially)
        let rooms = c
            .list_rooms()
            .await
            .unwrap();
        let initial_count = rooms.rooms.len();

        // Create room
        let room = c
            .create_room(&serde_json::json!({"name": "integration-test-room"}))
            .await
            .unwrap();
        assert_eq!(room.name, "integration-test-room");

        // List rooms (should have one more)
        let rooms = c.list_rooms().await.unwrap();
        assert_eq!(rooms.rooms.len(), initial_count + 1);

        // Delete room
        c.delete_room(&serde_json::json!({"room": "integration-test-room"}))
            .await
            .unwrap();

        // Verify deleted
        let rooms = c.list_rooms().await.unwrap();
        assert_eq!(rooms.rooms.len(), initial_count);
    }
}
