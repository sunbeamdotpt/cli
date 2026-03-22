//! Grafana API client.

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::Result;
use reqwest::Method;
use super::types::{self, *};

/// Client for the Grafana HTTP API (`/api`).
pub struct GrafanaClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for GrafanaClient {
    fn service_name(&self) -> &'static str {
        "grafana"
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

impl GrafanaClient {
    /// Build a GrafanaClient from domain (e.g. `https://metrics.{domain}/api`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://metrics.{domain}/api");
        Self::from_parts(base_url, AuthMethod::None)
    }

    // -- Dashboards ---------------------------------------------------------

    /// Create a new dashboard.
    pub async fn create_dashboard(
        &self,
        body: &serde_json::Value,
    ) -> Result<DashboardResponse> {
        self.transport
            .json(Method::POST, "dashboards/db", Some(body), "grafana create dashboard")
            .await
    }

    /// Get a dashboard by UID.
    pub async fn get_dashboard(&self, uid: &str) -> Result<DashboardResponse> {
        self.transport
            .json(
                Method::GET,
                &format!("dashboards/uid/{uid}"),
                Option::<&()>::None,
                "grafana get dashboard",
            )
            .await
    }

    /// Update an existing dashboard (same endpoint as create).
    pub async fn update_dashboard(
        &self,
        body: &serde_json::Value,
    ) -> Result<DashboardResponse> {
        self.transport
            .json(Method::POST, "dashboards/db", Some(body), "grafana update dashboard")
            .await
    }

    /// Delete a dashboard by UID.
    pub async fn delete_dashboard(&self, uid: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("dashboards/uid/{uid}"),
                Option::<&()>::None,
                "grafana delete dashboard",
            )
            .await
    }

    /// List all dashboards.
    pub async fn list_dashboards(&self) -> Result<Vec<DashboardSearchResult>> {
        self.transport
            .json(
                Method::GET,
                "search?type=dash-db",
                Option::<&()>::None,
                "grafana list dashboards",
            )
            .await
    }

    /// Search dashboards by query.
    pub async fn search_dashboards(
        &self,
        query: &str,
    ) -> Result<Vec<DashboardSearchResult>> {
        self.transport
            .json(
                Method::GET,
                &format!("search?query={}", types::urlencode(query)),
                Option::<&()>::None,
                "grafana search dashboards",
            )
            .await
    }

    // -- Datasources --------------------------------------------------------

    /// List all datasources.
    pub async fn list_datasources(&self) -> Result<Vec<Datasource>> {
        self.transport
            .json(Method::GET, "datasources", Option::<&()>::None, "grafana list datasources")
            .await
    }

    /// Get a datasource by numeric ID.
    pub async fn get_datasource(&self, id: u64) -> Result<Datasource> {
        self.transport
            .json(
                Method::GET,
                &format!("datasources/{id}"),
                Option::<&()>::None,
                "grafana get datasource",
            )
            .await
    }

    /// Get a datasource by UID.
    pub async fn get_datasource_by_uid(&self, uid: &str) -> Result<Datasource> {
        self.transport
            .json(
                Method::GET,
                &format!("datasources/uid/{uid}"),
                Option::<&()>::None,
                "grafana get datasource by uid",
            )
            .await
    }

    /// Create a new datasource.
    pub async fn create_datasource(
        &self,
        body: &serde_json::Value,
    ) -> Result<Datasource> {
        // Grafana wraps create response in {"datasource": {...}}
        let resp: serde_json::Value = self
            .transport
            .json(Method::POST, "datasources", Some(body), "grafana create datasource")
            .await?;
        if let Some(inner) = resp.get("datasource") {
            Ok(serde_json::from_value(inner.clone())?)
        } else {
            Ok(serde_json::from_value(resp)?)
        }
    }

    /// Update an existing datasource by numeric ID.
    pub async fn update_datasource(
        &self,
        id: u64,
        body: &serde_json::Value,
    ) -> Result<Datasource> {
        // Grafana wraps update response in {"datasource": {...}}
        let resp: serde_json::Value = self
            .transport
            .json(
                Method::PUT,
                &format!("datasources/{id}"),
                Some(body),
                "grafana update datasource",
            )
            .await?;
        if let Some(inner) = resp.get("datasource") {
            Ok(serde_json::from_value(inner.clone())?)
        } else {
            Ok(serde_json::from_value(resp)?)
        }
    }

    /// Delete a datasource by numeric ID.
    pub async fn delete_datasource(&self, id: u64) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("datasources/{id}"),
                Option::<&()>::None,
                "grafana delete datasource",
            )
            .await
    }

    /// Proxy a request to a datasource.
    pub async fn proxy_datasource(
        &self,
        id: u64,
        path: &str,
    ) -> Result<serde_json::Value> {
        self.transport
            .json(
                Method::GET,
                &format!("datasources/proxy/{id}/{}", path.trim_start_matches('/')),
                Option::<&()>::None,
                "grafana proxy datasource",
            )
            .await
    }

    // -- Folders ------------------------------------------------------------

    /// List all folders.
    pub async fn list_folders(&self) -> Result<Vec<Folder>> {
        self.transport
            .json(Method::GET, "folders", Option::<&()>::None, "grafana list folders")
            .await
    }

    /// Create a folder.
    pub async fn create_folder(&self, body: &serde_json::Value) -> Result<Folder> {
        self.transport
            .json(Method::POST, "folders", Some(body), "grafana create folder")
            .await
    }

    /// Get a folder by UID.
    pub async fn get_folder(&self, uid: &str) -> Result<Folder> {
        self.transport
            .json(
                Method::GET,
                &format!("folders/{uid}"),
                Option::<&()>::None,
                "grafana get folder",
            )
            .await
    }

    /// Update a folder by UID.
    pub async fn update_folder(
        &self,
        uid: &str,
        body: &serde_json::Value,
    ) -> Result<Folder> {
        self.transport
            .json(
                Method::PUT,
                &format!("folders/{uid}"),
                Some(body),
                "grafana update folder",
            )
            .await
    }

    /// Delete a folder by UID.
    pub async fn delete_folder(&self, uid: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("folders/{uid}"),
                Option::<&()>::None,
                "grafana delete folder",
            )
            .await
    }

    // -- Annotations --------------------------------------------------------

    /// List annotations with optional filter params.
    pub async fn list_annotations(
        &self,
        params: Option<&str>,
    ) -> Result<Vec<Annotation>> {
        let path = match params {
            Some(p) => format!("annotations?{p}"),
            None => "annotations".to_string(),
        };
        self.transport
            .json(Method::GET, &path, Option::<&()>::None, "grafana list annotations")
            .await
    }

    /// Create an annotation.
    pub async fn create_annotation(
        &self,
        body: &serde_json::Value,
    ) -> Result<AnnotationResponse> {
        self.transport
            .json(Method::POST, "annotations", Some(body), "grafana create annotation")
            .await
    }

    /// Get an annotation by ID.
    pub async fn get_annotation(&self, id: u64) -> Result<Annotation> {
        self.transport
            .json(
                Method::GET,
                &format!("annotations/{id}"),
                Option::<&()>::None,
                "grafana get annotation",
            )
            .await
    }

    /// Update an annotation by ID.
    pub async fn update_annotation(
        &self,
        id: u64,
        body: &serde_json::Value,
    ) -> Result<AnnotationResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("annotations/{id}"),
                Some(body),
                "grafana update annotation",
            )
            .await
    }

    /// Delete an annotation by ID.
    pub async fn delete_annotation(&self, id: u64) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("annotations/{id}"),
                Option::<&()>::None,
                "grafana delete annotation",
            )
            .await
    }

    // -- Alerts -------------------------------------------------------------

    /// Get all provisioned alert rules.
    pub async fn get_alert_rules(&self) -> Result<Vec<AlertRule>> {
        self.transport
            .json(
                Method::GET,
                "v1/provisioning/alert-rules",
                Option::<&()>::None,
                "grafana get alert rules",
            )
            .await
    }

    /// Create a provisioned alert rule.
    pub async fn create_alert_rule(
        &self,
        body: &serde_json::Value,
    ) -> Result<AlertRule> {
        self.transport
            .json(
                Method::POST,
                "v1/provisioning/alert-rules",
                Some(body),
                "grafana create alert rule",
            )
            .await
    }

    /// Update a provisioned alert rule by UID.
    pub async fn update_alert_rule(
        &self,
        uid: &str,
        body: &serde_json::Value,
    ) -> Result<AlertRule> {
        self.transport
            .json(
                Method::PUT,
                &format!("v1/provisioning/alert-rules/{uid}"),
                Some(body),
                "grafana update alert rule",
            )
            .await
    }

    /// Delete a provisioned alert rule by UID.
    pub async fn delete_alert_rule(&self, uid: &str) -> Result<()> {
        self.transport
            .send(
                Method::DELETE,
                &format!("v1/provisioning/alert-rules/{uid}"),
                Option::<&()>::None,
                "grafana delete alert rule",
            )
            .await
    }

    // -- Org ----------------------------------------------------------------

    /// Get the current organization.
    pub async fn get_current_org(&self) -> Result<Organization> {
        self.transport
            .json(Method::GET, "org", Option::<&()>::None, "grafana get current org")
            .await
    }

    /// Update the current organization.
    pub async fn update_org(&self, body: &serde_json::Value) -> Result<()> {
        self.transport
            .send(Method::PUT, "org", Some(body), "grafana update org")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = GrafanaClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://metrics.sunbeam.pt/api");
        assert_eq!(c.service_name(), "grafana");
    }

    #[test]
    fn test_from_parts() {
        let c = GrafanaClient::from_parts(
            "http://localhost:3000/api".into(),
            AuthMethod::None,
        );
        assert_eq!(c.base_url(), "http://localhost:3000/api");
    }
}
