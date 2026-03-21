//! Calendars service client — calendars, events, RSVP.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Calendars API.
pub struct CalendarsClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for CalendarsClient {
    fn service_name(&self) -> &'static str {
        "calendars"
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

impl CalendarsClient {
    /// Build a CalendarsClient from domain (e.g. `https://calendar.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://calendar.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    // -- Calendars ----------------------------------------------------------

    /// List calendars.
    pub async fn list_calendars(&self) -> Result<DRFPage<Calendar>> {
        self.transport
            .json(
                Method::GET,
                "calendars/",
                Option::<&()>::None,
                "calendars list calendars",
            )
            .await
    }

    /// Get a single calendar by ID.
    pub async fn get_calendar(&self, id: &str) -> Result<Calendar> {
        self.transport
            .json(
                Method::GET,
                &format!("calendars/{id}/"),
                Option::<&()>::None,
                "calendars get calendar",
            )
            .await
    }

    /// Create a new calendar.
    pub async fn create_calendar(&self, body: &serde_json::Value) -> Result<Calendar> {
        self.transport
            .json(Method::POST, "calendars/", Some(body), "calendars create calendar")
            .await
    }

    // -- Events -------------------------------------------------------------

    /// List events in a calendar.
    pub async fn list_events(&self, calendar_id: &str) -> Result<DRFPage<CalEvent>> {
        self.transport
            .json(
                Method::GET,
                &format!("calendars/{calendar_id}/events/"),
                Option::<&()>::None,
                "calendars list events",
            )
            .await
    }

    /// Get a single event.
    pub async fn get_event(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<CalEvent> {
        self.transport
            .json(
                Method::GET,
                &format!("calendars/{calendar_id}/events/{event_id}/"),
                Option::<&()>::None,
                "calendars get event",
            )
            .await
    }

    /// Create a new event in a calendar.
    pub async fn create_event(
        &self,
        calendar_id: &str,
        body: &serde_json::Value,
    ) -> Result<CalEvent> {
        self.transport
            .json(
                Method::POST,
                &format!("calendars/{calendar_id}/events/"),
                Some(body),
                "calendars create event",
            )
            .await
    }

    /// Update an event (partial).
    pub async fn update_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        body: &serde_json::Value,
    ) -> Result<CalEvent> {
        self.transport
            .json(
                Method::PATCH,
                &format!("calendars/{calendar_id}/events/{event_id}/"),
                Some(body),
                "calendars update event",
            )
            .await
    }

    /// Delete an event.
    pub async fn delete_event(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("calendars/{calendar_id}/events/{event_id}/"),
                Option::<&()>::None,
                "calendars delete event",
            )
            .await
    }

    /// RSVP to an event.
    pub async fn rsvp(
        &self,
        calendar_id: &str,
        event_id: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        self.transport
            .send(
                Method::POST,
                &format!("calendars/{calendar_id}/events/{event_id}/rsvp/"),
                Some(body),
                "calendars rsvp",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = CalendarsClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://calendar.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "calendars");
    }

    #[test]
    fn test_from_parts() {
        let c = CalendarsClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
