//! Prometheus API client.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::{self, *};

/// Client for the Prometheus HTTP API (`/api/v1`).
pub struct PrometheusClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for PrometheusClient {
    fn service_name(&self) -> &'static str {
        "prometheus"
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

impl PrometheusClient {
    /// Build a PrometheusClient from domain (e.g. `https://systemmetrics.{domain}/api/v1`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://systemmetrics.{domain}/api/v1");
        Self::from_parts(base_url, AuthMethod::None)
    }

    // -- Query --------------------------------------------------------------

    /// Execute an instant query.
    pub async fn query(
        &self,
        query: &str,
        time: Option<&str>,
    ) -> Result<QueryResult> {
        let mut path = format!("query?query={}", types::urlencode(query));
        if let Some(t) = time {
            path.push_str(&format!("&time={t}"));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "prometheus query")
            .await
    }

    /// Execute a range query.
    pub async fn query_range(
        &self,
        query: &str,
        start: &str,
        end: &str,
        step: &str,
    ) -> Result<QueryResult> {
        let path = format!(
            "query_range?query={}&start={start}&end={end}&step={step}",
            types::urlencode(query),
        );
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "prometheus query_range")
            .await
    }

    /// Format a PromQL expression.
    pub async fn format_query(&self, query: &str) -> Result<FormattedQuery> {
        let path = format!("format_query?query={}", types::urlencode(query));
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "prometheus format_query")
            .await
    }

    // -- Metadata -----------------------------------------------------------

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
            .json(Method::GET, &path, Option::<&()>::None, "prometheus series")
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
            .json(Method::GET, &path, Option::<&()>::None, "prometheus labels")
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
            .json(Method::GET, &path, Option::<&()>::None, "prometheus label values")
            .await
    }

    /// Get metadata about metrics scraped by targets.
    pub async fn targets_metadata(
        &self,
        metric: Option<&str>,
    ) -> Result<ApiResponse<Vec<serde_json::Value>>> {
        let mut path = String::from("targets/metadata");
        if let Some(m) = metric {
            path.push_str(&format!("?metric={}", types::urlencode(m)));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "prometheus targets metadata")
            .await
    }

    /// Get per-metric metadata.
    pub async fn metadata(
        &self,
        metric: Option<&str>,
    ) -> Result<ApiResponse<serde_json::Value>> {
        let mut path = String::from("metadata");
        if let Some(m) = metric {
            path.push_str(&format!("?metric={}", types::urlencode(m)));
        }
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "prometheus metadata")
            .await
    }

    // -- Infrastructure -----------------------------------------------------

    /// Get current target discovery status.
    pub async fn targets(&self) -> Result<TargetsResult> {
        self.transport
            .json(Method::GET, "targets", Option::<&()>::None, "prometheus targets")
            .await
    }

    /// List scrape pools.
    pub async fn scrape_pools(&self) -> Result<ApiResponse<serde_json::Value>> {
        self.transport
            .json(Method::GET, "scrape_pools", Option::<&()>::None, "prometheus scrape_pools")
            .await
    }

    /// Get discovered Alertmanager instances.
    pub async fn alertmanagers(&self) -> Result<ApiResponse<serde_json::Value>> {
        self.transport
            .json(Method::GET, "alertmanagers", Option::<&()>::None, "prometheus alertmanagers")
            .await
    }

    /// Get alerting and recording rules.
    pub async fn rules(&self) -> Result<RulesResult> {
        self.transport
            .json(Method::GET, "rules", Option::<&()>::None, "prometheus rules")
            .await
    }

    /// Get active alerts.
    pub async fn alerts(&self) -> Result<AlertsResult> {
        self.transport
            .json(Method::GET, "alerts", Option::<&()>::None, "prometheus alerts")
            .await
    }

    // -- Status -------------------------------------------------------------

    /// Get Prometheus configuration.
    pub async fn config(&self) -> Result<ConfigResult> {
        self.transport
            .json(Method::GET, "status/config", Option::<&()>::None, "prometheus config")
            .await
    }

    /// Get command-line flags.
    pub async fn flags(&self) -> Result<ApiResponse<serde_json::Value>> {
        self.transport
            .json(Method::GET, "status/flags", Option::<&()>::None, "prometheus flags")
            .await
    }

    /// Get runtime information.
    pub async fn runtime_info(&self) -> Result<ApiResponse<serde_json::Value>> {
        self.transport
            .json(Method::GET, "status/runtimeinfo", Option::<&()>::None, "prometheus runtimeinfo")
            .await
    }

    /// Get build information.
    pub async fn build_info(&self) -> Result<ApiResponse<serde_json::Value>> {
        self.transport
            .json(Method::GET, "status/buildinfo", Option::<&()>::None, "prometheus buildinfo")
            .await
    }

    /// Get TSDB statistics.
    pub async fn tsdb(&self) -> Result<ApiResponse<serde_json::Value>> {
        self.transport
            .json(Method::GET, "status/tsdb", Option::<&()>::None, "prometheus tsdb")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = PrometheusClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://systemmetrics.sunbeam.pt/api/v1");
        assert_eq!(c.service_name(), "prometheus");
    }

    #[test]
    fn test_from_parts() {
        let c = PrometheusClient::from_parts(
            "http://localhost:9090/api/v1".into(),
            AuthMethod::None,
        );
        assert_eq!(c.base_url(), "http://localhost:9090/api/v1");
    }
}
