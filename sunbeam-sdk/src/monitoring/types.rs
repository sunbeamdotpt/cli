//! Shared types for monitoring clients (Prometheus, Loki, Grafana).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Prometheus / Loki shared response wrapper
// ---------------------------------------------------------------------------

/// Standard Prometheus/Loki API response envelope.
///
/// Both APIs return `{"status":"success","data":{...}}` on success.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub status: String,
    pub data: Option<T>,
    #[serde(default, rename = "errorType")]
    pub error_type: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Prometheus types
// ---------------------------------------------------------------------------

/// Result of an instant or range query.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryData {
    pub result_type: String,
    pub result: serde_json::Value,
}

/// Alias: Prometheus/Loki query result is an `ApiResponse<QueryData>`.
pub type QueryResult = ApiResponse<QueryData>;

/// Formatted PromQL query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormattedQuery {
    pub status: String,
    pub data: String,
}

/// Prometheus targets response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsData {
    pub active_targets: Vec<serde_json::Value>,
    #[serde(default)]
    pub dropped_targets: Vec<serde_json::Value>,
}

pub type TargetsResult = ApiResponse<TargetsData>;

/// Prometheus rules response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesData {
    pub groups: Vec<serde_json::Value>,
}

pub type RulesResult = ApiResponse<RulesData>;

/// Prometheus alerts response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertsData {
    pub alerts: Vec<serde_json::Value>,
}

pub type AlertsResult = ApiResponse<AlertsData>;

/// Prometheus config response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigData {
    pub yaml: String,
}

pub type ConfigResult = ApiResponse<ConfigData>;

// ---------------------------------------------------------------------------
// Loki types
// ---------------------------------------------------------------------------

/// Loki readiness status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadyStatus {
    #[serde(default)]
    pub status: Option<String>,
}

// ---------------------------------------------------------------------------
// Grafana types
// ---------------------------------------------------------------------------

/// Grafana dashboard create/update response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardResponse {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub slug: Option<String>,
    /// Present on GET responses.
    #[serde(default)]
    pub dashboard: Option<serde_json::Value>,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
}

/// Grafana dashboard search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSearchResult {
    pub id: u64,
    pub uid: String,
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub is_starred: Option<bool>,
    #[serde(default, rename = "folderUid")]
    pub folder_uid: Option<String>,
}

/// Grafana datasource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Datasource {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub uid: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub access: Option<String>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub json_data: Option<serde_json::Value>,
    #[serde(default)]
    pub database: Option<String>,
}

/// Grafana folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: u64,
    pub uid: String,
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
}

/// Grafana annotation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub dashboard_id: Option<u64>,
    #[serde(default)]
    pub panel_id: Option<u64>,
    #[serde(default)]
    pub time: Option<u64>,
    #[serde(default)]
    pub time_end: Option<u64>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Grafana annotation create/update response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationResponse {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Grafana alert rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub data: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "folderUID")]
    pub folder_uid: Option<String>,
    #[serde(default, rename = "ruleGroup")]
    pub rule_group: Option<String>,
    #[serde(default, rename = "for")]
    pub for_duration: Option<String>,
    #[serde(default)]
    pub labels: Option<serde_json::Value>,
    #[serde(default)]
    pub annotations: Option<serde_json::Value>,
}

/// Grafana organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub address: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for URL query parameters.
pub(super) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response_deserialize_success() {
        let json = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;
        let resp: ApiResponse<QueryData> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "success");
        assert_eq!(resp.data.unwrap().result_type, "vector");
    }

    #[test]
    fn test_api_response_deserialize_with_warnings() {
        let json = r#"{
            "status": "success",
            "data": {"resultType":"matrix","result":[]},
            "warnings": ["some warning"]
        }"#;
        let resp: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(resp.warnings.unwrap().len(), 1);
    }

    #[test]
    fn test_api_response_deserialize_error() {
        let json = r#"{
            "status": "error",
            "errorType": "bad_data",
            "error": "invalid query",
            "data": {"resultType":"","result":[]}
        }"#;
        let resp: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "error");
        assert_eq!(resp.error_type.unwrap(), "bad_data");
    }

    #[test]
    fn test_dashboard_search_result_roundtrip() {
        let item = DashboardSearchResult {
            id: 1,
            uid: "abc123".into(),
            title: "My Dashboard".into(),
            url: Some("/d/abc123".into()),
            kind: Some("dash-db".into()),
            uri: None,
            tags: vec!["prod".into()],
            is_starred: Some(false),
            folder_uid: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: DashboardSearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.uid, "abc123");
    }

    #[test]
    fn test_datasource_roundtrip() {
        let ds = Datasource {
            id: Some(1),
            uid: Some("prom".into()),
            name: "Prometheus".into(),
            kind: "prometheus".into(),
            url: Some("http://prometheus:9090".into()),
            access: Some("proxy".into()),
            is_default: Some(true),
            json_data: None,
            database: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        let back: Datasource = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "Prometheus");
    }

    #[test]
    fn test_ready_status_minimal() {
        let json = r#"{}"#;
        let rs: ReadyStatus = serde_json::from_str(json).unwrap();
        assert!(rs.status.is_none());
    }

    #[test]
    fn test_config_result_deserialize() {
        let json = r#"{"status":"success","data":{"yaml":"global:\n  scrape_interval: 15s"}}"#;
        let resp: ConfigResult = serde_json::from_str(json).unwrap();
        assert!(resp.data.unwrap().yaml.contains("scrape_interval"));
    }

    #[test]
    fn test_alert_rule_roundtrip() {
        let rule = AlertRule {
            uid: Some("rule-1".into()),
            title: Some("High CPU".into()),
            condition: Some("A".into()),
            data: None,
            folder_uid: Some("folder-1".into()),
            rule_group: Some("group-1".into()),
            for_duration: Some("5m".into()),
            labels: None,
            annotations: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let back: AlertRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.uid.unwrap(), "rule-1");
    }
}
