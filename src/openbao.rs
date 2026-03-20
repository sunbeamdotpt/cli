//! Lightweight OpenBao/Vault HTTP API client.
//!
//! Replaces all `kubectl exec openbao-0 -- sh -c "bao ..."` calls from the
//! Python version with direct HTTP API calls via port-forward to openbao:8200.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenBao HTTP client wrapping a base URL and optional root token.
#[derive(Clone)]
pub struct BaoClient {
    pub base_url: String,
    pub token: Option<String>,
    http: reqwest::Client,
}

// ── API response types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct InitResponse {
    pub unseal_keys_b64: Vec<String>,
    pub root_token: String,
}

#[derive(Debug, Deserialize)]
pub struct SealStatusResponse {
    #[serde(default)]
    pub initialized: bool,
    #[serde(default)]
    pub sealed: bool,
    #[serde(default)]
    pub progress: u32,
    #[serde(default)]
    pub t: u32,
    #[serde(default)]
    pub n: u32,
}

#[derive(Debug, Deserialize)]
pub struct UnsealResponse {
    #[serde(default)]
    pub sealed: bool,
    #[serde(default)]
    pub progress: u32,
}

/// KV v2 read response wrapper.
#[derive(Debug, Deserialize)]
struct KvReadResponse {
    data: Option<KvReadData>,
}

#[derive(Debug, Deserialize)]
struct KvReadData {
    data: Option<HashMap<String, serde_json::Value>>,
}

// ── Client implementation ───────────────────────────────────────────────────

impl BaoClient {
    /// Create a new client pointing at `base_url` (e.g. `http://localhost:8200`).
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: None,
            http: reqwest::Client::new(),
        }
    }

    /// Create a client with an authentication token.
    pub fn with_token(base_url: &str, token: &str) -> Self {
        let mut client = Self::new(base_url);
        client.token = Some(token.to_string());
        client
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.request(method, self.url(path));
        if let Some(ref token) = self.token {
            req = req.header("X-Vault-Token", token);
        }
        req
    }

    // ── System operations ───────────────────────────────────────────────

    /// Get the seal status of the OpenBao instance.
    pub async fn seal_status(&self) -> Result<SealStatusResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/sys/seal-status", self.base_url))
            .send()
            .await
            .context("Failed to connect to OpenBao")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("OpenBao seal-status returned {status}: {body}");
        }
        resp.json().await.context("Failed to parse seal status")
    }

    /// Initialize OpenBao with the given number of key shares and threshold.
    pub async fn init(&self, key_shares: u32, key_threshold: u32) -> Result<InitResponse> {
        #[derive(Serialize)]
        struct InitRequest {
            secret_shares: u32,
            secret_threshold: u32,
        }

        let resp = self
            .http
            .put(format!("{}/v1/sys/init", self.base_url))
            .json(&InitRequest {
                secret_shares: key_shares,
                secret_threshold: key_threshold,
            })
            .send()
            .await
            .context("Failed to initialize OpenBao")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("OpenBao init returned {status}: {body}");
        }
        resp.json().await.context("Failed to parse init response")
    }

    /// Unseal OpenBao with one key share.
    pub async fn unseal(&self, key: &str) -> Result<UnsealResponse> {
        #[derive(Serialize)]
        struct UnsealRequest<'a> {
            key: &'a str,
        }

        let resp = self
            .http
            .put(format!("{}/v1/sys/unseal", self.base_url))
            .json(&UnsealRequest { key })
            .send()
            .await
            .context("Failed to unseal OpenBao")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("OpenBao unseal returned {status}: {body}");
        }
        resp.json().await.context("Failed to parse unseal response")
    }

    // ── Secrets engine management ───────────────────────────────────────

    /// Enable a secrets engine at the given path.
    /// Returns Ok(()) even if already enabled (409 is tolerated).
    pub async fn enable_secrets_engine(&self, path: &str, engine_type: &str) -> Result<()> {
        #[derive(Serialize)]
        struct EnableRequest<'a> {
            r#type: &'a str,
        }

        let resp = self
            .request(reqwest::Method::POST, &format!("sys/mounts/{path}"))
            .json(&EnableRequest {
                r#type: engine_type,
            })
            .send()
            .await
            .context("Failed to enable secrets engine")?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 400 {
            // 400 = "path is already in use" — idempotent
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            bail!("Enable secrets engine {path} returned {status}: {body}");
        }
    }

    // ── KV v2 operations ────────────────────────────────────────────────

    /// Read all fields from a KV v2 secret path.
    /// Returns None if the path doesn't exist (404).
    pub async fn kv_get(&self, mount: &str, path: &str) -> Result<Option<HashMap<String, String>>> {
        let resp = self
            .request(reqwest::Method::GET, &format!("{mount}/data/{path}"))
            .send()
            .await
            .context("Failed to read KV secret")?;

        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("KV get {mount}/{path} returned {status}: {body}");
        }

        let kv_resp: KvReadResponse = resp.json().await.context("Failed to parse KV response")?;
        let data = kv_resp
            .data
            .and_then(|d| d.data)
            .unwrap_or_default();

        // Convert all values to strings
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

    /// Read a single field from a KV v2 secret path.
    /// Returns empty string if path or field doesn't exist.
    pub async fn kv_get_field(&self, mount: &str, path: &str, field: &str) -> Result<String> {
        match self.kv_get(mount, path).await? {
            Some(data) => Ok(data.get(field).cloned().unwrap_or_default()),
            None => Ok(String::new()),
        }
    }

    /// Write (create or overwrite) all fields in a KV v2 secret path.
    pub async fn kv_put(
        &self,
        mount: &str,
        path: &str,
        data: &HashMap<String, String>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct KvWriteRequest<'a> {
            data: &'a HashMap<String, String>,
        }

        let resp = self
            .request(reqwest::Method::POST, &format!("{mount}/data/{path}"))
            .json(&KvWriteRequest { data })
            .send()
            .await
            .context("Failed to write KV secret")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("KV put {mount}/{path} returned {status}: {body}");
        }
        Ok(())
    }

    /// Patch (merge) fields into an existing KV v2 secret path.
    pub async fn kv_patch(
        &self,
        mount: &str,
        path: &str,
        data: &HashMap<String, String>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct KvWriteRequest<'a> {
            data: &'a HashMap<String, String>,
        }

        let resp = self
            .request(reqwest::Method::PATCH, &format!("{mount}/data/{path}"))
            .header("Content-Type", "application/merge-patch+json")
            .json(&KvWriteRequest { data })
            .send()
            .await
            .context("Failed to patch KV secret")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("KV patch {mount}/{path} returned {status}: {body}");
        }
        Ok(())
    }

    /// Delete a KV v2 secret path (soft delete — deletes latest version).
    pub async fn kv_delete(&self, mount: &str, path: &str) -> Result<()> {
        let resp = self
            .request(reqwest::Method::DELETE, &format!("{mount}/data/{path}"))
            .send()
            .await
            .context("Failed to delete KV secret")?;

        // 404 is fine (already deleted)
        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("KV delete {mount}/{path} returned {status}: {body}");
        }
        Ok(())
    }

    // ── Auth operations ─────────────────────────────────────────────────

    /// Enable an auth method at the given path.
    /// Tolerates "already enabled" (400/409).
    pub async fn auth_enable(&self, path: &str, method_type: &str) -> Result<()> {
        #[derive(Serialize)]
        struct AuthEnableRequest<'a> {
            r#type: &'a str,
        }

        let resp = self
            .request(reqwest::Method::POST, &format!("sys/auth/{path}"))
            .json(&AuthEnableRequest {
                r#type: method_type,
            })
            .send()
            .await
            .context("Failed to enable auth method")?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 400 {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            bail!("Enable auth {path} returned {status}: {body}");
        }
    }

    /// Write a policy.
    pub async fn write_policy(&self, name: &str, policy_hcl: &str) -> Result<()> {
        #[derive(Serialize)]
        struct PolicyRequest<'a> {
            policy: &'a str,
        }

        let resp = self
            .request(
                reqwest::Method::PUT,
                &format!("sys/policies/acl/{name}"),
            )
            .json(&PolicyRequest { policy: policy_hcl })
            .send()
            .await
            .context("Failed to write policy")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Write policy {name} returned {status}: {body}");
        }
        Ok(())
    }

    /// Write to an arbitrary API path (for auth config, roles, database config, etc.).
    pub async fn write(
        &self,
        path: &str,
        data: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let resp = self
            .request(reqwest::Method::POST, path)
            .json(data)
            .send()
            .await
            .with_context(|| format!("Failed to write to {path}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Write {path} returned {status}: {body}");
        }

        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            Ok(serde_json::Value::Null)
        } else {
            serde_json::from_str(&body).context("Failed to parse write response")
        }
    }

    /// Read from an arbitrary API path.
    pub async fn read(&self, path: &str) -> Result<Option<serde_json::Value>> {
        let resp = self
            .request(reqwest::Method::GET, path)
            .send()
            .await
            .with_context(|| format!("Failed to read {path}"))?;

        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Read {path} returned {status}: {body}");
        }

        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            Ok(Some(serde_json::Value::Null))
        } else {
            Ok(Some(serde_json::from_str(&body)?))
        }
    }

    // ── Database secrets engine ─────────────────────────────────────────

    /// Configure the database secrets engine connection.
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
        self.write(&format!("database/config/{name}"), &data).await?;
        Ok(())
    }

    /// Create a database static role.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_url_construction() {
        let client = BaoClient::new("http://localhost:8200");
        assert_eq!(client.url("sys/seal-status"), "http://localhost:8200/v1/sys/seal-status");
        assert_eq!(client.url("/sys/seal-status"), "http://localhost:8200/v1/sys/seal-status");
    }

    #[test]
    fn test_client_url_strips_trailing_slash() {
        let client = BaoClient::new("http://localhost:8200/");
        assert_eq!(client.base_url, "http://localhost:8200");
    }

    #[test]
    fn test_with_token() {
        let client = BaoClient::with_token("http://localhost:8200", "mytoken");
        assert_eq!(client.token, Some("mytoken".to_string()));
    }

    #[test]
    fn test_new_has_no_token() {
        let client = BaoClient::new("http://localhost:8200");
        assert!(client.token.is_none());
    }
}
