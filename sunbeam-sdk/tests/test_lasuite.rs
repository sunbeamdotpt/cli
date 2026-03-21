#![cfg(feature = "integration")]
use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::lasuite::*;
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path, query_param};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a standard DRF paginated response with a single result.
fn drf_page(result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "count": 1,
        "next": null,
        "previous": null,
        "results": [result]
    })
}

fn empty_page() -> serde_json::Value {
    serde_json::json!({
        "count": 0,
        "next": null,
        "previous": null,
        "results": []
    })
}

// =========================================================================
// PeopleClient
// =========================================================================

#[tokio::test]
async fn people_list_contacts() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "c1", "first_name": "Alice", "email": "alice@example.com"}));
    Mock::given(method("GET"))
        .and(path("/contacts/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_contacts(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].id, "c1");
}

#[tokio::test]
async fn people_list_contacts_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "c2"}));
    Mock::given(method("GET"))
        .and(path("/contacts/"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_contacts(Some(2)).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].id, "c2");
}

#[tokio::test]
async fn people_get_contact() {
    let server = MockServer::start().await;
    let contact = serde_json::json!({"id": "c1", "first_name": "Alice", "last_name": "Smith"});
    Mock::given(method("GET"))
        .and(path("/contacts/c1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&contact))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_contact("c1").await.unwrap();
    assert_eq!(result.id, "c1");
    assert_eq!(result.first_name.as_deref(), Some("Alice"));
}

#[tokio::test]
async fn people_create_contact() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "c-new", "first_name": "Bob"});
    Mock::given(method("POST"))
        .and(path("/contacts/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"first_name": "Bob"});
    let result = c.create_contact(&body).await.unwrap();
    assert_eq!(result.id, "c-new");
}

#[tokio::test]
async fn people_update_contact() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "c1", "first_name": "Alice", "last_name": "Jones"});
    Mock::given(method("PATCH"))
        .and(path("/contacts/c1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"last_name": "Jones"});
    let result = c.update_contact("c1", &body).await.unwrap();
    assert_eq!(result.last_name.as_deref(), Some("Jones"));
}

#[tokio::test]
async fn people_delete_contact() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/contacts/c1/"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    c.delete_contact("c1").await.unwrap();
}

#[tokio::test]
async fn people_list_teams() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "t1", "name": "Engineering"}));
    Mock::given(method("GET"))
        .and(path("/teams/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_teams(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].name.as_deref(), Some("Engineering"));
}

#[tokio::test]
async fn people_list_teams_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "t2", "name": "Design"}));
    Mock::given(method("GET"))
        .and(path("/teams/"))
        .and(query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_teams(Some(3)).await.unwrap();
    assert_eq!(page.results[0].id, "t2");
}

#[tokio::test]
async fn people_get_team() {
    let server = MockServer::start().await;
    let team = serde_json::json!({"id": "t1", "name": "Engineering", "members": ["u1", "u2"]});
    Mock::given(method("GET"))
        .and(path("/teams/t1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&team))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_team("t1").await.unwrap();
    assert_eq!(result.id, "t1");
    assert_eq!(result.members.as_ref().unwrap().len(), 2);
}

#[tokio::test]
async fn people_create_team() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "t-new", "name": "Ops"});
    Mock::given(method("POST"))
        .and(path("/teams/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"name": "Ops"});
    let result = c.create_team(&body).await.unwrap();
    assert_eq!(result.id, "t-new");
}

#[tokio::test]
async fn people_list_service_providers() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "sp1", "name": "OIDC Provider", "base_url": "https://auth.example.com"}));
    Mock::given(method("GET"))
        .and(path("/service-providers/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_service_providers().await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].name.as_deref(), Some("OIDC Provider"));
}

#[tokio::test]
async fn people_list_mail_domains() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "md1", "name": "example.com", "status": "active"}));
    Mock::given(method("GET"))
        .and(path("/mail-domains/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = PeopleClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_mail_domains().await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].status.as_deref(), Some("active"));
}

#[tokio::test]
async fn people_connect_and_with_token() {
    let c = PeopleClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://people.example.com/api/v1.0");
    assert_eq!(c.service_name(), "people");
}

// =========================================================================
// DocsClient
// =========================================================================

#[tokio::test]
async fn docs_list_documents() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "d1", "title": "My Doc"}));
    Mock::given(method("GET"))
        .and(path("/documents/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_documents(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].title.as_deref(), Some("My Doc"));
}

#[tokio::test]
async fn docs_list_documents_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "d2"}));
    Mock::given(method("GET"))
        .and(path("/documents/"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_documents(Some(2)).await.unwrap();
    assert_eq!(page.results[0].id, "d2");
}

#[tokio::test]
async fn docs_get_document() {
    let server = MockServer::start().await;
    let doc = serde_json::json!({"id": "d1", "title": "My Doc", "content": "Hello world"});
    Mock::given(method("GET"))
        .and(path("/documents/d1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&doc))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_document("d1").await.unwrap();
    assert_eq!(result.id, "d1");
    assert_eq!(result.content.as_deref(), Some("Hello world"));
}

#[tokio::test]
async fn docs_create_document() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "d-new", "title": "New Doc", "is_public": false});
    Mock::given(method("POST"))
        .and(path("/documents/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"title": "New Doc"});
    let result = c.create_document(&body).await.unwrap();
    assert_eq!(result.id, "d-new");
    assert_eq!(result.is_public, Some(false));
}

#[tokio::test]
async fn docs_update_document() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "d1", "title": "Updated Doc"});
    Mock::given(method("PATCH"))
        .and(path("/documents/d1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"title": "Updated Doc"});
    let result = c.update_document("d1", &body).await.unwrap();
    assert_eq!(result.title.as_deref(), Some("Updated Doc"));
}

#[tokio::test]
async fn docs_delete_document() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/documents/d1/"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    c.delete_document("d1").await.unwrap();
}

#[tokio::test]
async fn docs_list_templates() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "tpl1", "title": "Meeting Notes"}));
    Mock::given(method("GET"))
        .and(path("/templates/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_templates(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].title.as_deref(), Some("Meeting Notes"));
}

#[tokio::test]
async fn docs_list_templates_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "tpl2"}));
    Mock::given(method("GET"))
        .and(path("/templates/"))
        .and(query_param("page", "4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_templates(Some(4)).await.unwrap();
    assert_eq!(page.results[0].id, "tpl2");
}

#[tokio::test]
async fn docs_create_template() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "tpl-new", "title": "Blank", "is_public": true});
    Mock::given(method("POST"))
        .and(path("/templates/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"title": "Blank"});
    let result = c.create_template(&body).await.unwrap();
    assert_eq!(result.id, "tpl-new");
    assert_eq!(result.is_public, Some(true));
}

#[tokio::test]
async fn docs_list_versions() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "v1", "document_id": "d1", "version_number": 3}));
    Mock::given(method("GET"))
        .and(path("/documents/d1/versions/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_versions("d1").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].version_number, Some(3));
}

#[tokio::test]
async fn docs_invite_user() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "inv1", "email": "bob@example.com", "role": "editor", "document_id": "d1"});
    Mock::given(method("POST"))
        .and(path("/documents/d1/invitations/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = DocsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"email": "bob@example.com", "role": "editor"});
    let result = c.invite_user("d1", &body).await.unwrap();
    assert_eq!(result.id, "inv1");
    assert_eq!(result.role.as_deref(), Some("editor"));
}

#[tokio::test]
async fn docs_connect_and_with_token() {
    let c = DocsClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://docs.example.com/api/v1.0");
    assert_eq!(c.service_name(), "docs");
}

// =========================================================================
// MeetClient
// =========================================================================

#[tokio::test]
async fn meet_list_rooms() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "r1", "name": "Standup", "slug": "standup"}));
    Mock::given(method("GET"))
        .and(path("/rooms/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_rooms(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].slug.as_deref(), Some("standup"));
}

#[tokio::test]
async fn meet_list_rooms_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "r2", "name": "Retro"}));
    Mock::given(method("GET"))
        .and(path("/rooms/"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_rooms(Some(2)).await.unwrap();
    assert_eq!(page.results[0].id, "r2");
}

#[tokio::test]
async fn meet_create_room() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "r-new", "name": "Planning", "is_public": true});
    Mock::given(method("POST"))
        .and(path("/rooms/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"name": "Planning"});
    let result = c.create_room(&body).await.unwrap();
    assert_eq!(result.id, "r-new");
    assert_eq!(result.is_public, Some(true));
}

#[tokio::test]
async fn meet_get_room() {
    let server = MockServer::start().await;
    let room = serde_json::json!({"id": "r1", "name": "Standup", "configuration": {"audio": true}});
    Mock::given(method("GET"))
        .and(path("/rooms/r1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&room))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_room("r1").await.unwrap();
    assert_eq!(result.id, "r1");
    assert!(result.configuration.is_some());
}

#[tokio::test]
async fn meet_update_room() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "r1", "name": "Daily Standup"});
    Mock::given(method("PATCH"))
        .and(path("/rooms/r1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"name": "Daily Standup"});
    let result = c.update_room("r1", &body).await.unwrap();
    assert_eq!(result.name.as_deref(), Some("Daily Standup"));
}

#[tokio::test]
async fn meet_delete_room() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/rooms/r1/"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    c.delete_room("r1").await.unwrap();
}

#[tokio::test]
async fn meet_list_recordings() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "rec1", "room_id": "r1", "filename": "recording.mp4", "duration": 3600.5}));
    Mock::given(method("GET"))
        .and(path("/rooms/r1/recordings/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MeetClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_recordings("r1").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].duration, Some(3600.5));
}

#[tokio::test]
async fn meet_connect_and_with_token() {
    let c = MeetClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://meet.example.com/api/v1.0");
    assert_eq!(c.service_name(), "meet");
}

// =========================================================================
// DriveClient
// =========================================================================

#[tokio::test]
async fn drive_list_files() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "f1", "name": "report.pdf", "size": 1024, "mime_type": "application/pdf"}));
    Mock::given(method("GET"))
        .and(path("/files/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_files(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].name.as_deref(), Some("report.pdf"));
    assert_eq!(page.results[0].size, Some(1024));
}

#[tokio::test]
async fn drive_list_files_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "f2"}));
    Mock::given(method("GET"))
        .and(path("/files/"))
        .and(query_param("page", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_files(Some(5)).await.unwrap();
    assert_eq!(page.results[0].id, "f2");
}

#[tokio::test]
async fn drive_get_file() {
    let server = MockServer::start().await;
    let file = serde_json::json!({"id": "f1", "name": "report.pdf", "url": "https://cdn.example.com/f1"});
    Mock::given(method("GET"))
        .and(path("/files/f1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&file))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_file("f1").await.unwrap();
    assert_eq!(result.id, "f1");
    assert_eq!(result.url.as_deref(), Some("https://cdn.example.com/f1"));
}

#[tokio::test]
async fn drive_upload_file() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "f-new", "name": "upload.txt", "size": 256});
    Mock::given(method("POST"))
        .and(path("/files/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"name": "upload.txt"});
    let result = c.upload_file(&body).await.unwrap();
    assert_eq!(result.id, "f-new");
}

#[tokio::test]
async fn drive_delete_file() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/files/f1/"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    c.delete_file("f1").await.unwrap();
}

#[tokio::test]
async fn drive_list_folders() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "fld1", "name": "Documents", "parent_id": null}));
    Mock::given(method("GET"))
        .and(path("/folders/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_folders(None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].name.as_deref(), Some("Documents"));
}

#[tokio::test]
async fn drive_list_folders_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "fld2"}));
    Mock::given(method("GET"))
        .and(path("/folders/"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_folders(Some(2)).await.unwrap();
    assert_eq!(page.results[0].id, "fld2");
}

#[tokio::test]
async fn drive_create_folder() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "fld-new", "name": "Archive"});
    Mock::given(method("POST"))
        .and(path("/folders/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"name": "Archive"});
    let result = c.create_folder(&body).await.unwrap();
    assert_eq!(result.id, "fld-new");
}

#[tokio::test]
async fn drive_share_file() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "sh1", "file_id": "f1", "user_id": "u1", "role": "viewer"});
    Mock::given(method("POST"))
        .and(path("/files/f1/shares/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"user_id": "u1", "role": "viewer"});
    let result = c.share_file("f1", &body).await.unwrap();
    assert_eq!(result.id, "sh1");
    assert_eq!(result.role.as_deref(), Some("viewer"));
}

#[tokio::test]
async fn drive_get_permissions() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "perm1", "file_id": "f1", "can_read": true, "can_write": false}));
    Mock::given(method("GET"))
        .and(path("/files/f1/permissions/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = DriveClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.get_permissions("f1").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].can_read, Some(true));
    assert_eq!(page.results[0].can_write, Some(false));
}

#[tokio::test]
async fn drive_connect_and_with_token() {
    let c = DriveClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://drive.example.com/api/v1.0");
    assert_eq!(c.service_name(), "drive");
}

// =========================================================================
// MessagesClient
// =========================================================================

#[tokio::test]
async fn messages_list_mailboxes() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "mb1", "email": "alice@example.com", "display_name": "Alice"}));
    Mock::given(method("GET"))
        .and(path("/mailboxes/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_mailboxes().await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].email.as_deref(), Some("alice@example.com"));
}

#[tokio::test]
async fn messages_get_mailbox() {
    let server = MockServer::start().await;
    let mb = serde_json::json!({"id": "mb1", "email": "alice@example.com", "display_name": "Alice"});
    Mock::given(method("GET"))
        .and(path("/mailboxes/mb1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&mb))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_mailbox("mb1").await.unwrap();
    assert_eq!(result.id, "mb1");
    assert_eq!(result.display_name.as_deref(), Some("Alice"));
}

#[tokio::test]
async fn messages_list_messages() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({
        "id": "msg1", "subject": "Hello", "from_address": "bob@example.com",
        "to_addresses": ["alice@example.com"], "is_read": false
    }));
    Mock::given(method("GET"))
        .and(path("/mailboxes/mb1/messages/"))
        .and(query_param("folder", "inbox"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_messages("mb1", "inbox").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].subject.as_deref(), Some("Hello"));
    assert_eq!(page.results[0].is_read, Some(false));
}

#[tokio::test]
async fn messages_get_message() {
    let server = MockServer::start().await;
    let msg = serde_json::json!({
        "id": "msg1", "subject": "Hello", "body": "Hi Alice",
        "from_address": "bob@example.com", "folder": "inbox"
    });
    Mock::given(method("GET"))
        .and(path("/mailboxes/mb1/messages/msg1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&msg))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_message("mb1", "msg1").await.unwrap();
    assert_eq!(result.id, "msg1");
    assert_eq!(result.body.as_deref(), Some("Hi Alice"));
}

#[tokio::test]
async fn messages_send_message() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({
        "id": "msg-new", "subject": "Re: Hello",
        "to_addresses": ["bob@example.com"], "folder": "sent"
    });
    Mock::given(method("POST"))
        .and(path("/mailboxes/mb1/messages/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"subject": "Re: Hello", "to_addresses": ["bob@example.com"], "body": "Thanks!"});
    let result = c.send_message("mb1", &body).await.unwrap();
    assert_eq!(result.id, "msg-new");
    assert_eq!(result.folder.as_deref(), Some("sent"));
}

#[tokio::test]
async fn messages_list_folders() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "fld1", "name": "inbox", "message_count": 42, "unread_count": 5}));
    Mock::given(method("GET"))
        .and(path("/mailboxes/mb1/folders/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_folders("mb1").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].message_count, Some(42));
    assert_eq!(page.results[0].unread_count, Some(5));
}

#[tokio::test]
async fn messages_list_contacts() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "mc1", "email": "charlie@example.com", "display_name": "Charlie"}));
    Mock::given(method("GET"))
        .and(path("/mailboxes/mb1/contacts/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = MessagesClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_contacts("mb1").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].display_name.as_deref(), Some("Charlie"));
}

#[tokio::test]
async fn messages_connect_and_with_token() {
    let c = MessagesClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://mail.example.com/api/v1.0");
    assert_eq!(c.service_name(), "messages");
}

// =========================================================================
// CalendarsClient
// =========================================================================

#[tokio::test]
async fn calendars_list_calendars() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "cal1", "name": "Work", "color": "#0000ff", "is_default": true}));
    Mock::given(method("GET"))
        .and(path("/calendars/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_calendars().await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].name.as_deref(), Some("Work"));
    assert_eq!(page.results[0].is_default, Some(true));
}

#[tokio::test]
async fn calendars_get_calendar() {
    let server = MockServer::start().await;
    let cal = serde_json::json!({"id": "cal1", "name": "Work", "description": "Work calendar"});
    Mock::given(method("GET"))
        .and(path("/calendars/cal1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&cal))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_calendar("cal1").await.unwrap();
    assert_eq!(result.id, "cal1");
    assert_eq!(result.description.as_deref(), Some("Work calendar"));
}

#[tokio::test]
async fn calendars_create_calendar() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "cal-new", "name": "Personal", "color": "#ff0000"});
    Mock::given(method("POST"))
        .and(path("/calendars/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"name": "Personal", "color": "#ff0000"});
    let result = c.create_calendar(&body).await.unwrap();
    assert_eq!(result.id, "cal-new");
    assert_eq!(result.color.as_deref(), Some("#ff0000"));
}

#[tokio::test]
async fn calendars_list_events() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({
        "id": "ev1", "title": "Standup", "start": "2026-03-21T09:00:00Z",
        "end": "2026-03-21T09:30:00Z", "all_day": false
    }));
    Mock::given(method("GET"))
        .and(path("/calendars/cal1/events/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.list_events("cal1").await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].title.as_deref(), Some("Standup"));
    assert_eq!(page.results[0].all_day, Some(false));
}

#[tokio::test]
async fn calendars_get_event() {
    let server = MockServer::start().await;
    let ev = serde_json::json!({
        "id": "ev1", "title": "Standup", "location": "Room A",
        "attendees": ["alice@example.com", "bob@example.com"]
    });
    Mock::given(method("GET"))
        .and(path("/calendars/cal1/events/ev1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&ev))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let result = c.get_event("cal1", "ev1").await.unwrap();
    assert_eq!(result.id, "ev1");
    assert_eq!(result.location.as_deref(), Some("Room A"));
    assert_eq!(result.attendees.as_ref().unwrap().len(), 2);
}

#[tokio::test]
async fn calendars_create_event() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({
        "id": "ev-new", "title": "Lunch", "start": "2026-03-21T12:00:00Z",
        "end": "2026-03-21T13:00:00Z", "calendar_id": "cal1"
    });
    Mock::given(method("POST"))
        .and(path("/calendars/cal1/events/"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&resp))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"title": "Lunch", "start": "2026-03-21T12:00:00Z", "end": "2026-03-21T13:00:00Z"});
    let result = c.create_event("cal1", &body).await.unwrap();
    assert_eq!(result.id, "ev-new");
    assert_eq!(result.calendar_id.as_deref(), Some("cal1"));
}

#[tokio::test]
async fn calendars_update_event() {
    let server = MockServer::start().await;
    let resp = serde_json::json!({"id": "ev1", "title": "Updated Standup", "location": "Room B"});
    Mock::given(method("PATCH"))
        .and(path("/calendars/cal1/events/ev1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&resp))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"title": "Updated Standup", "location": "Room B"});
    let result = c.update_event("cal1", "ev1", &body).await.unwrap();
    assert_eq!(result.title.as_deref(), Some("Updated Standup"));
    assert_eq!(result.location.as_deref(), Some("Room B"));
}

#[tokio::test]
async fn calendars_delete_event() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/calendars/cal1/events/ev1/"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    c.delete_event("cal1", "ev1").await.unwrap();
}

#[tokio::test]
async fn calendars_rsvp() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/calendars/cal1/events/ev1/rsvp/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server).await;

    let c = CalendarsClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let body = serde_json::json!({"status": "accepted"});
    c.rsvp("cal1", "ev1", &body).await.unwrap();
}

#[tokio::test]
async fn calendars_connect_and_with_token() {
    let c = CalendarsClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://calendar.example.com/api/v1.0");
    assert_eq!(c.service_name(), "calendars");
}

// =========================================================================
// FindClient
// =========================================================================

#[tokio::test]
async fn find_search() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({
        "id": "sr1", "title": "Meeting Notes", "description": "Notes from standup",
        "url": "https://docs.example.com/d/1", "source": "docs", "score": 0.95
    }));
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("q", "meeting"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = FindClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.search("meeting", None).await.unwrap();
    assert_eq!(page.count, 1);
    assert_eq!(page.results[0].title.as_deref(), Some("Meeting Notes"));
    assert_eq!(page.results[0].score, Some(0.95));
    assert_eq!(page.results[0].source.as_deref(), Some("docs"));
}

#[tokio::test]
async fn find_search_paginated() {
    let server = MockServer::start().await;
    let body = drf_page(serde_json::json!({"id": "sr2", "title": "Budget Report"}));
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("q", "budget"))
        .and(query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = FindClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.search("budget", Some(3)).await.unwrap();
    assert_eq!(page.results[0].id, "sr2");
}

#[tokio::test]
async fn find_search_empty() {
    let server = MockServer::start().await;
    let body = empty_page();
    Mock::given(method("GET"))
        .and(path("/search/"))
        .and(query_param("q", "nonexistent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server).await;

    let c = FindClient::from_parts(server.uri(), AuthMethod::Bearer("tok".into()));
    let page = c.search("nonexistent", None).await.unwrap();
    assert_eq!(page.count, 0);
    assert!(page.results.is_empty());
}

#[tokio::test]
async fn find_connect_and_with_token() {
    let c = FindClient::connect("example.com").with_token("my-tok");
    assert_eq!(c.base_url(), "https://find.example.com/api/v1.0");
    assert_eq!(c.service_name(), "find");
}
