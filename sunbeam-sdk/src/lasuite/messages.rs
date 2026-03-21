//! Messages (mail) service client — mailboxes, messages, folders, contacts.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::*;

/// Client for the La Suite Messages (mail) API.
pub struct MessagesClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for MessagesClient {
    fn service_name(&self) -> &'static str {
        "messages"
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

impl MessagesClient {
    /// Build a MessagesClient from domain (e.g. `https://mail.{domain}/api/v1.0`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://mail.{domain}/api/v1.0");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Set the bearer token for authentication.
    pub fn with_token(mut self, token: &str) -> Self {
        self.transport.set_auth(AuthMethod::Bearer(token.to_string()));
        self
    }

    // -- Mailboxes ----------------------------------------------------------

    /// List mailboxes.
    pub async fn list_mailboxes(&self) -> Result<DRFPage<Mailbox>> {
        self.transport
            .json(
                Method::GET,
                "mailboxes/",
                Option::<&()>::None,
                "messages list mailboxes",
            )
            .await
    }

    /// Get a single mailbox by ID.
    pub async fn get_mailbox(&self, id: &str) -> Result<Mailbox> {
        self.transport
            .json(
                Method::GET,
                &format!("mailboxes/{id}/"),
                Option::<&()>::None,
                "messages get mailbox",
            )
            .await
    }

    // -- Messages -----------------------------------------------------------

    /// List messages in a mailbox folder.
    pub async fn list_messages(
        &self,
        mailbox_id: &str,
        folder: &str,
    ) -> Result<DRFPage<EmailMessage>> {
        self.transport
            .json(
                Method::GET,
                &format!("mailboxes/{mailbox_id}/messages/?folder={folder}"),
                Option::<&()>::None,
                "messages list messages",
            )
            .await
    }

    /// Get a single message.
    pub async fn get_message(
        &self,
        mailbox_id: &str,
        message_id: &str,
    ) -> Result<EmailMessage> {
        self.transport
            .json(
                Method::GET,
                &format!("mailboxes/{mailbox_id}/messages/{message_id}/"),
                Option::<&()>::None,
                "messages get message",
            )
            .await
    }

    /// Send a message from a mailbox.
    pub async fn send_message(
        &self,
        mailbox_id: &str,
        body: &serde_json::Value,
    ) -> Result<EmailMessage> {
        self.transport
            .json(
                Method::POST,
                &format!("mailboxes/{mailbox_id}/messages/"),
                Some(body),
                "messages send message",
            )
            .await
    }

    // -- Folders ------------------------------------------------------------

    /// List folders in a mailbox.
    pub async fn list_folders(&self, mailbox_id: &str) -> Result<DRFPage<MailFolder>> {
        self.transport
            .json(
                Method::GET,
                &format!("mailboxes/{mailbox_id}/folders/"),
                Option::<&()>::None,
                "messages list folders",
            )
            .await
    }

    // -- Contacts -----------------------------------------------------------

    /// List contacts in a mailbox.
    pub async fn list_contacts(&self, mailbox_id: &str) -> Result<DRFPage<MailContact>> {
        self.transport
            .json(
                Method::GET,
                &format!("mailboxes/{mailbox_id}/contacts/"),
                Option::<&()>::None,
                "messages list contacts",
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = MessagesClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://mail.sunbeam.pt/api/v1.0");
        assert_eq!(c.service_name(), "messages");
    }

    #[test]
    fn test_from_parts() {
        let c = MessagesClient::from_parts(
            "http://localhost:8000/api/v1.0".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:8000/api/v1.0");
    }
}
