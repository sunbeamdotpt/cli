//! Ory Hydra OAuth2/OIDC admin API client.

pub mod types;

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use types::*;

/// Client for the Ory Hydra Admin API.
pub struct HydraClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for HydraClient {
    fn service_name(&self) -> &'static str {
        "hydra"
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

impl HydraClient {
    /// Build a HydraClient from domain (e.g. `https://auth.{domain}`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://auth.{domain}");
        Self::from_parts(base_url, AuthMethod::None)
    }

    // -- OAuth2 Clients -----------------------------------------------------

    pub async fn list_clients(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<OAuth2Client>> {
        let limit = limit.unwrap_or(20);
        let offset = offset.unwrap_or(0);
        self.transport
            .json(
                Method::GET,
                &format!("admin/clients?limit={limit}&offset={offset}"),
                Option::<&()>::None,
                "hydra list clients",
            )
            .await
    }

    pub async fn create_client(&self, body: &OAuth2Client) -> Result<OAuth2Client> {
        self.transport
            .json(Method::POST, "admin/clients", Some(body), "hydra create client")
            .await
    }

    pub async fn get_client(&self, id: &str) -> Result<OAuth2Client> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/clients/{id}"),
                Option::<&()>::None,
                "hydra get client",
            )
            .await
    }

    pub async fn update_client(&self, id: &str, body: &OAuth2Client) -> Result<OAuth2Client> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/clients/{id}"),
                Some(body),
                "hydra update client",
            )
            .await
    }

    pub async fn patch_client(
        &self,
        id: &str,
        patches: &[serde_json::Value],
    ) -> Result<OAuth2Client> {
        self.transport
            .json(
                Method::PATCH,
                &format!("admin/clients/{id}"),
                Some(&patches),
                "hydra patch client",
            )
            .await
    }

    pub async fn delete_client(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/clients/{id}"),
                Option::<&()>::None,
                "hydra delete client",
            )
            .await
    }

    pub async fn set_client_lifespans(
        &self,
        id: &str,
        body: &TokenLifespans,
    ) -> Result<OAuth2Client> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/clients/{id}/lifespans"),
                Some(body),
                "hydra set lifespans",
            )
            .await
    }

    // -- Login flow ---------------------------------------------------------

    pub async fn get_login_request(&self, challenge: &str) -> Result<LoginRequest> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/oauth2/auth/requests/login?login_challenge={challenge}"),
                Option::<&()>::None,
                "hydra get login request",
            )
            .await
    }

    pub async fn accept_login(
        &self,
        challenge: &str,
        body: &AcceptLoginBody,
    ) -> Result<RedirectResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/oauth2/auth/requests/login/accept?login_challenge={challenge}"),
                Some(body),
                "hydra accept login",
            )
            .await
    }

    pub async fn reject_login(
        &self,
        challenge: &str,
        body: &RejectBody,
    ) -> Result<RedirectResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/oauth2/auth/requests/login/reject?login_challenge={challenge}"),
                Some(body),
                "hydra reject login",
            )
            .await
    }

    // -- Consent flow -------------------------------------------------------

    pub async fn get_consent_request(&self, challenge: &str) -> Result<ConsentRequest> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/oauth2/auth/requests/consent?consent_challenge={challenge}"),
                Option::<&()>::None,
                "hydra get consent request",
            )
            .await
    }

    pub async fn accept_consent(
        &self,
        challenge: &str,
        body: &AcceptConsentBody,
    ) -> Result<RedirectResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/oauth2/auth/requests/consent/accept?consent_challenge={challenge}"),
                Some(body),
                "hydra accept consent",
            )
            .await
    }

    pub async fn reject_consent(
        &self,
        challenge: &str,
        body: &RejectBody,
    ) -> Result<RedirectResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/oauth2/auth/requests/consent/reject?consent_challenge={challenge}"),
                Some(body),
                "hydra reject consent",
            )
            .await
    }

    // -- Logout flow --------------------------------------------------------

    pub async fn get_logout_request(&self, challenge: &str) -> Result<LogoutRequest> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/oauth2/auth/requests/logout?logout_challenge={challenge}"),
                Option::<&()>::None,
                "hydra get logout request",
            )
            .await
    }

    pub async fn accept_logout(&self, challenge: &str) -> Result<RedirectResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/oauth2/auth/requests/logout/accept?logout_challenge={challenge}"),
                Option::<&()>::None,
                "hydra accept logout",
            )
            .await
    }

    pub async fn reject_logout(
        &self,
        challenge: &str,
        body: &RejectBody,
    ) -> Result<RedirectResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/oauth2/auth/requests/logout/reject?logout_challenge={challenge}"),
                Some(body),
                "hydra reject logout",
            )
            .await
    }

    // -- JWK ----------------------------------------------------------------

    pub async fn get_jwk_set(&self, set_name: &str) -> Result<JwkSet> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/keys/{set_name}"),
                Option::<&()>::None,
                "hydra get jwk set",
            )
            .await
    }

    pub async fn create_jwk_set(
        &self,
        set_name: &str,
        body: &CreateJwkBody,
    ) -> Result<JwkSet> {
        self.transport
            .json(
                Method::POST,
                &format!("admin/keys/{set_name}"),
                Some(body),
                "hydra create jwk set",
            )
            .await
    }

    pub async fn update_jwk_set(&self, set_name: &str, body: &JwkSet) -> Result<JwkSet> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/keys/{set_name}"),
                Some(body),
                "hydra update jwk set",
            )
            .await
    }

    pub async fn delete_jwk_set(&self, set_name: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/keys/{set_name}"),
                Option::<&()>::None,
                "hydra delete jwk set",
            )
            .await
    }

    pub async fn get_jwk_key(&self, set_name: &str, kid: &str) -> Result<JwkSet> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/keys/{set_name}/{kid}"),
                Option::<&()>::None,
                "hydra get jwk key",
            )
            .await
    }

    pub async fn update_jwk_key(
        &self,
        set_name: &str,
        kid: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::PUT,
                &format!("admin/keys/{set_name}/{kid}"),
                Some(body),
                "hydra update jwk key",
            )
            .await
    }

    pub async fn delete_jwk_key(&self, set_name: &str, kid: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/keys/{set_name}/{kid}"),
                Option::<&()>::None,
                "hydra delete jwk key",
            )
            .await
    }

    // -- Trusted issuers ----------------------------------------------------

    pub async fn create_trusted_issuer(
        &self,
        body: &TrustedJwtIssuer,
    ) -> Result<TrustedJwtIssuer> {
        self.transport
            .json(
                Method::POST,
                "admin/trust/grants/jwt-bearer/issuers",
                Some(body),
                "hydra create trusted issuer",
            )
            .await
    }

    pub async fn list_trusted_issuers(&self) -> Result<Vec<TrustedJwtIssuer>> {
        self.transport
            .json(
                Method::GET,
                "admin/trust/grants/jwt-bearer/issuers",
                Option::<&()>::None,
                "hydra list trusted issuers",
            )
            .await
    }

    pub async fn get_trusted_issuer(&self, id: &str) -> Result<TrustedJwtIssuer> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/trust/grants/jwt-bearer/issuers/{id}"),
                Option::<&()>::None,
                "hydra get trusted issuer",
            )
            .await
    }

    pub async fn delete_trusted_issuer(&self, id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/trust/grants/jwt-bearer/issuers/{id}"),
                Option::<&()>::None,
                "hydra delete trusted issuer",
            )
            .await
    }

    // -- Sessions -----------------------------------------------------------

    pub async fn list_consent_sessions(&self, subject: &str) -> Result<Vec<ConsentSession>> {
        self.transport
            .json(
                Method::GET,
                &format!("admin/oauth2/auth/sessions/consent?subject={subject}"),
                Option::<&()>::None,
                "hydra list consent sessions",
            )
            .await
    }

    pub async fn revoke_consent_sessions(
        &self,
        subject: &str,
        client_id: Option<&str>,
    ) -> Result<()> {
        let mut path = format!("admin/oauth2/auth/sessions/consent?subject={subject}");
        if let Some(cid) = client_id {
            path.push_str(&format!("&client={cid}"));
        }
        self.transport
            .send(Method::DELETE, &path, Option::<&()>::None, "hydra revoke consent")
            .await
    }

    pub async fn revoke_login_sessions(&self, subject: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/oauth2/auth/sessions/login?subject={subject}"),
                Option::<&()>::None,
                "hydra revoke login sessions",
            )
            .await
    }

    // -- Tokens -------------------------------------------------------------

    pub async fn introspect_token(&self, token: &str) -> Result<IntrospectResult> {
        // Hydra requires application/x-www-form-urlencoded for introspect
        let resp = self
            .transport
            .request(Method::POST, "admin/oauth2/introspect")
            .form(&[("token", token)])
            .send()
            .await
            .map_err(|e| crate::error::SunbeamError::network(format!("hydra introspect: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::error::SunbeamError::network(format!(
                "hydra introspect: HTTP {status}: {body}"
            )));
        }

        resp.json().await.map_err(|e| {
            crate::error::SunbeamError::network(format!("hydra introspect parse: {e}"))
        })
    }

    pub async fn delete_tokens_for_client(&self, client_id: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("admin/oauth2/tokens?client_id={client_id}"),
                Option::<&()>::None,
                "hydra delete tokens",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = HydraClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://auth.sunbeam.pt");
        assert_eq!(c.service_name(), "hydra");
    }

    #[test]
    fn test_from_parts() {
        let c = HydraClient::from_parts(
            "http://localhost:4445".into(),
            AuthMethod::None,
        );
        assert_eq!(c.base_url(), "http://localhost:4445");
    }
}
