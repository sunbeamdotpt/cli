//! Loki API client.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::{self, *};

/// Client for the Loki HTTP API (`/loki/api/v1`).
pub struct LokiClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for LokiClient {
    fn service_name(&self) -> &'static str {
        "loki"
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

impl LokiClient {
    /// Build a LokiClient from domain (e.g. `https://loki.{domain}/loki/api/v1`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://loki.{domain}/loki/api/v1");
        Self::from_parts(base_url, AuthMethod::None)
    }

    // -- Query --------------------------------------------------------------

    /// Execute an instant query.
    pub async fn query(
        &self,
        query: &str,
        limit: Option<u32>,
        time: Option<&str>,
    ) -> Result<QueryResult> {
        let mut path = format!("query?query={}", types::urlencode(query));
        if let Some(l) = limit {
            path.push_str(&format!("&limit={l}"));
        }
        if let Some(t) = time {
            path.push_str(&format!("&time={t}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki query")
            .await
    }

    /// Execute a range query.
    pub async fn query_range(
        &self,
        query: &str,
        start: &str,
        end: &str,
        limit: Option<u32>,
        step: Option<&str>,
    ) -> Result<QueryResult> {
        let mut path = format!(
            "query_range?query={}&start={start}&end={end}",
            types::urlencode(query),
        );
        if let Some(l) = limit {
            path.push_str(&format!("&limit={l}"));
        }
        if let Some(s) = step {
            path.push_str(&format!("&step={s}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki query_range")
            .await
    }

    /// Get all label names.
    pub async fn labels(
        &self,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<ApiResponse<Vec<String>>> {
        let mut path = String::from("labels");
        let mut sep = '?';
        if let Some(s) = start {
            path.push_str(&format!("{sep}start={s}"));
            sep = '&';
        }
        if let Some(e) = end {
            path.push_str(&format!("{sep}end={e}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki labels")
            .await
    }

    /// Get values for a specific label.
    pub async fn label_values(
        &self,
        label: &str,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<ApiResponse<Vec<String>>> {
        let mut path = format!("label/{label}/values");
        let mut sep = '?';
        if let Some(s) = start {
            path.push_str(&format!("{sep}start={s}"));
            sep = '&';
        }
        if let Some(e) = end {
            path.push_str(&format!("{sep}end={e}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki label values")
            .await
    }

    /// Find series matching label matchers.
    pub async fn series(
        &self,
        match_params: &[&str],
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<ApiResponse<Vec<serde_json::Value>>> {
        let mut path = String::from("series?");
        for (i, m) in match_params.iter().enumerate() {
            if i > 0 {
                path.push('&');
            }
            path.push_str(&format!("match[]={}", types::urlencode(m)));
        }
        if let Some(s) = start {
            path.push_str(&format!("&start={s}"));
        }
        if let Some(e) = end {
            path.push_str(&format!("&end={e}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki series")
            .await
    }

    // -- Index --------------------------------------------------------------

    /// Get index statistics.
    pub async fn index_stats(&self) -> Result<serde_json::Value> {
        self.transport
            .json(Method::GET, "index/stats", Option::<&()>::None, "loki index stats")
            .await
    }

    /// Get index volume for a query.
    pub async fn index_volume(
        &self,
        query: &str,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = format!("index/volume?query={}", types::urlencode(query));
        if let Some(s) = start {
            path.push_str(&format!("&start={s}"));
        }
        if let Some(e) = end {
            path.push_str(&format!("&end={e}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki index volume")
            .await
    }

    /// Get index volume range for a query.
    pub async fn index_volume_range(
        &self,
        query: &str,
        start: &str,
        end: &str,
        step: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = format!(
            "index/volume_range?query={}&start={start}&end={end}",
            types::urlencode(query),
        );
        if let Some(s) = step {
            path.push_str(&format!("&step={s}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki index volume_range")
            .await
    }

    // -- Patterns -----------------------------------------------------------

    /// Detect log patterns.
    pub async fn detect_patterns(
        &self,
        query: &str,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = format!("patterns?query={}", types::urlencode(query));
        if let Some(s) = start {
            path.push_str(&format!("&start={s}"));
        }
        if let Some(e) = end {
            path.push_str(&format!("&end={e}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "loki detect patterns")
            .await
    }

    // -- Ingest -------------------------------------------------------------

    /// Push log entries.
    pub async fn push(&self, body: &serde_json::Value) -> Result<()> {
        self.transport
            .send(Method::POST, "push", Some(body), "loki push")
            .await
    }

    // -- Status -------------------------------------------------------------

    /// Check readiness. Note: Loki's `/ready` is at the server root,
    /// not under the API prefix.
    pub async fn ready(&self) -> Result<ReadyStatus> {
        // Build URL from base by stripping the API path suffix
        let base = self.transport.base_url.trim_end_matches("/loki/api/v1");
        let url = format!("{base}/ready");
        let resp = self
            .transport
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| crate::error::SunbeamError::network(format!("loki ready: {e}")))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::error::SunbeamError::network(format!("loki ready: {body}")));
        }
        Ok(ReadyStatus {
            status: Some("ready".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = LokiClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://loki.sunbeam.pt/loki/api/v1");
        assert_eq!(c.service_name(), "loki");
    }

    #[test]
    fn test_from_parts() {
        let c = LokiClient::from_parts(
            "http://localhost:3100/loki/api/v1".into(),
            AuthMethod::None,
        );
        assert_eq!(c.base_url(), "http://localhost:3100/loki/api/v1");
    }
}
