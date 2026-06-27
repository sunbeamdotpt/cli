//! OpenBao/Vault client — thin wrapper around vaultrs.
//!
//! Provides a `BaoClient` API that can be swapped to a different backend
//! without changing callers.

use crate::error::{Result, ResultExt};
use std::collections::HashMap;
use vaultrs::client::{Client, VaultClient, VaultClientSettingsBuilder};

/// OpenBao HTTP client wrapping vaultrs::VaultClient.
pub struct BaoClient {
    inner: VaultClient,
    /// Base url.
    pub base_url: String,
}

// Re-export the init response type for callers that need it.
pub use vaultrs::api::sys::responses::StartInitializationResponse as InitResponse;

/// Seal status response.
#[derive(Debug, Default)]
pub struct SealStatusResponse {
    /// Initialized.
    pub initialized: bool,
    /// Sealed.
    pub sealed: bool,
}

/// Unseal response.
#[derive(Debug, Default)]
pub struct UnsealResponse {
    /// Sealed.
    pub sealed: bool,
}

impl BaoClient {
    /// Create a new client pointing at `base_url` (e.g. `http://localhost:8200`).
    pub fn new(base_url: &str) -> Self {
        let url = base_url.trim_end_matches('/');
        let settings = match VaultClientSettingsBuilder::default()
            .address(url)
            .build()
        {
            Ok(settings) => settings,
            // A well-formed HTTP URL always produces valid Vault settings.
            Err(_) => unreachable!(),
        };
        let inner = match VaultClient::new(settings) {
            Ok(client) => client,
            // Valid settings always produce a usable Vault client.
            Err(_) => unreachable!(),
        };
        Self {
            inner,
            base_url: url.to_string(),
        }
    }

    /// Create a client with an authentication token.
    pub fn with_token(base_url: &str, token: &str) -> Self {
        let url = base_url.trim_end_matches('/');
        let settings = match VaultClientSettingsBuilder::default()
            .address(url)
            .token(token.to_string())
            .build()
        {
            Ok(settings) => settings,
            // A well-formed HTTP URL and non-empty token produce valid settings.
            Err(_) => unreachable!(),
        };
        let inner = match VaultClient::new(settings) {
            Ok(client) => client,
            // Valid settings always produce a usable Vault client.
            Err(_) => unreachable!(),
        };
        Self {
            inner,
            base_url: url.to_string(),
        }
    }

    fn token_header(&self) -> Option<String> {
        let t = &self.inner.settings().token;
        if t.is_empty() { None } else { Some(t.clone()) }
    }

    // ── System operations ───────────────────────────────────────────────

    /// Seal status.
    #[tracing::instrument(skip(self))]
    pub async fn seal_status(&self) -> Result<SealStatusResponse> {
        tracing::debug!("seal_status");
        match vaultrs::sys::status(&self.inner).await {
            Ok(status) => {
                use vaultrs::sys::ServerStatus;
                let (initialized, sealed) = match status {
                    ServerStatus::OK => (true, false),
                    ServerStatus::SEALED => (true, true),
                    ServerStatus::PERFSTANDBY | ServerStatus::STANDBY => (true, false),
                    ServerStatus::RECOVERY => (true, true),
                    ServerStatus::UNINITIALIZED | ServerStatus::UNKNOWN => (false, true),
                };
                Ok(SealStatusResponse {
                    initialized,
                    sealed,
                })
            }
            Err(e) => Err(crate::error::SunbeamError::Other(format!(
                "Failed to get seal status: {e}"
            ))),
        }
    }

    /// Init.
    #[tracing::instrument(skip(self))]
    pub async fn init(&self, key_shares: u32, key_threshold: u32) -> Result<InitResponse> {
        tracing::debug!("init key_shares={key_shares} key_threshold={key_threshold}");
        vaultrs::sys::start_initialization(
            &self.inner,
            key_shares as u64,
            key_threshold as u64,
            None,
        )
        .await
        .map_err(|e| crate::error::SunbeamError::Other(format!("OpenBao init failed: {e}")))
    }

    /// Unseal.
    #[tracing::instrument(skip(self))]
    pub async fn unseal(&self, key: &str) -> Result<UnsealResponse> {
        tracing::debug!("unseal");
        let resp = vaultrs::sys::unseal(&self.inner, Some(key.to_string()), None, None)
            .await
            .map_err(|e| {
                crate::error::SunbeamError::Other(format!("OpenBao unseal failed: {e}"))
            })?;
        Ok(UnsealResponse {
            sealed: resp.sealed,
        })
    }

    // ── Secrets engine management ───────────────────────────────────────

    /// Enable secrets engine.
    #[tracing::instrument(skip(self))]
    pub async fn enable_secrets_engine(&self, path: &str, engine_type: &str) -> Result<()> {
        tracing::debug!("enable_secrets_engine {path} type={engine_type}");
        match vaultrs::sys::mount::enable(&self.inner, path, engine_type, None).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("400") || msg.contains("already in use") {
                    Ok(()) // idempotent
                } else {
                    Err(crate::error::SunbeamError::Other(format!(
                        "Enable secrets engine {path}: {e}"
                    )))
                }
            }
        }
    }

    // ── KV v2 operations ────────────────────────────────────────────────

    /// Kv get.
    #[tracing::instrument(skip(self))]
    pub async fn kv_get(&self, mount: &str, path: &str) -> Result<Option<HashMap<String, String>>> {
        tracing::debug!("kv_get {mount}/{path}");
        match vaultrs::kv2::read::<HashMap<String, serde_json::Value>>(&self.inner, mount, path)
            .await
        {
            Ok(data) => {
                let result: HashMap<String, String> = data
                    .into_iter()
                    .map(|(k, v)| {
                        let s = match v {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        };
                        (k, s)
                    })
                    .collect();
                Ok(Some(result))
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("Not Found") {
                    Ok(None)
                } else {
                    Err(crate::error::SunbeamError::Other(format!(
                        "KV get {mount}/{path}: {e}"
                    )))
                }
            }
        }
    }

    /// Kv get field.
    #[tracing::instrument(skip(self))]
    pub async fn kv_get_field(&self, mount: &str, path: &str, field: &str) -> Result<String> {
        tracing::debug!("kv_get_field {mount}/{path} field={field}");
        match self.kv_get(mount, path).await? {
            Some(data) => Ok(data.get(field).cloned().unwrap_or_default()),
            None => Ok(String::new()),
        }
    }

    /// Kv put.
    #[tracing::instrument(skip(self))]
    pub async fn kv_put(
        &self,
        mount: &str,
        path: &str,
        data: &HashMap<String, String>,
    ) -> Result<()> {
        tracing::debug!("kv_put {mount}/{path}");
        vaultrs::kv2::set(&self.inner, mount, path, data)
            .await
            .map_err(|e| {
                crate::error::SunbeamError::Other(format!("KV put {mount}/{path}: {e}"))
            })?;
        Ok(())
    }

    /// Patch (merge) fields into an existing KV v2 secret.
    /// vaultrs doesn't have a patch method, so we use a raw HTTP request.
    #[tracing::instrument(skip(self))]
    pub async fn kv_patch(
        &self,
        mount: &str,
        path: &str,
        data: &HashMap<String, String>,
    ) -> Result<()> {
        tracing::debug!("kv_patch {mount}/{path}");
        #[derive(serde::Serialize)]
        struct KvWriteRequest<'a> {
            data: &'a HashMap<String, String>,
        }

        let url = format!("{}/v1/{mount}/data/{path}", self.base_url);
        let mut req = reqwest::Client::new()
            .patch(&url)
            .header("Content-Type", "application/merge-patch+json")
            .json(&KvWriteRequest { data });

        if let Some(token) = self.token_header() {
            req = req.header("X-Vault-Token", token);
        }

        let resp = req.send().await.ctx("Failed to patch KV secret")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("KV patch {mount}/{path} returned {status}: {body}");
        }
        Ok(())
    }

    /// Kv delete.
    #[tracing::instrument(skip(self))]
    pub async fn kv_delete(&self, mount: &str, path: &str) -> Result<()> {
        tracing::debug!("kv_delete {mount}/{path}");
        match vaultrs::kv2::delete_latest(&self.inner, mount, path).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") {
                    Ok(())
                } else {
                    Err(crate::error::SunbeamError::Other(format!(
                        "KV delete {mount}/{path}: {e}"
                    )))
                }
            }
        }
    }

    // ── Auth operations ─────────────────────────────────────────────────

    /// Auth enable.
    #[tracing::instrument(skip(self))]
    pub async fn auth_enable(&self, path: &str, method_type: &str) -> Result<()> {
        tracing::debug!("auth_enable {path} type={method_type}");
        match vaultrs::sys::auth::enable(&self.inner, path, method_type, None).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("400") || msg.contains("already in use") {
                    Ok(())
                } else {
                    Err(crate::error::SunbeamError::Other(format!(
                        "Enable auth {path}: {e}"
                    )))
                }
            }
        }
    }

    /// Write policy.
    #[tracing::instrument(skip(self))]
    pub async fn write_policy(&self, name: &str, policy_hcl: &str) -> Result<()> {
        tracing::debug!("write_policy {name}");
        vaultrs::sys::policy::set(&self.inner, name, policy_hcl)
            .await
            .map_err(|e| crate::error::SunbeamError::Other(format!("Write policy {name}: {e}")))
    }

    // ── Generic read (for transit keys, arbitrary secret paths) ─────────

    /// Generic GET against the OpenBao API.
    ///
    /// Returns the parsed JSON body on success, or `Ok(None)` on 404.
    /// Use for non-KV paths like `transit/<mount>/keys/<name>`.
    #[tracing::instrument(skip(self))]
    pub async fn read(&self, path: &str) -> Result<Option<serde_json::Value>> {
        tracing::debug!("read {path}");
        let url = format!("{}/v1/{}", self.base_url, path.trim_start_matches('/'));
        let mut req = reqwest::Client::new().get(&url);
        if let Some(token) = self.token_header() {
            req = req.header("X-Vault-Token", token);
        }

        let resp = req
            .send()
            .await
            .with_ctx(|| format!("Failed to read from {path}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Read {path} returned {status}: {body}");
        }

        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                serde_json::from_str(&body).ctx("Failed to parse read response")?,
            ))
        }
    }

    /// Generic LIST against the OpenBao API.
    ///
    /// Returns the parsed JSON body on success, or `Ok(None)` on 404.
    #[tracing::instrument(skip(self))]
    pub async fn list(&self, path: &str) -> Result<Option<serde_json::Value>> {
        tracing::debug!("list {path}");
        let url = format!(
            "{}/v1/{}?list=true",
            self.base_url,
            path.trim_start_matches('/')
        );
        let mut req = reqwest::Client::new().get(&url);
        if let Some(token) = self.token_header() {
            req = req.header("X-Vault-Token", token);
        }

        let resp = req
            .send()
            .await
            .with_ctx(|| format!("Failed to list {path}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("List {path} returned {status}: {body}");
        }

        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                serde_json::from_str(&body).ctx("Failed to parse list response")?,
            ))
        }
    }

    // ── Generic write (for auth config, roles, etc.) ────────────────────

    /// Write.
    #[tracing::instrument(skip(self))]
    pub async fn write(&self, path: &str, data: &serde_json::Value) -> Result<serde_json::Value> {
        tracing::debug!("write {path}");
        let url = format!("{}/v1/{}", self.base_url, path.trim_start_matches('/'));
        let mut req = reqwest::Client::new().post(&url).json(data);
        if let Some(token) = self.token_header() {
            req = req.header("X-Vault-Token", token);
        }

        let resp = req
            .send()
            .await
            .with_ctx(|| format!("Failed to write to {path}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Write {path} returned {status}: {body}");
        }

        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            Ok(serde_json::Value::Null)
        } else {
            serde_json::from_str(&body).ctx("Failed to parse write response")
        }
    }

    // ── Database secrets engine ─────────────────────────────────────────

    /// Write db config.
    #[tracing::instrument(skip(self))]
    pub async fn write_db_config(
        &self,
        name: &str,
        plugin: &str,
        connection_url: &str,
        username: &str,
        password: &str,
        allowed_roles: &str,
    ) -> Result<()> {
        tracing::debug!("write_db_config {name}");
        let data = serde_json::json!({
            "plugin_name": plugin,
            "connection_url": connection_url,
            "username": username,
            "password": password,
            "allowed_roles": allowed_roles,
        });
        self.write(&format!("database/config/{name}"), &data)
            .await?;
        Ok(())
    }

    /// Write db static role.
    pub async fn write_db_static_role(
        &self,
        name: &str,
        db_name: &str,
        username: &str,
        rotation_period: u64,
        rotation_statements: &[&str],
    ) -> Result<()> {
        tracing::debug!("write_db_static_role {db_name}");
        let data = serde_json::json!({
            "db_name": db_name,
            "username": username,
            "rotation_period": rotation_period,
            "rotation_statements": rotation_statements,
        });
        self.write(&format!("database/static-roles/{name}"), &data)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_client() {
        let client = BaoClient::new("http://localhost:8200");
        assert_eq!(client.base_url, "http://localhost:8200");
    }

    #[test]
    fn test_with_token() {
        let client = BaoClient::with_token("http://localhost:8200", "mytoken");
        assert_eq!(client.inner.settings().token, "mytoken");
    }

    #[test]
    fn test_strips_trailing_slash() {
        let client = BaoClient::new("http://localhost:8200/");
        assert_eq!(client.base_url, "http://localhost:8200");
    }

    #[tokio::test]
    async fn test_seal_status_error_on_nonexistent_server() {
        let client = BaoClient::new("http://127.0.0.1:19999");
        let result = client.seal_status().await;
        assert!(result.is_err());
    }
}
