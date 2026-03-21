//! Find (search) service client.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Find (search) API.
pub struct FindClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for FindClient {
    fn service_name(&self) -> &'static str {
        "find"
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

impl FindClient {
    /// Build a FindClient from domain (e.g. `https://find.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://find.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    /// Search across La Suite services.
    pub async fn search(
        &self,
        query: &str,
        page: Option<u32>,
    ) -> Result<DRFPage<SearchResult>> {
        let path = match page {
            Some(p) => format!("search/?q={query}&page={p}"),
            None => format!("search/?q={query}"),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "find search")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = FindClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://find.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "find");
    }

    #[test]
    fn test_from_parts() {
        let c = FindClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
