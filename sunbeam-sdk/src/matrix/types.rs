//! Matrix client-server API types.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginTypesResponse {
    #[serde(default)]
    pub flows: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    #[serde(rename = "type")]
    pub login_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_device_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub home_server: Option<String>,
    #[serde(default)]
    pub well_known: Option<serde_json::Value>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_device_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inhibit_login: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResponse {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub home_server: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoamiResponse {
    pub user_id: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub is_guest: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThirdPartyIds {
    #[serde(default)]
    pub threepids: Vec<ThirdPartyId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThirdPartyId {
    #[serde(default)]
    pub medium: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub validated_at: Option<u64>,
    #[serde(default)]
    pub added_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Add3pidRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delete3pidRequest {
    pub medium: String,
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_server: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub new_password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logout_devices: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeactivateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub erase: Option<bool>,
}

// ---------------------------------------------------------------------------
// Rooms
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_alias_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_direct: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_content: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<Vec<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_level_content_override: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomResponse {
    pub room_id: String,
    #[serde(default)]
    pub room_alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicRoomsResponse {
    #[serde(default)]
    pub chunk: Vec<PublicRoom>,
    #[serde(default)]
    pub next_batch: Option<String>,
    #[serde(default)]
    pub prev_batch: Option<String>,
    #[serde(default)]
    pub total_room_count_estimate: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicRoom {
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub canonical_alias: Option<String>,
    #[serde(default)]
    pub num_joined_members: u64,
    #[serde(default)]
    pub world_readable: bool,
    #[serde(default)]
    pub guest_can_join: bool,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub join_rule: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPublicRoomsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_all_networks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub third_party_instance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomVisibility {
    #[serde(default)]
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRoomVisibilityRequest {
    pub visibility: String,
}

// ---------------------------------------------------------------------------
// Membership
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteRequest {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanRequest {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbanRequest {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KickRequest {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEvent {
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub state_key: String,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub origin_server_ts: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventIdResponse {
    pub event_id: String,
}

// ---------------------------------------------------------------------------
// Messages / Sync
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_state: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_presence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    #[serde(default)]
    pub next_batch: String,
    #[serde(default)]
    pub rooms: Option<serde_json::Value>,
    #[serde(default)]
    pub presence: Option<serde_json::Value>,
    #[serde(default)]
    pub account_data: Option<serde_json::Value>,
    #[serde(default)]
    pub to_device: Option<serde_json::Value>,
    #[serde(default)]
    pub device_lists: Option<serde_json::Value>,
    #[serde(default)]
    pub device_one_time_keys_count: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub origin_server_ts: Option<u64>,
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub state_key: Option<String>,
    #[serde(default)]
    pub unsigned: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessagesParams {
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub chunk: Vec<Event>,
    #[serde(default)]
    pub state: Option<Vec<StateEvent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResponse {
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub event: Option<Event>,
    #[serde(default)]
    pub events_before: Vec<Event>,
    #[serde(default)]
    pub events_after: Vec<Event>,
    #[serde(default)]
    pub state: Option<Vec<StateEvent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceStatus {
    #[serde(default)]
    pub presence: String,
    #[serde(default)]
    pub last_active_ago: Option<u64>,
    #[serde(default)]
    pub status_msg: Option<String>,
    #[serde(default)]
    pub currently_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetPresenceRequest {
    pub presence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_msg: Option<String>,
}

// ---------------------------------------------------------------------------
// Typing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypingRequest {
    pub typing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

// ---------------------------------------------------------------------------
// Receipts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReceiptRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub displayname: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Displayname {
    #[serde(default)]
    pub displayname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDisplaynameRequest {
    pub displayname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvatarUrl {
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAvatarUrlRequest {
    pub avatar_url: String,
}

// ---------------------------------------------------------------------------
// Aliases
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAliasRequest {
    pub room_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasResponse {
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub servers: Vec<String>,
}

// ---------------------------------------------------------------------------
// User Directory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSearchRequest {
    pub search_term: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSearchResponse {
    #[serde(default)]
    pub results: Vec<UserSearchResult>,
    #[serde(default)]
    pub limited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSearchResult {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Media
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResponse {
    pub content_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThumbnailParams {
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicesResponse {
    #[serde(default)]
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub last_seen_ip: Option<String>,
    #[serde(default)]
    pub last_seen_ts: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDeviceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteDeviceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDeleteDevicesRequest {
    pub devices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// E2EE / Keys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysUploadRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_keys: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_time_keys: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_keys: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysUploadResponse {
    #[serde(default)]
    pub one_time_key_counts: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysQueryRequest {
    pub device_keys: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysQueryResponse {
    #[serde(default)]
    pub device_keys: serde_json::Value,
    #[serde(default)]
    pub failures: Option<serde_json::Value>,
    #[serde(default)]
    pub master_keys: Option<serde_json::Value>,
    #[serde(default)]
    pub self_signing_keys: Option<serde_json::Value>,
    #[serde(default)]
    pub user_signing_keys: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysClaimRequest {
    pub one_time_keys: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysClaimResponse {
    #[serde(default)]
    pub one_time_keys: serde_json::Value,
    #[serde(default)]
    pub failures: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushersResponse {
    #[serde(default)]
    pub pushers: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRulesResponse {
    #[serde(default)]
    pub global: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationsResponse {
    #[serde(default)]
    pub notifications: Vec<serde_json::Value>,
    #[serde(default)]
    pub next_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagsResponse {
    #[serde(default)]
    pub tags: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub search_categories: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub search_categories: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterIdResponse {
    pub filter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    #[serde(default)]
    pub event_fields: Option<Vec<String>>,
    #[serde(default)]
    pub event_format: Option<String>,
    #[serde(default)]
    pub presence: Option<serde_json::Value>,
    #[serde(default)]
    pub account_data: Option<serde_json::Value>,
    #[serde(default)]
    pub room: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpaceHierarchyParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceHierarchy {
    #[serde(default)]
    pub rooms: Vec<serde_json::Value>,
    #[serde(default)]
    pub next_batch: Option<String>,
}

// ---------------------------------------------------------------------------
// Send-to-device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendToDeviceRequest {
    pub messages: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_request_serialize() {
        let req = LoginRequest {
            login_type: "m.login.password".into(),
            identifier: Some(serde_json::json!({
                "type": "m.id.user",
                "user": "@alice:example.com"
            })),
            password: Some("secret".into()),
            token: None,
            device_id: None,
            initial_device_display_name: None,
            refresh_token: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "m.login.password");
        assert_eq!(json["password"], "secret");
        assert!(json.get("token").is_none());
    }

    #[test]
    fn test_login_response_deserialize() {
        let json = serde_json::json!({
            "user_id": "@alice:example.com",
            "access_token": "tok123",
            "device_id": "DEV1"
        });
        let resp: LoginResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.user_id, "@alice:example.com");
        assert_eq!(resp.access_token, "tok123");
        assert_eq!(resp.device_id.as_deref(), Some("DEV1"));
    }

    #[test]
    fn test_sync_response_deserialize_minimal() {
        let json = serde_json::json!({ "next_batch": "s123" });
        let resp: SyncResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.next_batch, "s123");
        assert!(resp.rooms.is_none());
    }

    #[test]
    fn test_create_room_request_skip_none() {
        let req = CreateRoomRequest {
            name: Some("Test Room".into()),
            topic: None,
            room_alias_name: None,
            visibility: None,
            preset: None,
            invite: None,
            is_direct: None,
            creation_content: None,
            initial_state: None,
            power_level_content_override: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["name"], "Test Room");
        assert!(json.get("topic").is_none());
    }

    #[test]
    fn test_event_deserialize() {
        let json = serde_json::json!({
            "type": "m.room.message",
            "content": { "body": "hello", "msgtype": "m.text" },
            "event_id": "$abc123",
            "sender": "@bob:example.com",
            "origin_server_ts": 1234567890
        });
        let ev: Event = serde_json::from_value(json).unwrap();
        assert_eq!(ev.event_type, "m.room.message");
        assert_eq!(ev.sender.as_deref(), Some("@bob:example.com"));
    }

    #[test]
    fn test_device_deserialize() {
        let json = serde_json::json!({
            "device_id": "DEV1",
            "display_name": "My Phone",
            "last_seen_ip": "10.0.0.1",
            "last_seen_ts": 1700000000000u64
        });
        let d: Device = serde_json::from_value(json).unwrap();
        assert_eq!(d.device_id, "DEV1");
        assert_eq!(d.display_name.as_deref(), Some("My Phone"));
    }

    #[test]
    fn test_profile_deserialize() {
        let json = serde_json::json!({
            "displayname": "Alice",
            "avatar_url": "mxc://example.com/abc"
        });
        let p: Profile = serde_json::from_value(json).unwrap();
        assert_eq!(p.displayname.as_deref(), Some("Alice"));
        assert_eq!(p.avatar_url.as_deref(), Some("mxc://example.com/abc"));
    }
}
