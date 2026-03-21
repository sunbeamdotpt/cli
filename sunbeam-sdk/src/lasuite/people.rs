//! People service client — contacts, teams, service providers, mail domains.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite People API.
pub struct PeopleClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for PeopleClient {
    fn service_name(&self) -> &'static str {
        "people"
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

impl PeopleClient {
    /// Build a PeopleClient from domain (e.g. `https://people.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://people.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    // -- Contacts -----------------------------------------------------------

    /// List contacts with optional pagination.
    pub async fn list_contacts(&self, page: Option<u32>) -> Result<DRFPage<Contact>> {
        let path = match page {
            Some(p) => format!("contacts/?page={p}"),
            None => "contacts/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "people list contacts")
            .await
    }

    /// Get a single contact by ID.
    pub async fn get_contact(&self, id: &str) -> Result<Contact> {
        self.transport
            .json(
                Method::GET,
                &format!("contacts/{id}/"),
                Option::<&()>::None,
                "people get contact",
            )
            .await
    }

    /// Create a new contact.
    pub async fn create_contact(&self, body: &serde_json::Value) -> Result<Contact> {
        self.transport
            .json(Method::POST, "contacts/", Some(body), "people create contact")
            .await
    }

    /// Update a contact (partial).
    pub async fn update_contact(&self, id: &str, body: &serde_json::Value) -> Result<Contact> {
        self.transport
            .json(
                Method::PATCH,
                &format!("contacts/{id}/"),
                Some(body),
                "people update contact",
            )
            .await
    }

    /// Delete a contact.
    pub async fn delete_contact(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("contacts/{id}/"),
                Option::<&()>::None,
                "people delete contact",
            )
            .await
    }

    // -- Teams --------------------------------------------------------------

    /// List teams with optional pagination.
    pub async fn list_teams(&self, page: Option<u32>) -> Result<DRFPage<Team>> {
        let path = match page {
            Some(p) => format!("teams/?page={p}"),
            None => "teams/".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "people list teams")
            .await
    }

    /// Get a single team by ID.
    pub async fn get_team(&self, id: &str) -> Result<Team> {
        self.transport
            .json(
                Method::GET,
                &format!("teams/{id}/"),
                Option::<&()>::None,
                "people get team",
            )
            .await
    }

    /// Create a new team.
    pub async fn create_team(&self, body: &serde_json::Value) -> Result<Team> {
        self.transport
            .json(Method::POST, "teams/", Some(body), "people create team")
            .await
    }

    // -- Service providers --------------------------------------------------

    /// List service providers.
    pub async fn list_service_providers(&self) -> Result<DRFPage<ServiceProvider>> {
        self.transport
            .json(
                Method::GET,
                "service-providers/",
                Option::<&()>::None,
                "people list service providers",
            )
            .await
    }

    // -- Mail domains -------------------------------------------------------

    /// List mail domains.
    pub async fn list_mail_domains(&self) -> Result<DRFPage<MailDomain>> {
        self.transport
            .json(
                Method::GET,
                "mail-domains/",
                Option::<&()>::None,
                "people list mail domains",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = PeopleClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://people.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "people");
    }

    #[test]
    fn test_from_parts() {
        let c = PeopleClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
