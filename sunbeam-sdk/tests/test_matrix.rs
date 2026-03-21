#![cfg(feature = "integration")]
use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::matrix::MatrixClient;
use sunbeam_sdk::matrix::types::*;
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path, path_regex};

fn client(uri: &str) -> MatrixClient {
    MatrixClient::from_parts(uri.to_string(), AuthMethod::Bearer("test-token".into()))
}

fn ok_json(body: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

fn ok_empty() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// ServiceClient trait + connect / set_token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn service_client_trait() {
    let server = MockServer::start().await;
    let mut c = client(&server.uri());
    assert_eq!(c.service_name(), "matrix");
    assert_eq!(c.base_url(), server.uri());

    c.set_token("new-tok");
    // just exercises set_token; nothing to assert beyond no panic

    let c2 = MatrixClient::connect("example.com");
    assert_eq!(c2.base_url(), "https://matrix.example.com/_matrix");
}

// ---------------------------------------------------------------------------
// Auth endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // list_login_types
    Mock::given(method("GET")).and(path("/client/v3/login"))
        .respond_with(ok_json(serde_json::json!({"flows": []})))
        .mount(&server).await;
    let r = c.list_login_types().await.unwrap();
    assert!(r.flows.is_empty());

    // login
    Mock::given(method("POST")).and(path("/client/v3/login"))
        .respond_with(ok_json(serde_json::json!({
            "user_id": "@u:localhost",
            "access_token": "tok",
            "device_id": "D1"
        })))
        .mount(&server).await;
    let body = LoginRequest {
        login_type: "m.login.password".into(),
        identifier: None, password: Some("pw".into()), token: None,
        device_id: None, initial_device_display_name: None, refresh_token: None,
    };
    let r = c.login(&body).await.unwrap();
    assert_eq!(r.user_id, "@u:localhost");

    // refresh
    Mock::given(method("POST")).and(path("/client/v3/refresh"))
        .respond_with(ok_json(serde_json::json!({
            "access_token": "new-tok"
        })))
        .mount(&server).await;
    let r = c.refresh(&RefreshRequest { refresh_token: "rt".into() }).await.unwrap();
    assert_eq!(r.access_token, "new-tok");

    // logout
    Mock::given(method("POST")).and(path("/client/v3/logout"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.logout().await.unwrap();

    // logout_all
    Mock::given(method("POST")).and(path("/client/v3/logout/all"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.logout_all().await.unwrap();
}

// ---------------------------------------------------------------------------
// Account endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // register
    Mock::given(method("POST")).and(path("/client/v3/register"))
        .respond_with(ok_json(serde_json::json!({"user_id": "@new:localhost"})))
        .mount(&server).await;
    let body = RegisterRequest {
        username: Some("new".into()), password: Some("pw".into()),
        device_id: None, initial_device_display_name: None,
        inhibit_login: None, refresh_token: None, auth: None, kind: None,
    };
    let r = c.register(&body).await.unwrap();
    assert_eq!(r.user_id, "@new:localhost");

    // whoami
    Mock::given(method("GET")).and(path("/client/v3/account/whoami"))
        .respond_with(ok_json(serde_json::json!({"user_id": "@me:localhost"})))
        .mount(&server).await;
    let r = c.whoami().await.unwrap();
    assert_eq!(r.user_id, "@me:localhost");

    // list_3pids
    Mock::given(method("GET")).and(path("/client/v3/account/3pid"))
        .respond_with(ok_json(serde_json::json!({"threepids": []})))
        .mount(&server).await;
    let r = c.list_3pids().await.unwrap();
    assert!(r.threepids.is_empty());

    // add_3pid
    Mock::given(method("POST")).and(path("/client/v3/account/3pid/add"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.add_3pid(&Add3pidRequest { client_secret: None, sid: None, auth: None }).await.unwrap();

    // delete_3pid
    Mock::given(method("POST")).and(path("/client/v3/account/3pid/delete"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.delete_3pid(&Delete3pidRequest {
        medium: "email".into(), address: "a@b.c".into(), id_server: None,
    }).await.unwrap();

    // change_password
    Mock::given(method("POST")).and(path("/client/v3/account/password"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.change_password(&ChangePasswordRequest {
        new_password: "new-pw".into(), logout_devices: None, auth: None,
    }).await.unwrap();

    // deactivate
    Mock::given(method("POST")).and(path("/client/v3/account/deactivate"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.deactivate(&DeactivateRequest { auth: None, id_server: None, erase: None }).await.unwrap();
}

// ---------------------------------------------------------------------------
// Room endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn room_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // create_room
    Mock::given(method("POST")).and(path("/client/v3/createRoom"))
        .respond_with(ok_json(serde_json::json!({"room_id": "!r:localhost"})))
        .mount(&server).await;
    let body = CreateRoomRequest {
        name: Some("test".into()), topic: None, room_alias_name: None,
        visibility: None, preset: None, invite: None, is_direct: None,
        creation_content: None, initial_state: None, power_level_content_override: None,
    };
    let r = c.create_room(&body).await.unwrap();
    assert_eq!(r.room_id, "!r:localhost");

    // list_public_rooms — no params
    Mock::given(method("GET")).and(path("/client/v3/publicRooms"))
        .respond_with(ok_json(serde_json::json!({"chunk": []})))
        .mount(&server).await;
    let r = c.list_public_rooms(None, None).await.unwrap();
    assert!(r.chunk.is_empty());

    // list_public_rooms — with limit + since (exercises query-string branch)
    let r = c.list_public_rooms(Some(10), Some("tok")).await.unwrap();
    assert!(r.chunk.is_empty());

    // search_public_rooms
    Mock::given(method("POST")).and(path("/client/v3/publicRooms"))
        .respond_with(ok_json(serde_json::json!({"chunk": []})))
        .mount(&server).await;
    let body = SearchPublicRoomsRequest {
        limit: None, since: None, filter: None,
        include_all_networks: None, third_party_instance_id: None,
    };
    let r = c.search_public_rooms(&body).await.unwrap();
    assert!(r.chunk.is_empty());

    // get_room_visibility
    Mock::given(method("GET")).and(path_regex("/client/v3/directory/list/room/.+"))
        .respond_with(ok_json(serde_json::json!({"visibility": "public"})))
        .mount(&server).await;
    let r = c.get_room_visibility("!r:localhost").await.unwrap();
    assert_eq!(r.visibility, "public");

    // set_room_visibility
    Mock::given(method("PUT")).and(path_regex("/client/v3/directory/list/room/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_room_visibility("!r:localhost", &SetRoomVisibilityRequest {
        visibility: "private".into(),
    }).await.unwrap();
}

// ---------------------------------------------------------------------------
// Membership endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn membership_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // join_room_by_id
    Mock::given(method("POST")).and(path_regex("/client/v3/join/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.join_room_by_id("!room:localhost").await.unwrap();

    // join_room_by_alias — use URL-encoded alias to avoid # being treated as fragment
    c.join_room_by_alias("%23alias:localhost").await.unwrap();

    // leave_room
    Mock::given(method("POST")).and(path_regex("/client/v3/rooms/.+/leave"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.leave_room("!room:localhost").await.unwrap();

    // invite
    Mock::given(method("POST")).and(path_regex("/client/v3/rooms/.+/invite"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.invite("!room:localhost", &InviteRequest {
        user_id: "@u:localhost".into(), reason: None,
    }).await.unwrap();

    // ban
    Mock::given(method("POST")).and(path_regex("/client/v3/rooms/.+/ban"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.ban("!room:localhost", &BanRequest {
        user_id: "@u:localhost".into(), reason: None,
    }).await.unwrap();

    // unban
    Mock::given(method("POST")).and(path_regex("/client/v3/rooms/.+/unban"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.unban("!room:localhost", &UnbanRequest {
        user_id: "@u:localhost".into(), reason: None,
    }).await.unwrap();

    // kick
    Mock::given(method("POST")).and(path_regex("/client/v3/rooms/.+/kick"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.kick("!room:localhost", &KickRequest {
        user_id: "@u:localhost".into(), reason: None,
    }).await.unwrap();
}

// ---------------------------------------------------------------------------
// State endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn state_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // get_all_state — use exact path for the room
    Mock::given(method("GET")).and(path("/client/v3/rooms/room1/state"))
        .respond_with(ok_json(serde_json::json!([])))
        .mount(&server).await;
    let r = c.get_all_state("room1").await.unwrap();
    assert!(r.is_empty());

    // get_state_event — the path includes event_type/state_key, trailing slash for empty key
    Mock::given(method("GET")).and(path_regex("/client/v3/rooms/.+/state/.+/.*"))
        .respond_with(ok_json(serde_json::json!({"name": "Room"})))
        .mount(&server).await;
    let r = c.get_state_event("room2", "m.room.name", "").await.unwrap();
    assert_eq!(r["name"], "Room");

    // set_state_event
    Mock::given(method("PUT")).and(path_regex("/client/v3/rooms/.+/state/.+/.*"))
        .respond_with(ok_json(serde_json::json!({"event_id": "$e1"})))
        .mount(&server).await;
    let r = c.set_state_event(
        "room2", "m.room.name", "",
        &serde_json::json!({"name": "New"}),
    ).await.unwrap();
    assert_eq!(r.event_id, "$e1");
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // sync — no params
    Mock::given(method("GET")).and(path("/client/v3/sync"))
        .respond_with(ok_json(serde_json::json!({"next_batch": "s1"})))
        .mount(&server).await;
    let r = c.sync(&SyncParams::default()).await.unwrap();
    assert_eq!(r.next_batch, "s1");

    // sync — all params populated to cover every query-string branch
    let r = c.sync(&SyncParams {
        filter: Some("f".into()),
        since: Some("s0".into()),
        full_state: Some(true),
        set_presence: Some("online".into()),
        timeout: Some(30000),
    }).await.unwrap();
    assert_eq!(r.next_batch, "s1");
}

// ---------------------------------------------------------------------------
// Messages / events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn message_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // send_event
    Mock::given(method("PUT")).and(path_regex("/client/v3/rooms/.+/send/.+/.+"))
        .respond_with(ok_json(serde_json::json!({"event_id": "$e1"})))
        .mount(&server).await;
    let r = c.send_event(
        "!r:localhost", "m.room.message", "txn1",
        &serde_json::json!({"msgtype": "m.text", "body": "hi"}),
    ).await.unwrap();
    assert_eq!(r.event_id, "$e1");

    // get_messages — minimal params
    Mock::given(method("GET")).and(path_regex("/client/v3/rooms/.+/messages"))
        .respond_with(ok_json(serde_json::json!({"start": "s0", "chunk": []})))
        .mount(&server).await;
    let r = c.get_messages("!r:localhost", &MessagesParams {
        dir: "b".into(), from: None, to: None, limit: None, filter: None,
    }).await.unwrap();
    assert!(r.chunk.is_empty());

    // get_messages — all optional params to cover every branch
    let r = c.get_messages("!r:localhost", &MessagesParams {
        dir: "b".into(),
        from: Some("tok1".into()),
        to: Some("tok2".into()),
        limit: Some(10),
        filter: Some("{}".into()),
    }).await.unwrap();
    assert!(r.chunk.is_empty());

    // get_event
    Mock::given(method("GET")).and(path_regex("/client/v3/rooms/.+/event/.+"))
        .respond_with(ok_json(serde_json::json!({
            "type": "m.room.message",
            "content": {"body": "hi"}
        })))
        .mount(&server).await;
    let r = c.get_event("!r:localhost", "$ev1").await.unwrap();
    assert_eq!(r.event_type, "m.room.message");

    // get_context — without limit
    Mock::given(method("GET")).and(path_regex("/client/v3/rooms/.+/context/.+"))
        .respond_with(ok_json(serde_json::json!({
            "start": "s0", "end": "s1",
            "events_before": [], "events_after": []
        })))
        .mount(&server).await;
    c.get_context("!r:localhost", "$ev1", None).await.unwrap();

    // get_context — with limit to cover the branch
    c.get_context("!r:localhost", "$ev1", Some(5)).await.unwrap();

    // redact
    Mock::given(method("PUT")).and(path_regex("/client/v3/rooms/.+/redact/.+/.+"))
        .respond_with(ok_json(serde_json::json!({"event_id": "$re1"})))
        .mount(&server).await;
    let r = c.redact("!r:localhost", "$ev1", "txn2", &RedactRequest { reason: None }).await.unwrap();
    assert_eq!(r.event_id, "$re1");
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn presence_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // get_presence
    Mock::given(method("GET")).and(path_regex("/client/v3/presence/.+/status"))
        .respond_with(ok_json(serde_json::json!({"presence": "online"})))
        .mount(&server).await;
    let r = c.get_presence("@u:localhost").await.unwrap();
    assert_eq!(r.presence, "online");

    // set_presence
    Mock::given(method("PUT")).and(path_regex("/client/v3/presence/.+/status"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_presence("@u:localhost", &SetPresenceRequest {
        presence: "offline".into(), status_msg: None,
    }).await.unwrap();
}

// ---------------------------------------------------------------------------
// Typing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn typing_endpoint() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    Mock::given(method("PUT")).and(path_regex("/client/v3/rooms/.+/typing/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.send_typing("!r:localhost", "@u:localhost", &TypingRequest {
        typing: true, timeout: Some(30000),
    }).await.unwrap();
}

// ---------------------------------------------------------------------------
// Receipts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn receipt_endpoint() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    Mock::given(method("POST")).and(path_regex("/client/v3/rooms/.+/receipt/.+/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.send_receipt("!r:localhost", "m.read", "$ev1", &ReceiptRequest { thread_id: None }).await.unwrap();
}

// ---------------------------------------------------------------------------
// Profile endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn profile_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // get_profile
    Mock::given(method("GET")).and(path_regex("/client/v3/profile/[^/]+$"))
        .respond_with(ok_json(serde_json::json!({"displayname": "Alice"})))
        .mount(&server).await;
    let r = c.get_profile("@u:localhost").await.unwrap();
    assert_eq!(r.displayname.as_deref(), Some("Alice"));

    // get_displayname
    Mock::given(method("GET")).and(path_regex("/client/v3/profile/.+/displayname"))
        .respond_with(ok_json(serde_json::json!({"displayname": "Alice"})))
        .mount(&server).await;
    let r = c.get_displayname("@u:localhost").await.unwrap();
    assert_eq!(r.displayname.as_deref(), Some("Alice"));

    // set_displayname
    Mock::given(method("PUT")).and(path_regex("/client/v3/profile/.+/displayname"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_displayname("@u:localhost", &SetDisplaynameRequest {
        displayname: "Bob".into(),
    }).await.unwrap();

    // get_avatar_url
    Mock::given(method("GET")).and(path_regex("/client/v3/profile/.+/avatar_url"))
        .respond_with(ok_json(serde_json::json!({"avatar_url": "mxc://example/abc"})))
        .mount(&server).await;
    let r = c.get_avatar_url("@u:localhost").await.unwrap();
    assert_eq!(r.avatar_url.as_deref(), Some("mxc://example/abc"));

    // set_avatar_url
    Mock::given(method("PUT")).and(path_regex("/client/v3/profile/.+/avatar_url"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_avatar_url("@u:localhost", &SetAvatarUrlRequest {
        avatar_url: "mxc://example/xyz".into(),
    }).await.unwrap();
}

// ---------------------------------------------------------------------------
// Alias endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn alias_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // create_alias
    Mock::given(method("PUT")).and(path_regex("/client/v3/directory/room/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.create_alias("%23test:localhost", &CreateAliasRequest {
        room_id: "!r:localhost".into(),
    }).await.unwrap();

    // resolve_alias
    Mock::given(method("GET")).and(path_regex("/client/v3/directory/room/.+"))
        .respond_with(ok_json(serde_json::json!({
            "room_id": "!r:localhost", "servers": ["localhost"]
        })))
        .mount(&server).await;
    let r = c.resolve_alias("%23test:localhost").await.unwrap();
    assert_eq!(r.room_id.as_deref(), Some("!r:localhost"));

    // delete_alias
    Mock::given(method("DELETE")).and(path_regex("/client/v3/directory/room/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.delete_alias("%23test:localhost").await.unwrap();
}

// ---------------------------------------------------------------------------
// User directory search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_users_endpoint() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    Mock::given(method("POST")).and(path("/client/v3/user_directory/search"))
        .respond_with(ok_json(serde_json::json!({"results": [], "limited": false})))
        .mount(&server).await;
    let r = c.search_users(&UserSearchRequest {
        search_term: "alice".into(), limit: None,
    }).await.unwrap();
    assert!(r.results.is_empty());
}

// ---------------------------------------------------------------------------
// Media endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn media_upload() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // upload_media — success
    Mock::given(method("POST")).and(path("/media/v3/upload"))
        .respond_with(ok_json(serde_json::json!({"content_uri": "mxc://example/abc"})))
        .mount(&server).await;
    let r = c.upload_media("image/png", b"fake-png".to_vec()).await.unwrap();
    assert_eq!(r.content_uri, "mxc://example/abc");
}

#[tokio::test]
async fn media_upload_http_error() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // upload_media — HTTP error branch
    Mock::given(method("POST")).and(path("/media/v3/upload"))
        .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
        .mount(&server).await;
    let r = c.upload_media("image/png", b"fake-png".to_vec()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn media_upload_bad_json() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // upload_media — invalid JSON response branch
    Mock::given(method("POST")).and(path("/media/v3/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server).await;
    let r = c.upload_media("image/png", b"fake-png".to_vec()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn media_download_and_thumbnail() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // download_media
    Mock::given(method("GET")).and(path_regex("/media/v3/download/.+/.+"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-content".to_vec()))
        .mount(&server).await;
    let r = c.download_media("localhost", "abc123").await.unwrap();
    assert_eq!(r.as_ref(), b"fake-content");

    // thumbnail — without method param
    Mock::given(method("GET")).and(path_regex("/media/v3/thumbnail/.+/.+"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"thumb-bytes".to_vec()))
        .mount(&server).await;
    let r = c.thumbnail("localhost", "abc123", &ThumbnailParams {
        width: 64, height: 64, method: None,
    }).await.unwrap();
    assert_eq!(r.as_ref(), b"thumb-bytes");

    // thumbnail — with method param to cover the branch
    let r = c.thumbnail("localhost", "abc123", &ThumbnailParams {
        width: 64, height: 64, method: Some("crop".into()),
    }).await.unwrap();
    assert_eq!(r.as_ref(), b"thumb-bytes");
}

// ---------------------------------------------------------------------------
// Device endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn device_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // list_devices
    Mock::given(method("GET")).and(path("/client/v3/devices"))
        .respond_with(ok_json(serde_json::json!({"devices": []})))
        .mount(&server).await;
    let r = c.list_devices().await.unwrap();
    assert!(r.devices.is_empty());

    // get_device
    Mock::given(method("GET")).and(path_regex("/client/v3/devices/.+"))
        .respond_with(ok_json(serde_json::json!({"device_id": "D1"})))
        .mount(&server).await;
    let r = c.get_device("D1").await.unwrap();
    assert_eq!(r.device_id, "D1");

    // update_device
    Mock::given(method("PUT")).and(path_regex("/client/v3/devices/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.update_device("D1", &UpdateDeviceRequest {
        display_name: Some("Phone".into()),
    }).await.unwrap();

    // delete_device
    Mock::given(method("DELETE")).and(path_regex("/client/v3/devices/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.delete_device("D1", &DeleteDeviceRequest { auth: None }).await.unwrap();

    // batch_delete_devices
    Mock::given(method("POST")).and(path("/client/v3/delete_devices"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.batch_delete_devices(&BatchDeleteDevicesRequest {
        devices: vec!["D1".into(), "D2".into()], auth: None,
    }).await.unwrap();
}

// ---------------------------------------------------------------------------
// E2EE / Keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn keys_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // upload_keys
    Mock::given(method("POST")).and(path("/client/v3/keys/upload"))
        .respond_with(ok_json(serde_json::json!({"one_time_key_counts": {}})))
        .mount(&server).await;
    let r = c.upload_keys(&KeysUploadRequest {
        device_keys: None, one_time_keys: None, fallback_keys: None,
    }).await.unwrap();
    assert!(r.one_time_key_counts.is_object());

    // query_keys
    Mock::given(method("POST")).and(path("/client/v3/keys/query"))
        .respond_with(ok_json(serde_json::json!({"device_keys": {}})))
        .mount(&server).await;
    let r = c.query_keys(&KeysQueryRequest {
        device_keys: serde_json::json!({}), timeout: None,
    }).await.unwrap();
    assert!(r.device_keys.is_object());

    // claim_keys
    Mock::given(method("POST")).and(path("/client/v3/keys/claim"))
        .respond_with(ok_json(serde_json::json!({"one_time_keys": {}})))
        .mount(&server).await;
    let r = c.claim_keys(&KeysClaimRequest {
        one_time_keys: serde_json::json!({}), timeout: None,
    }).await.unwrap();
    assert!(r.one_time_keys.is_object());
}

// ---------------------------------------------------------------------------
// Push endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // list_pushers
    Mock::given(method("GET")).and(path("/client/v3/pushers"))
        .respond_with(ok_json(serde_json::json!({"pushers": []})))
        .mount(&server).await;
    let r = c.list_pushers().await.unwrap();
    assert!(r.pushers.is_empty());

    // set_pusher
    Mock::given(method("POST")).and(path("/client/v3/pushers/set"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_pusher(&serde_json::json!({})).await.unwrap();

    // get_push_rules
    Mock::given(method("GET")).and(path("/client/v3/pushrules/"))
        .respond_with(ok_json(serde_json::json!({"global": {}})))
        .mount(&server).await;
    let r = c.get_push_rules().await.unwrap();
    assert!(r.global.is_some());

    // set_push_rule
    Mock::given(method("PUT")).and(path_regex("/client/v3/pushrules/.+/.+/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_push_rule("global", "content", "rule1", &serde_json::json!({})).await.unwrap();

    // delete_push_rule
    Mock::given(method("DELETE")).and(path_regex("/client/v3/pushrules/.+/.+/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.delete_push_rule("global", "content", "rule1").await.unwrap();

    // get_notifications — no params
    Mock::given(method("GET")).and(path("/client/v3/notifications"))
        .respond_with(ok_json(serde_json::json!({"notifications": []})))
        .mount(&server).await;
    let r = c.get_notifications(&NotificationsParams::default()).await.unwrap();
    assert!(r.notifications.is_empty());

    // get_notifications — all params to cover all branches
    let r = c.get_notifications(&NotificationsParams {
        from: Some("tok".into()),
        limit: Some(5),
        only: Some("highlight".into()),
    }).await.unwrap();
    assert!(r.notifications.is_empty());
}

// ---------------------------------------------------------------------------
// Account data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_data_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // get_account_data
    Mock::given(method("GET")).and(path_regex("/client/v3/user/.+/account_data/.+"))
        .respond_with(ok_json(serde_json::json!({"key": "val"})))
        .mount(&server).await;
    let r = c.get_account_data("@u:localhost", "m.some_type").await.unwrap();
    assert_eq!(r["key"], "val");

    // set_account_data
    Mock::given(method("PUT")).and(path_regex("/client/v3/user/.+/account_data/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_account_data("@u:localhost", "m.some_type", &serde_json::json!({"key": "val"})).await.unwrap();
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tag_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // get_tags
    Mock::given(method("GET")).and(path_regex("/client/v3/user/.+/rooms/.+/tags$"))
        .respond_with(ok_json(serde_json::json!({"tags": {}})))
        .mount(&server).await;
    let r = c.get_tags("@u:localhost", "!r:localhost").await.unwrap();
    assert!(r.tags.is_object());

    // set_tag
    Mock::given(method("PUT")).and(path_regex("/client/v3/user/.+/rooms/.+/tags/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.set_tag("@u:localhost", "!r:localhost", "m.favourite", &serde_json::json!({})).await.unwrap();

    // delete_tag
    Mock::given(method("DELETE")).and(path_regex("/client/v3/user/.+/rooms/.+/tags/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.delete_tag("@u:localhost", "!r:localhost", "m.favourite").await.unwrap();
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_messages_endpoint() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    Mock::given(method("POST")).and(path("/client/v3/search"))
        .respond_with(ok_json(serde_json::json!({"search_categories": {}})))
        .mount(&server).await;
    let r = c.search_messages(&SearchRequest {
        search_categories: serde_json::json!({}),
    }).await.unwrap();
    assert!(r.search_categories.is_object());
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filter_endpoints() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    // create_filter
    Mock::given(method("POST")).and(path_regex("/client/v3/user/.+/filter$"))
        .respond_with(ok_json(serde_json::json!({"filter_id": "f1"})))
        .mount(&server).await;
    let r = c.create_filter("@u:localhost", &serde_json::json!({})).await.unwrap();
    assert_eq!(r.filter_id, "f1");

    // get_filter
    Mock::given(method("GET")).and(path_regex("/client/v3/user/.+/filter/.+"))
        .respond_with(ok_json(serde_json::json!({})))
        .mount(&server).await;
    c.get_filter("@u:localhost", "f1").await.unwrap();
}

// ---------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------

#[tokio::test]
async fn space_hierarchy_endpoint() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    Mock::given(method("GET")).and(path_regex("/client/v1/rooms/.+/hierarchy"))
        .respond_with(ok_json(serde_json::json!({"rooms": []})))
        .mount(&server).await;

    // no params
    let r = c.get_space_hierarchy("!r:localhost", &SpaceHierarchyParams::default()).await.unwrap();
    assert!(r.rooms.is_empty());

    // all params to cover all branches
    let r = c.get_space_hierarchy("!r:localhost", &SpaceHierarchyParams {
        from: Some("tok".into()),
        limit: Some(10),
        max_depth: Some(3),
        suggested_only: Some(true),
    }).await.unwrap();
    assert!(r.rooms.is_empty());
}

// ---------------------------------------------------------------------------
// Send-to-device
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_to_device_endpoint() {
    let server = MockServer::start().await;
    let c = client(&server.uri());

    Mock::given(method("PUT")).and(path_regex("/client/v3/sendToDevice/.+/.+"))
        .respond_with(ok_empty())
        .mount(&server).await;
    c.send_to_device("m.room.encrypted", "txn1", &SendToDeviceRequest {
        messages: serde_json::json!({}),
    }).await.unwrap();
}
