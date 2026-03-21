//! Meet service client — rooms and recordings.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Meet API.
pub struct MeetClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for MeetClient {
    fn service_name(&self) -> &'static str {
        "meet"
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

impl MeetClient {
    /// Build a MeetClient from domain (e.g. `https://meet.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://meet.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    // -- Rooms --------------------------------------------------------------

    /// List rooms with optional pagination.
    pub async fn list_rooms(&self, page: Option<u32>) -> Result<DRFPage<MeetRoom>> {
        let path = match page {
            Some(p) => format!("rooms/?page={p}"),
            None => "rooms/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "meet list rooms")
            .await
    }

    /// Create a new room.
    pub async fn create_room(&self, body: &serde_json::Value) -> Result<MeetRoom> {
        self.transport
            .json(Method::POST, "rooms/", Some(body), "meet create room")
            .await
    }

    /// Get a single room by ID.
    pub async fn get_room(&self, id: &str) -> Result<MeetRoom> {
        self.transport
            .json(
                Method::GET,
                &format!("rooms/{id}/"),
                Option::<&()>::None,
                "meet get room",
            )
            .await
    }

    /// Update a room (partial).
    pub async fn update_room(&self, id: &str, body: &serde_json::Value) -> Result<MeetRoom> {
        self.transport
            .json(
                Method::PATCH,
                &format!("rooms/{id}/"),
                Some(body),
                "meet update room",
            )
            .await
    }

    /// Delete a room.
    pub async fn delete_room(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("rooms/{id}/"),
                Option::<&()>::None,
                "meet delete room",
            )
            .await
    }

    // -- Recordings ---------------------------------------------------------

    /// List recordings for a room.
    pub async fn list_recordings(&self, room_id: &str) -> Result<DRFPage<Recording>> {
        self.transport
            .json(
                Method::GET,
                &format!("rooms/{room_id}/recordings/"),
                Option::<&()>::None,
                "meet list recordings",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = MeetClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://meet.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "meet");
    }

    #[test]
    fn test_from_parts() {
        let c = MeetClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
