//! OpenBao/Vault client — thin wrapper around vaultrs.
//!
//! Provides a `BaoClient` API that can be swapped to a different backend
//! without changing callers.

use crate::error::{Result, ResultExt};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::collections::HashMap;
use vaultrs::client::{Client, VaultClient, VaultClientSettingsBuilder};

/// OpenBao HTTP client wrapping vaultrs::VaultClient.
pub struct BaoClient {
    inner: VaultClient,
    pub base_url: String,
}

// Re-export the init response type for callers that need it.
pub use vaultrs::api::sys::responses::StartInitializationResponse as InitResponse;

/// Seal status response.
#[derive(Debug, Default)]
pub struct SealStatusResponse {
    pub initialized: bool,
    pub sealed: bool,
}

/// Unseal response.
#[derive(Debug, Default)]
pub struct UnsealResponse {
    pub sealed: bool,
}

/// Transit key information.
#[derive(Debug, Clone)]
pub struct TransitKeyInfo {
    pub key_type: String,
    pub latest_version: u32,
    pub public_key_raw: Vec<u8>,  // 32-byte Ed25519 pub
    /// Creation timestamp of the latest key version, as reported by Vault.
    /// Used to produce a stable OpenPGP V4 fingerprint across restarts.
    pub creation_time: chrono::DateTime<chrono::Utc>,
}

impl BaoClient {
    /// Create a new client pointing at `base_url` (e.g. `http://localhost:8200`).
    pub fn new(base_url: &str) -> Self {
        let url = base_url.trim_end_matches('/');
        let settings = VaultClientSettingsBuilder::default()
            .address(url)
            .build()
            .expect("valid vault client settings");
        Self {
            inner: VaultClient::new(settings).expect("valid vault client"),
            base_url: url.to_string(),
        }
    }

    /// Create a client with an authentication token.
    pub fn with_token(base_url: &str, token: &str) -> Self {
        let url = base_url.trim_end_matches('/');
        let settings = VaultClientSettingsBuilder::default()
            .address(url)
            .token(token.to_string())
            .build()
            .expect("valid vault client settings");
        Self {
            inner: VaultClient::new(settings).expect("valid vault client"),
            base_url: url.to_string(),
        }
    }

    fn token_header(&self) -> Option<String> {
        let t = &self.inner.settings().token;
        if t.is_empty() { None } else { Some(t.clone()) }
    }

    // ── System operations ───────────────────────────────────────────────

    pub async fn seal_status(&self) -> Result<SealStatusResponse> {
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

    pub async fn init(&self, key_shares: u32, key_threshold: u32) -> Result<InitResponse> {
        vaultrs::sys::start_initialization(
            &self.inner,
            key_shares as u64,
            key_threshold as u64,
            None,
        )
        .await
        .map_err(|e| crate::error::SunbeamError::Other(format!("OpenBao init failed: {e}")))
    }

    pub async fn unseal(&self, key: &str) -> Result<UnsealResponse> {
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

    pub async fn enable_secrets_engine(&self, path: &str, engine_type: &str) -> Result<()> {
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

    pub async fn kv_get(&self, mount: &str, path: &str) -> Result<Option<HashMap<String, String>>> {
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

    pub async fn kv_get_field(&self, mount: &str, path: &str, field: &str) -> Result<String> {
        match self.kv_get(mount, path).await? {
            Some(data) => Ok(data.get(field).cloned().unwrap_or_default()),
            None => Ok(String::new()),
        }
    }

    pub async fn kv_put(
        &self,
        mount: &str,
        path: &str,
        data: &HashMap<String, String>,
    ) -> Result<()> {
        vaultrs::kv2::set(&self.inner, mount, path, data)
            .await
            .map_err(|e| {
                crate::error::SunbeamError::Other(format!("KV put {mount}/{path}: {e}"))
            })?;
        Ok(())
    }

    /// Patch (merge) fields into an existing KV v2 secret.
    /// vaultrs doesn't have a patch method, so we use a raw HTTP request.
    pub async fn kv_patch(
        &self,
        mount: &str,
        path: &str,
        data: &HashMap<String, String>,
    ) -> Result<()> {
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

    pub async fn kv_delete(&self, mount: &str, path: &str) -> Result<()> {
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

    pub async fn auth_enable(&self, path: &str, method_type: &str) -> Result<()> {
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

    pub async fn write_policy(&self, name: &str, policy_hcl: &str) -> Result<()> {
        vaultrs::sys::policy::set(&self.inner, name, policy_hcl)
            .await
            .map_err(|e| crate::error::SunbeamError::Other(format!("Write policy {name}: {e}")))
    }

    // ── Generic read (for transit keys, arbitrary secret paths) ─────────

    /// Generic GET against the OpenBao API.
    ///
    /// Returns the parsed JSON body on success, or `Ok(None)` on 404.
    /// Use for non-KV paths like `transit/<mount>/keys/<name>`.
    pub async fn read(&self, path: &str) -> Result<Option<serde_json::Value>> {
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

    // ── Generic write (for auth config, roles, etc.) ────────────────────

    pub async fn write(&self, path: &str, data: &serde_json::Value) -> Result<serde_json::Value> {
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

    pub async fn write_db_config(
        &self,
        name: &str,
        plugin: &str,
        connection_url: &str,
        username: &str,
        password: &str,
        allowed_roles: &str,
    ) -> Result<()> {
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

    pub async fn write_db_static_role(
        &self,
        name: &str,
        db_name: &str,
        username: &str,
        rotation_period: u64,
        rotation_statements: &[&str],
    ) -> Result<()> {
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

    // ── Transit secrets engine ──────────────────────────────────────────

    /// Create a transit key. Idempotent — returns Ok on duplicate.
    pub async fn transit_create_key(
        &self,
        mount: &str,
        name: &str,
        key_type: &str,  // e.g. "ed25519"
    ) -> Result<()> {
        let path = format!("transit/{}/keys/{}", mount, name);
        match self.write(&path, &serde_json::json!({ "type": key_type })).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                // Vault returns 400 when the key already exists.
                if msg.contains("key already exists") || msg.contains("400") {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Read a transit key's public material.
    pub async fn transit_get_public_key(
        &self,
        mount: &str,
        name: &str,
    ) -> Result<TransitKeyInfo> {
        let path = format!("transit/{}/keys/{}", mount, name);
        let resp = self
            .read(&path)
            .await?
            .ok_or_else(|| crate::error::SunbeamError::Other(
                format!("transit key not found: {path}")
            ))?;

        // Walk: data.keys.<latest_version>.public_key
        let keys = resp
            .get("data")
            .and_then(|d| d.get("keys"))
            .ok_or_else(|| crate::error::SunbeamError::Other(
                format!("missing data.keys in transit key response: {path}")
            ))?;

        // The keys object has numeric string keys ("1", "2", …). Take the last
        // one (highest version).
        let latest = keys
            .as_object()
            .and_then(|m| {
                m.keys()
                    .filter_map(|k| k.parse::<u64>().ok().map(|n| (n, k.as_str())))
                    .max_by_key(|(n, _)| *n)
                    .and_then(|(_, k)| m.get(k))
            })
            .ok_or_else(|| crate::error::SunbeamError::Other(
                format!("missing latest version in transit key: {path}")
            ))?;

        let pub_key_b64 = latest
            .get("public_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::SunbeamError::Other(
                format!("missing public_key field in transit key: {path}")
            ))?;

        let public_key_raw = BASE64
            .decode(pub_key_b64)
            .map_err(|e| crate::error::SunbeamError::Other(
                format!("failed to decode public_key base64: {e}")
            ))?;

        // Parse the creation_time field from the latest version entry.
        // Vault returns RFC 3339, e.g. "2024-01-15T12:00:00Z".
        // Fall back to epoch if missing or unparseable so that the server
        // can still boot; the fallback is stable (same value every restart).
        let creation_time = latest
            .get("creation_time")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap_or(chrono::Utc::now()));

        Ok(TransitKeyInfo {
            key_type: latest
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("ed25519")
                .to_string(),
            latest_version: keys
                .as_object()
                .and_then(|m| {
                    m.keys()
                        .filter_map(|k| k.parse::<u32>().ok())
                        .max()
                })
                .unwrap_or(1),
            public_key_raw,
            creation_time,
        })
    }

    /// Sign data with a transit key.
    /// `prehashed = false` → standard Ed25519 (data hashed by Ed25519's internal SHA-512).
    /// `prehashed = true` → Ed25519ph (input is already a digest).
    pub async fn transit_sign(
        &self,
        mount: &str,
        name: &str,
        data: &[u8],
        prehashed: bool,
    ) -> Result<Vec<u8>> {
        let path = format!("transit/{}/sign/{}", mount, name);
        let body = serde_json::json!({
            "input": BASE64.encode(data),
            "prehashed": prehashed,
        });
        let resp = self.write(&path, &body).await?;

        // Parse the "signature" field; format is "vault:v1:<base64>".
        let sig_str = resp
            .get("data")
            .and_then(|d| d.get("signature"))
            .and_then(|s| s.as_str())
            .ok_or_else(|| crate::error::SunbeamError::Other(
                format!("missing signature field in transit sign response")
            ))?;

        let raw_b64 = sig_str
            .strip_prefix("vault:v1:")
            .ok_or_else(|| crate::error::SunbeamError::Other(
                format!("signature missing vault:v1: prefix: {sig_str}")
            ))?;

        BASE64
            .decode(raw_b64)
            .map_err(|e| crate::error::SunbeamError::Other(
                format!("failed to decode signature base64: {e}")
            ))
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
