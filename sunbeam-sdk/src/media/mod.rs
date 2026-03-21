//! LiveKit media service client.

pub mod types;

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::{Result, SunbeamError};
use base64::Engine;
use reqwest::Method;
use types::*;

/// Client for the LiveKit Twirp API.
pub struct LiveKitClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for LiveKitClient {
    fn service_name(&self) -> &'static str {
        "livekit"
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

impl LiveKitClient {
    /// Build a LiveKitClient from domain (e.g. `https://livekit.{domain}`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://livekit.{domain}");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Replace the auth method.
    pub fn set_auth(&mut self, auth: AuthMethod) {
        self.transport.set_auth(auth);
    }

    // -- Rooms ---------------------------------------------------------------

    /// Create a room.
    pub async fn create_room(&self, body: &(impl serde::Serialize + Sync)) -> Result<Room> {
        self.twirp("livekit.RoomService/CreateRoom", body).await
    }

    /// List all rooms.
    pub async fn list_rooms(&self) -> Result<ListRoomsResponse> {
        self.twirp("livekit.RoomService/ListRooms", &serde_json::json!({}))
            .await
    }

    /// Delete a room.
    pub async fn delete_room(&self, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.twirp_send("livekit.RoomService/DeleteRoom", body)
            .await
    }

    /// Update room metadata.
    pub async fn update_room_metadata(&self, body: &(impl serde::Serialize + Sync)) -> Result<Room> {
        self.twirp("livekit.RoomService/UpdateRoomMetadata", body)
            .await
    }

    /// Send data to a room.
    pub async fn send_data(&self, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.twirp_send("livekit.RoomService/SendData", body).await
    }

    // -- Participants --------------------------------------------------------

    /// List participants in a room.
    pub async fn list_participants(
        &self,
        body: &(impl serde::Serialize + Sync),
    ) -> Result<ListParticipantsResponse> {
        self.twirp("livekit.RoomService/ListParticipants", body)
            .await
    }

    /// Get a single participant.
    pub async fn get_participant(&self, body: &(impl serde::Serialize + Sync)) -> Result<ParticipantInfo> {
        self.twirp("livekit.RoomService/GetParticipant", body)
            .await
    }

    /// Remove a participant from a room.
    pub async fn remove_participant(&self, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.twirp_send("livekit.RoomService/RemoveParticipant", body)
            .await
    }

    /// Update a participant.
    pub async fn update_participant(
        &self,
        body: &(impl serde::Serialize + Sync),
    ) -> Result<ParticipantInfo> {
        self.twirp("livekit.RoomService/UpdateParticipant", body)
            .await
    }

    /// Mute a published track.
    pub async fn mute_track(&self, body: &(impl serde::Serialize + Sync)) -> Result<MuteTrackResponse> {
        self.twirp("livekit.RoomService/MutePublishedTrack", body)
            .await
    }

    // -- Egress --------------------------------------------------------------

    /// Start a room composite egress.
    pub async fn start_room_composite_egress(
        &self,
        body: &(impl serde::Serialize + Sync),
    ) -> Result<EgressInfo> {
        self.twirp("livekit.Egress/StartRoomCompositeEgress", body)
            .await
    }

    /// Start a track egress.
    pub async fn start_track_egress(&self, body: &(impl serde::Serialize + Sync)) -> Result<EgressInfo> {
        self.twirp("livekit.Egress/StartTrackEgress", body).await
    }

    /// List egress sessions.
    pub async fn list_egress(&self, body: &(impl serde::Serialize + Sync)) -> Result<ListEgressResponse> {
        self.twirp("livekit.Egress/ListEgress", body).await
    }

    /// Stop an egress session.
    pub async fn stop_egress(&self, body: &(impl serde::Serialize + Sync)) -> Result<EgressInfo> {
        self.twirp("livekit.Egress/StopEgress", body).await
    }

    // -- Token ---------------------------------------------------------------

    /// Generate a LiveKit access token (JWT signed with HMAC-SHA256).
    ///
    /// - `api_key`: LiveKit API key (used as `iss` claim).
    /// - `api_secret`: LiveKit API secret (HMAC key).
    /// - `identity`: participant identity (used as `sub` claim).
    /// - `grants`: video grant permissions.
    /// - `ttl_secs`: token lifetime in seconds.
    pub fn generate_access_token(
        api_key: &str,
        api_secret: &str,
        identity: &str,
        grants: &VideoGrants,
        ttl_secs: u64,
    ) -> Result<String> {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;

        let header = serde_json::json!({"alg": "HS256", "typ": "JWT"});
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| SunbeamError::Other(format!("system time error: {e}")))?
            .as_secs();

        let claims = serde_json::json!({
            "iss": api_key,
            "sub": identity,
            "nbf": now,
            "exp": now + ttl_secs,
            "video": grants,
        });

        let header_b64 = b64.encode(serde_json::to_vec(&header)?);
        let claims_b64 = b64.encode(serde_json::to_vec(&claims)?);
        let signing_input = format!("{header_b64}.{claims_b64}");

        let mut mac = Hmac::<Sha256>::new_from_slice(api_secret.as_bytes())
            .map_err(|e| SunbeamError::Other(format!("HMAC key error: {e}")))?;
        mac.update(signing_input.as_bytes());
        let signature = b64.encode(mac.finalize().into_bytes());

        Ok(format!("{signing_input}.{signature}"))
    }

    // -- Internal helpers ----------------------------------------------------

    /// Twirp POST that returns a parsed JSON response.
    async fn twirp<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        body: &(impl serde::Serialize + Sync),
    ) -> Result<T> {
        self.transport
            .json(Method::POST, &format!("twirp/{method}"), Some(body), method)
            .await
    }

    /// Twirp POST that discards the response body.
    async fn twirp_send(&self, method: &str, body: &(impl serde::Serialize + Sync)) -> Result<()> {
        self.transport
            .send(Method::POST, &format!("twirp/{method}"), Some(body), method)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = LiveKitClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://livekit.sunbeam.pt");
        assert_eq!(c.service_name(), "livekit");
    }

    #[test]
    fn test_from_parts() {
        let c = LiveKitClient::from_parts(
            "http://localhost:7880".into(),
            AuthMethod::Bearer("test-token".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:7880");
    }

    #[test]
    fn test_generate_access_token() {
        let grants = VideoGrants {
            room_join: Some(true),
            room: Some("test-room".into()),
            can_publish: Some(true),
            can_subscribe: Some(true),
            ..Default::default()
        };
        let token = LiveKitClient::generate_access_token(
            "api-key",
            "api-secret",
            "user-1",
            &grants,
            3600,
        )
        .unwrap();

        // JWT has three dot-separated parts
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Verify header
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header_bytes = b64.decode(parts[0]).unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).unwrap();
        assert_eq!(header["alg"], "HS256");
        assert_eq!(header["typ"], "JWT");

        // Verify claims
        let claims_bytes = b64.decode(parts[1]).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&claims_bytes).unwrap();
        assert_eq!(claims["iss"], "api-key");
        assert_eq!(claims["sub"], "user-1");
        assert!(claims["exp"].as_u64().unwrap() > claims["nbf"].as_u64().unwrap());
        assert_eq!(claims["video"]["roomJoin"], true);
        assert_eq!(claims["video"]["room"], "test-room");
    }

    #[test]
    fn test_generate_access_token_signature_valid() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let grants = VideoGrants {
            room_create: Some(true),
            ..Default::default()
        };
        let secret = "my-secret-key";
        let token =
            LiveKitClient::generate_access_token("key", secret, "id", &grants, 600).unwrap();

        let parts: Vec<&str> = token.split('.').collect();
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let sig_bytes = b64.decode(parts[2]).unwrap();

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signing_input.as_bytes());
        assert!(mac.verify_slice(&sig_bytes).is_ok());
    }

    #[tokio::test]
    async fn test_list_rooms_unreachable() {
        let c = LiveKitClient::from_parts(
            "http://127.0.0.1:19998".into(),
            AuthMethod::Bearer("tok".into()),
        );
        let result = c.list_rooms().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_room_unreachable() {
        let c = LiveKitClient::from_parts(
            "http://127.0.0.1:19998".into(),
            AuthMethod::Bearer("tok".into()),
        );
        let body = serde_json::json!({"name": "test-room"});
        let result = c.create_room(&body).await;
        assert!(result.is_err());
    }
}
