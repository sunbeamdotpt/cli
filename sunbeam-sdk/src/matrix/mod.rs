//! Matrix client-server API client.

pub mod types;

#[cfg(feature = "cli")]
pub mod cli;

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use types::*;

/// Client for the Matrix Client-Server API.
pub struct MatrixClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for MatrixClient {
    fn service_name(&self) -> &'static str {
        "matrix"
    }

    fn base_url(&self) -> &str {
        &self.transport.base_url
    }

    fn from_parts(base_url: String, auth: AuthMethod) -> Self {
        Self {
            transport: HttpTransport::new(&base_url, auth),
        }
    }
}

impl MatrixClient {
    /// Build a MatrixClient from domain (e.g. `https://matrix.{domain}/_matrix`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://matrix.{domain}/_matrix");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Replace the auth token.
    pub fn set_token(&mut self, token: &str) {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
    }

    // -----------------------------------------------------------------------
    // Auth
    // -----------------------------------------------------------------------

    /// List supported login types.
    pub async fn list_login_types(&self) -> Result<LoginTypesResponse> {
        self.transport
            .json(
                Method::GET,
                "client/v3/login",
                Option::<&()>::None,
                "matrix list login types",
            )
            .await
    }

    /// Authenticate and obtain an access token.
    pub async fn login(&self, body: &LoginRequest) -> Result<LoginResponse> {
        self.transport
            .json(Method::POST, "client/v3/login", Some(body), "matrix login")
            .await
    }

    /// Refresh an access token.
    pub async fn refresh(&self, body: &RefreshRequest) -> Result<RefreshResponse> {
        self.transport
            .json(Method::POST, "client/v3/refresh", Some(body), "matrix refresh")
            .await
    }

    /// Invalidate the current access token.
    pub async fn logout(&self) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                "client/v3/logout",
                Option::<&()>::None,
                "matrix logout",
            )
            .await
    }

    /// Invalidate all access tokens for the user.
    pub async fn logout_all(&self) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                "client/v3/logout/all",
                Option::<&()>::None,
                "matrix logout all",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Account
    // -----------------------------------------------------------------------

    /// Register a new account.
    pub async fn register(&self, body: &RegisterRequest) -> Result<RegisterResponse> {
        self.transport
            .json(Method::POST, "client/v3/register", Some(body), "matrix register")
            .await
    }

    /// Get the authenticated user's identity.
    pub async fn whoami(&self) -> Result<WhoamiResponse> {
        self.transport
            .json(
                Method::GET,
                "client/v3/account/whoami",
                Option::<&()>::None,
                "matrix whoami",
            )
            .await
    }

    /// List third-party identifiers for the account.
    pub async fn list_3pids(&self) -> Result<ThirdPartyIds> {
        self.transport
            .json(
                Method::GET,
                "client/v3/account/3pid",
                Option::<&()>::None,
                "matrix list 3pids",
            )
            .await
    }

    /// Add a third-party identifier to the account.
    pub async fn add_3pid(&self, body: &Add3pidRequest) -> Result<()> {
        self.transport
            .send(Method::POST, "client/v3/account/3pid/add", Some(body), "matrix add 3pid")
            .await
    }

    /// Remove a third-party identifier from the account.
    pub async fn delete_3pid(&self, body: &Delete3pidRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                "client/v3/account/3pid/delete",
                Some(body),
                "matrix delete 3pid",
            )
            .await
    }

    /// Change the account password.
    pub async fn change_password(&self, body: &ChangePasswordRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                "client/v3/account/password",
                Some(body),
                "matrix change password",
            )
            .await
    }

    /// Deactivate the account.
    pub async fn deactivate(&self, body: &DeactivateRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                "client/v3/account/deactivate",
                Some(body),
                "matrix deactivate",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Rooms
    // -----------------------------------------------------------------------

    /// Create a new room.
    pub async fn create_room(&self, body: &CreateRoomRequest) -> Result<CreateRoomResponse> {
        self.transport
            .json(Method::POST, "client/v3/createRoom", Some(body), "matrix create room")
            .await
    }

    /// List public rooms on the server.
    pub async fn list_public_rooms(
        &self,
        limit: Option<u32>,
        since: Option<&str>,
    ) -> Result<PublicRoomsResponse> {
        let mut path = "client/v3/publicRooms".to_string();
        let mut params = Vec::new();
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        if let Some(s) = since {
            params.push(format!("since={s}"));
        }
        if !params.is_empty() {
            path.push('?');
            path.push_str(&params.join("&"));
        }
        self.transport
            .json(
                Method::GET,
                &path,
                Option::<&()>::None,
                "matrix list public rooms",
            )
            .await
    }

    /// Search public rooms with filtering.
    pub async fn search_public_rooms(
        &self,
        body: &SearchPublicRoomsRequest,
    ) -> Result<PublicRoomsResponse> {
        self.transport
            .json(
                Method::POST,
                "client/v3/publicRooms",
                Some(body),
                "matrix search public rooms",
            )
            .await
    }

    /// Get a room's visibility in the directory.
    pub async fn get_room_visibility(&self, room_id: &str) -> Result<RoomVisibility> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/directory/list/room/{room_id}"),
                Option::<&()>::None,
                "matrix get room visibility",
            )
            .await
    }

    /// Set a room's visibility in the directory.
    pub async fn set_room_visibility(
        &self,
        room_id: &str,
        body: &SetRoomVisibilityRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/directory/list/room/{room_id}"),
                Some(body),
                "matrix set room visibility",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Membership
    // -----------------------------------------------------------------------

    /// Join a room by room ID.
    pub async fn join_room_by_id(&self, room_id: &str) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/join/{room_id}"),
                Option::<&()>::None,
                "matrix join room by id",
            )
            .await
    }

    /// Join a room by alias.
    pub async fn join_room_by_alias(&self, alias: &str) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/join/{alias}"),
                Option::<&()>::None,
                "matrix join room by alias",
            )
            .await
    }

    /// Leave a room.
    pub async fn leave_room(&self, room_id: &str) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/rooms/{room_id}/leave"),
                Option::<&()>::None,
                "matrix leave room",
            )
            .await
    }

    /// Invite a user to a room.
    pub async fn invite(&self, room_id: &str, body: &InviteRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/rooms/{room_id}/invite"),
                Some(body),
                "matrix invite",
            )
            .await
    }

    /// Ban a user from a room.
    pub async fn ban(&self, room_id: &str, body: &BanRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/rooms/{room_id}/ban"),
                Some(body),
                "matrix ban",
            )
            .await
    }

    /// Unban a user from a room.
    pub async fn unban(&self, room_id: &str, body: &UnbanRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/rooms/{room_id}/unban"),
                Some(body),
                "matrix unban",
            )
            .await
    }

    /// Kick a user from a room.
    pub async fn kick(&self, room_id: &str, body: &KickRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/rooms/{room_id}/kick"),
                Some(body),
                "matrix kick",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // State
    // -----------------------------------------------------------------------

    /// Get all state events for a room.
    pub async fn get_all_state(&self, room_id: &str) -> Result<Vec<StateEvent>> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/rooms/{room_id}/state"),
                Option::<&()>::None,
                "matrix get all state",
            )
            .await
    }

    /// Get a specific state event.
    pub async fn get_state_event(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/rooms/{room_id}/state/{event_type}/{state_key}"),
                Option::<&()>::None,
                "matrix get state event",
            )
            .await
    }

    /// Set a state event in a room.
    pub async fn set_state_event(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
        body: &serde_json::Value,
    ) -> Result<EventIdResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("client/v3/rooms/{room_id}/state/{event_type}/{state_key}"),
                Some(body),
                "matrix set state event",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Messages
    // -----------------------------------------------------------------------

    /// Synchronise the client's state with the server.
    pub async fn sync(&self, params: &SyncParams) -> Result<SyncResponse> {
        let mut path = "client/v3/sync".to_string();
        let mut qs = Vec::new();
        if let Some(ref f) = params.filter {
            qs.push(format!("filter={f}"));
        }
        if let Some(ref s) = params.since {
            qs.push(format!("since={s}"));
        }
        if let Some(fs) = params.full_state {
            qs.push(format!("full_state={fs}"));
        }
        if let Some(ref sp) = params.set_presence {
            qs.push(format!("set_presence={sp}"));
        }
        if let Some(t) = params.timeout {
            qs.push(format!("timeout={t}"));
        }
        if !qs.is_empty() {
            path.push('?');
            path.push_str(&qs.join("&"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "matrix sync")
            .await
    }

    /// Send a message event to a room.
    pub async fn send_event(
        &self,
        room_id: &str,
        event_type: &str,
        txn_id: &str,
        body: &serde_json::Value,
    ) -> Result<EventIdResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("client/v3/rooms/{room_id}/send/{event_type}/{txn_id}"),
                Some(body),
                "matrix send event",
            )
            .await
    }

    /// Get messages for a room.
    pub async fn get_messages(
        &self,
        room_id: &str,
        params: &MessagesParams,
    ) -> Result<MessagesResponse> {
        let mut path = format!("client/v3/rooms/{room_id}/messages?dir={}", params.dir);
        if let Some(ref f) = params.from {
            path.push_str(&format!("&from={f}"));
        }
        if let Some(ref t) = params.to {
            path.push_str(&format!("&to={t}"));
        }
        if let Some(l) = params.limit {
            path.push_str(&format!("&limit={l}"));
        }
        if let Some(ref f) = params.filter {
            path.push_str(&format!("&filter={f}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "matrix get messages")
            .await
    }

    /// Get a single event from a room.
    pub async fn get_event(&self, room_id: &str, event_id: &str) -> Result<Event> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/rooms/{room_id}/event/{event_id}"),
                Option::<&()>::None,
                "matrix get event",
            )
            .await
    }

    /// Get events around a given event.
    pub async fn get_context(
        &self,
        room_id: &str,
        event_id: &str,
        limit: Option<u32>,
    ) -> Result<ContextResponse> {
        let mut path = format!("client/v3/rooms/{room_id}/context/{event_id}");
        if let Some(l) = limit {
            path.push_str(&format!("?limit={l}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "matrix get context")
            .await
    }

    /// Redact an event in a room.
    pub async fn redact(
        &self,
        room_id: &str,
        event_id: &str,
        txn_id: &str,
        body: &RedactRequest,
    ) -> Result<EventIdResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("client/v3/rooms/{room_id}/redact/{event_id}/{txn_id}"),
                Some(body),
                "matrix redact",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Presence
    // -----------------------------------------------------------------------

    /// Get presence status for a user.
    pub async fn get_presence(&self, user_id: &str) -> Result<PresenceStatus> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/presence/{user_id}/status"),
                Option::<&()>::None,
                "matrix get presence",
            )
            .await
    }

    /// Set presence status for a user.
    pub async fn set_presence(
        &self,
        user_id: &str,
        body: &SetPresenceRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/presence/{user_id}/status"),
                Some(body),
                "matrix set presence",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Typing
    // -----------------------------------------------------------------------

    /// Send a typing notification.
    pub async fn send_typing(
        &self,
        room_id: &str,
        user_id: &str,
        body: &TypingRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/rooms/{room_id}/typing/{user_id}"),
                Some(body),
                "matrix send typing",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Receipts
    // -----------------------------------------------------------------------

    /// Send a read receipt.
    pub async fn send_receipt(
        &self,
        room_id: &str,
        receipt_type: &str,
        event_id: &str,
        body: &ReceiptRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("client/v3/rooms/{room_id}/receipt/{receipt_type}/{event_id}"),
                Some(body),
                "matrix send receipt",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Profiles
    // -----------------------------------------------------------------------

    /// Get a user's profile.
    pub async fn get_profile(&self, user_id: &str) -> Result<Profile> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/profile/{user_id}"),
                Option::<&()>::None,
                "matrix get profile",
            )
            .await
    }

    /// Get a user's display name.
    pub async fn get_displayname(&self, user_id: &str) -> Result<Displayname> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/profile/{user_id}/displayname"),
                Option::<&()>::None,
                "matrix get displayname",
            )
            .await
    }

    /// Set a user's display name.
    pub async fn set_displayname(
        &self,
        user_id: &str,
        body: &SetDisplaynameRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/profile/{user_id}/displayname"),
                Some(body),
                "matrix set displayname",
            )
            .await
    }

    /// Get a user's avatar URL.
    pub async fn get_avatar_url(&self, user_id: &str) -> Result<AvatarUrl> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/profile/{user_id}/avatar_url"),
                Option::<&()>::None,
                "matrix get avatar url",
            )
            .await
    }

    /// Set a user's avatar URL.
    pub async fn set_avatar_url(
        &self,
        user_id: &str,
        body: &SetAvatarUrlRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/profile/{user_id}/avatar_url"),
                Some(body),
                "matrix set avatar url",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Aliases
    // -----------------------------------------------------------------------

    /// Create a room alias.
    pub async fn create_alias(&self, alias: &str, body: &CreateAliasRequest) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/directory/room/{alias}"),
                Some(body),
                "matrix create alias",
            )
            .await
    }

    /// Resolve a room alias to a room ID.
    pub async fn resolve_alias(&self, alias: &str) -> Result<AliasResponse> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/directory/room/{alias}"),
                Option::<&()>::None,
                "matrix resolve alias",
            )
            .await
    }

    /// Delete a room alias.
    pub async fn delete_alias(&self, alias: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("client/v3/directory/room/{alias}"),
                Option::<&()>::None,
                "matrix delete alias",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // User Directory
    // -----------------------------------------------------------------------

    /// Search the user directory.
    pub async fn search_users(&self, body: &UserSearchRequest) -> Result<UserSearchResponse> {
        self.transport
            .json(
                Method::POST,
                "client/v3/user_directory/search",
                Some(body),
                "matrix search users",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Media
    // -----------------------------------------------------------------------

    /// Upload media content.
    pub async fn upload_media(
        &self,
        content_type: &str,
        data: Vec<u8>,
    ) -> Result<UploadResponse> {
        let resp = self
            .transport
            .request(Method::POST, "media/v3/upload")
            .header("Content-Type", content_type)
            .body(data)
            .send()
            .await
            .map_err(|e| crate::error::SunbeamError::network(format!(
                "matrix upload media: request failed: {e}"
            )))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(crate::error::SunbeamError::network(format!(
                "matrix upload media: HTTP {status}: {body_text}"
            )));
        }

        resp.json::<UploadResponse>()
            .await
            .map_err(|e| crate::error::SunbeamError::network(format!(
                "matrix upload media: failed to parse response: {e}"
            )))
    }

    /// Download media content.
    pub async fn download_media(
        &self,
        server: &str,
        media_id: &str,
    ) -> Result<bytes::Bytes> {
        self.transport
            .bytes(
                Method::GET,
                &format!("media/v3/download/{server}/{media_id}"),
                "matrix download media",
            )
            .await
    }

    /// Download a thumbnail of media content.
    pub async fn thumbnail(
        &self,
        server: &str,
        media_id: &str,
        params: &ThumbnailParams,
    ) -> Result<bytes::Bytes> {
        let mut path = format!(
            "media/v3/thumbnail/{server}/{media_id}?width={}&height={}",
            params.width, params.height
        );
        if let Some(ref m) = params.method {
            path.push_str(&format!("&method={m}"));
        }
        self.transport
            .bytes(Method::GET, &path, "matrix thumbnail")
            .await
    }

    // -----------------------------------------------------------------------
    // Devices
    // -----------------------------------------------------------------------

    /// List all devices for the authenticated user.
    pub async fn list_devices(&self) -> Result<DevicesResponse> {
        self.transport
            .json(
                Method::GET,
                "client/v3/devices",
                Option::<&()>::None,
                "matrix list devices",
            )
            .await
    }

    /// Get information about a specific device.
    pub async fn get_device(&self, device_id: &str) -> Result<Device> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/devices/{device_id}"),
                Option::<&()>::None,
                "matrix get device",
            )
            .await
    }

    /// Update a device's metadata.
    pub async fn update_device(
        &self,
        device_id: &str,
        body: &UpdateDeviceRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/devices/{device_id}"),
                Some(body),
                "matrix update device",
            )
            .await
    }

    /// Delete a device.
    pub async fn delete_device(
        &self,
        device_id: &str,
        body: &DeleteDeviceRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("client/v3/devices/{device_id}"),
                Some(body),
                "matrix delete device",
            )
            .await
    }

    /// Delete multiple devices at once.
    pub async fn batch_delete_devices(&self, body: &BatchDeleteDevicesRequest) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                "client/v3/delete_devices",
                Some(body),
                "matrix batch delete devices",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // E2EE / Keys
    // -----------------------------------------------------------------------

    /// Upload end-to-end encryption keys.
    pub async fn upload_keys(&self, body: &KeysUploadRequest) -> Result<KeysUploadResponse> {
        self.transport
            .json(
                Method::POST,
                "client/v3/keys/upload",
                Some(body),
                "matrix upload keys",
            )
            .await
    }

    /// Query users' device keys.
    pub async fn query_keys(&self, body: &KeysQueryRequest) -> Result<KeysQueryResponse> {
        self.transport
            .json(
                Method::POST,
                "client/v3/keys/query",
                Some(body),
                "matrix query keys",
            )
            .await
    }

    /// Claim one-time keys.
    pub async fn claim_keys(&self, body: &KeysClaimRequest) -> Result<KeysClaimResponse> {
        self.transport
            .json(
                Method::POST,
                "client/v3/keys/claim",
                Some(body),
                "matrix claim keys",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Push
    // -----------------------------------------------------------------------

    /// List pushers for the authenticated user.
    pub async fn list_pushers(&self) -> Result<PushersResponse> {
        self.transport
            .json(
                Method::GET,
                "client/v3/pushers",
                Option::<&()>::None,
                "matrix list pushers",
            )
            .await
    }

    /// Set a pusher for the authenticated user.
    pub async fn set_pusher(&self, body: &serde_json::Value) -> Result<()> {
        self.transport
            .send(Method::POST, "client/v3/pushers/set", Some(body), "matrix set pusher")
            .await
    }

    /// Get all push rules for the authenticated user.
    pub async fn get_push_rules(&self) -> Result<PushRulesResponse> {
        self.transport
            .json(
                Method::GET,
                "client/v3/pushrules/",
                Option::<&()>::None,
                "matrix get push rules",
            )
            .await
    }

    /// Set a push rule.
    pub async fn set_push_rule(
        &self,
        scope: &str,
        kind: &str,
        rule_id: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/pushrules/{scope}/{kind}/{rule_id}"),
                Some(body),
                "matrix set push rule",
            )
            .await
    }

    /// Delete a push rule.
    pub async fn delete_push_rule(
        &self,
        scope: &str,
        kind: &str,
        rule_id: &str,
    ) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("client/v3/pushrules/{scope}/{kind}/{rule_id}"),
                Option::<&()>::None,
                "matrix delete push rule",
            )
            .await
    }

    /// Get notifications for the authenticated user.
    pub async fn get_notifications(
        &self,
        params: &NotificationsParams,
    ) -> Result<NotificationsResponse> {
        let mut path = "client/v3/notifications".to_string();
        let mut qs = Vec::new();
        if let Some(ref f) = params.from {
            qs.push(format!("from={f}"));
        }
        if let Some(l) = params.limit {
            qs.push(format!("limit={l}"));
        }
        if let Some(ref o) = params.only {
            qs.push(format!("only={o}"));
        }
        if !qs.is_empty() {
            path.push('?');
            path.push_str(&qs.join("&"));
        }
        self.transport
            .json(
                Method::GET,
                &path,
                Option::<&()>::None,
                "matrix get notifications",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Account Data
    // -----------------------------------------------------------------------

    /// Get account data for a user.
    pub async fn get_account_data(
        &self,
        user_id: &str,
        data_type: &str,
    ) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/user/{user_id}/account_data/{data_type}"),
                Option::<&()>::None,
                "matrix get account data",
            )
            .await
    }

    /// Set account data for a user.
    pub async fn set_account_data(
        &self,
        user_id: &str,
        data_type: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/user/{user_id}/account_data/{data_type}"),
                Some(body),
                "matrix set account data",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Tags
    // -----------------------------------------------------------------------

    /// Get tags for a room.
    pub async fn get_tags(&self, user_id: &str, room_id: &str) -> Result<TagsResponse> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/user/{user_id}/rooms/{room_id}/tags"),
                Option::<&()>::None,
                "matrix get tags",
            )
            .await
    }

    /// Set a tag on a room.
    pub async fn set_tag(
        &self,
        user_id: &str,
        room_id: &str,
        tag: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/user/{user_id}/rooms/{room_id}/tags/{tag}"),
                Some(body),
                "matrix set tag",
            )
            .await
    }

    /// Delete a tag from a room.
    pub async fn delete_tag(
        &self,
        user_id: &str,
        room_id: &str,
        tag: &str,
    ) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("client/v3/user/{user_id}/rooms/{room_id}/tags/{tag}"),
                Option::<&()>::None,
                "matrix delete tag",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Search for messages in rooms.
    pub async fn search_messages(&self, body: &SearchRequest) -> Result<SearchResponse> {
        self.transport
            .json(
                Method::POST,
                "client/v3/search",
                Some(body),
                "matrix search messages",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Filters
    // -----------------------------------------------------------------------

    /// Create a filter for a user.
    pub async fn create_filter(
        &self,
        user_id: &str,
        body: &serde_json::Value,
    ) -> Result<FilterIdResponse> {
        self.transport
            .json(
                Method::POST,
                &format!("client/v3/user/{user_id}/filter"),
                Some(body),
                "matrix create filter",
            )
            .await
    }

    /// Get a previously created filter.
    pub async fn get_filter(&self, user_id: &str, filter_id: &str) -> Result<Filter> {
        self.transport
            .json(
                Method::GET,
                &format!("client/v3/user/{user_id}/filter/{filter_id}"),
                Option::<&()>::None,
                "matrix get filter",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Spaces
    // -----------------------------------------------------------------------

    /// Get the space hierarchy for a room.
    pub async fn get_space_hierarchy(
        &self,
        room_id: &str,
        params: &SpaceHierarchyParams,
    ) -> Result<SpaceHierarchy> {
        let mut path = format!("client/v1/rooms/{room_id}/hierarchy");
        let mut qs = Vec::new();
        if let Some(ref f) = params.from {
            qs.push(format!("from={f}"));
        }
        if let Some(l) = params.limit {
            qs.push(format!("limit={l}"));
        }
        if let Some(d) = params.max_depth {
            qs.push(format!("max_depth={d}"));
        }
        if let Some(s) = params.suggested_only {
            qs.push(format!("suggested_only={s}"));
        }
        if !qs.is_empty() {
            path.push('?');
            path.push_str(&qs.join("&"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "matrix get space hierarchy")
            .await
    }

    // -----------------------------------------------------------------------
    // Send-to-device
    // -----------------------------------------------------------------------

    /// Send an event to specific devices.
    pub async fn send_to_device(
        &self,
        event_type: &str,
        txn_id: &str,
        body: &SendToDeviceRequest,
    ) -> Result<()> {
        self.transport
            .send(
                Method::PUT,
                &format!("client/v3/sendToDevice/{event_type}/{txn_id}"),
                Some(body),
                "matrix send to device",
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ServiceClient;

    #[test]
    fn test_connect_url() {
        let c = MatrixClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://matrix.sunbeam.pt/_matrix");
        assert_eq!(c.service_name(), "matrix");
    }

    #[test]
    fn test_from_parts() {
        let c = MatrixClient::from_parts(
            "http://localhost:8008/_matrix".into(),
            AuthMethod::Bearer("tok123".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8008/_matrix");
    }

    #[test]
    fn test_connect_uses_bearer_auth() {
        let c = MatrixClient::connect("example.com");
        assert!(matches!(c.transport.auth, AuthMethod::Bearer(_)));
    }

    #[test]
    fn test_set_token() {
        let mut c = MatrixClient::connect("example.com");
        c.set_token("my-access-token");
        assert!(
            matches!(c.transport.auth, AuthMethod::Bearer(ref s) if s == "my-access-token")
        );
    }
}
