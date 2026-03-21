//! OpenSearch client.

#[cfg(feature = "cli")]
pub mod cli;
pub mod types;

use crate::client::{AuthMethod, HttpTransport, ServiceClient};
use crate::error::{Result, ResultExt, SunbeamError};
use reqwest::Method;
use serde_json::Value;
use types::*;

/// Client for the OpenSearch HTTP API.
pub struct OpenSearchClient {
    pub(crate) transport: HttpTransport,
}

impl ServiceClient for OpenSearchClient {
    fn service_name(&self) -> &'static str {
        "opensearch"
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

impl OpenSearchClient {
    /// Build an OpenSearchClient from domain (e.g. `https://search.{domain}`).
    pub fn connect(domain: &str) -> Self {
        let base_url = format!("https://search.{domain}");
        Self::from_parts(base_url, AuthMethod::Bearer(String::new()))
    }

    /// Replace the bearer token.
    pub fn set_token(&mut self, token: String) {
        self.transport.set_auth(AuthMethod::Bearer(token));
    }

    // -----------------------------------------------------------------------
    // Documents
    // -----------------------------------------------------------------------

    /// Index a document with an explicit ID.
    pub async fn index_doc(
        &self,
        index: &str,
        id: &str,
        body: &Value,
    ) -> Result<IndexResponse> {
        self.transport
            .json(Method::PUT, &format!("{index}/_doc/{id}"), Some(body), "opensearch index doc")
            .await
    }

    /// Index a document with an auto-generated ID.
    pub async fn index_doc_auto_id(
        &self,
        index: &str,
        body: &Value,
    ) -> Result<IndexResponse> {
        self.transport
            .json(Method::POST, &format!("{index}/_doc"), Some(body), "opensearch index doc auto")
            .await
    }

    /// Get a document by ID.
    pub async fn get_doc(&self, index: &str, id: &str) -> Result<GetResponse> {
        self.transport
            .json(
                Method::GET,
                &format!("{index}/_doc/{id}"),
                Option::<&()>::None,
                "opensearch get doc",
            )
            .await
    }

    /// Check if a document exists (HEAD request, returns bool).
    pub async fn head_doc(&self, index: &str, id: &str) -> Result<bool> {
        let resp = self
            .transport
            .request(Method::HEAD, &format!("{index}/_doc/{id}"))
            .send()
            .await
            .with_ctx(|| "opensearch head doc: request failed".to_string())?;
        Ok(resp.status().is_success())
    }

    /// Delete a document by ID.
    pub async fn delete_doc(&self, index: &str, id: &str) -> Result<DeleteResponse> {
        self.transport
            .json(
                Method::DELETE,
                &format!("{index}/_doc/{id}"),
                Option::<&()>::None,
                "opensearch delete doc",
            )
            .await
    }

    /// Update a document by ID.
    pub async fn update_doc(
        &self,
        index: &str,
        id: &str,
        body: &Value,
    ) -> Result<UpdateResponse> {
        self.transport
            .json(Method::POST, &format!("{index}/_update/{id}"), Some(body), "opensearch update doc")
            .await
    }

    /// Bulk index/update/delete operations.
    pub async fn bulk(&self, body: &Value) -> Result<BulkResponse> {
        self.transport
            .json(Method::POST, "_bulk", Some(body), "opensearch bulk")
            .await
    }

    /// Multi-get documents.
    pub async fn multi_get(&self, body: &Value) -> Result<MultiGetResponse> {
        self.transport
            .json(Method::POST, "_mget", Some(body), "opensearch mget")
            .await
    }

    /// Reindex documents from one index to another.
    pub async fn reindex(&self, body: &Value) -> Result<ReindexResponse> {
        self.transport
            .json(Method::POST, "_reindex", Some(body), "opensearch reindex")
            .await
    }

    /// Delete documents matching a query.
    pub async fn delete_by_query(
        &self,
        index: &str,
        body: &Value,
    ) -> Result<DeleteByQueryResponse> {
        self.transport
            .json(
                Method::POST,
                &format!("{index}/_delete_by_query"),
                Some(body),
                "opensearch delete by query",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Search an index.
    pub async fn search(&self, index: &str, body: &Value) -> Result<SearchResponse> {
        self.transport
            .json(Method::POST, &format!("{index}/_search"), Some(body), "opensearch search")
            .await
    }

    /// Search across all indices.
    pub async fn search_all(&self, body: &Value) -> Result<SearchResponse> {
        self.transport
            .json(Method::POST, "_search", Some(body), "opensearch search all")
            .await
    }

    /// Multi-search.
    pub async fn multi_search(&self, body: &Value) -> Result<MultiSearchResponse> {
        self.transport
            .json(Method::POST, "_msearch", Some(body), "opensearch msearch")
            .await
    }

    /// Count documents matching a query.
    pub async fn count(&self, index: &str, body: &Value) -> Result<CountResponse> {
        self.transport
            .json(Method::POST, &format!("{index}/_count"), Some(body), "opensearch count")
            .await
    }

    /// Scroll through search results.
    pub async fn scroll(&self, body: &Value) -> Result<SearchResponse> {
        self.transport
            .json(Method::POST, "_search/scroll", Some(body), "opensearch scroll")
            .await
    }

    /// Clear a scroll context.
    pub async fn clear_scroll(&self, body: &Value) -> Result<()> {
        self.transport
            .send(Method::DELETE, "_search/scroll", Some(body), "opensearch clear scroll")
            .await
    }

    /// Get search shard information.
    pub async fn search_shards(&self, index: &str) -> Result<ShardsResponse> {
        self.transport
            .json(
                Method::GET,
                &format!("{index}/_search_shards"),
                Option::<&()>::None,
                "opensearch search shards",
            )
            .await
    }

    /// Execute a search template.
    pub async fn search_template(&self, body: &Value) -> Result<SearchResponse> {
        self.transport
            .json(Method::POST, "_search/template", Some(body), "opensearch search template")
            .await
    }

    // -----------------------------------------------------------------------
    // Indices
    // -----------------------------------------------------------------------

    /// Create an index.
    pub async fn create_index(&self, index: &str, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(Method::PUT, index, Some(body), "opensearch create index")
            .await
    }

    /// Delete an index.
    pub async fn delete_index(&self, index: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::DELETE,
                index,
                Option::<&()>::None,
                "opensearch delete index",
            )
            .await
    }

    /// Get index metadata.
    pub async fn get_index(&self, index: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                index,
                Option::<&()>::None,
                "opensearch get index",
            )
            .await
    }

    /// Check if an index exists (HEAD request, returns bool).
    pub async fn index_exists(&self, index: &str) -> Result<bool> {
        let resp = self
            .transport
            .request(Method::HEAD, index)
            .send()
            .await
            .with_ctx(|| "opensearch index exists: request failed".to_string())?;
        Ok(resp.status().is_success())
    }

    /// Get index settings.
    pub async fn get_settings(&self, index: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                &format!("{index}/_settings"),
                Option::<&()>::None,
                "opensearch get settings",
            )
            .await
    }

    /// Update index settings.
    pub async fn update_settings(&self, index: &str, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("{index}/_settings"),
                Some(body),
                "opensearch update settings",
            )
            .await
    }

    /// Get index mapping.
    pub async fn get_mapping(&self, index: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                &format!("{index}/_mapping"),
                Option::<&()>::None,
                "opensearch get mapping",
            )
            .await
    }

    /// Put (update) index mapping.
    pub async fn put_mapping(&self, index: &str, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("{index}/_mapping"),
                Some(body),
                "opensearch put mapping",
            )
            .await
    }

    /// Get aliases for an index.
    pub async fn get_aliases(&self, index: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                &format!("{index}/_alias"),
                Option::<&()>::None,
                "opensearch get aliases",
            )
            .await
    }

    /// Create or update aliases (bulk alias action).
    pub async fn create_alias(&self, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(Method::POST, "_aliases", Some(body), "opensearch create alias")
            .await
    }

    /// Delete an alias from an index.
    pub async fn delete_alias(&self, index: &str, alias: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::DELETE,
                &format!("{index}/_alias/{alias}"),
                Option::<&()>::None,
                "opensearch delete alias",
            )
            .await
    }

    /// Create or update an index template.
    pub async fn create_template(&self, name: &str, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("_index_template/{name}"),
                Some(body),
                "opensearch create template",
            )
            .await
    }

    /// Delete an index template.
    pub async fn delete_template(&self, name: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::DELETE,
                &format!("_index_template/{name}"),
                Option::<&()>::None,
                "opensearch delete template",
            )
            .await
    }

    /// Get an index template.
    pub async fn get_template(&self, name: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                &format!("_index_template/{name}"),
                Option::<&()>::None,
                "opensearch get template",
            )
            .await
    }

    /// Open a closed index.
    pub async fn open_index(&self, index: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::POST,
                &format!("{index}/_open"),
                Option::<&()>::None,
                "opensearch open index",
            )
            .await
    }

    /// Close an index.
    pub async fn close_index(&self, index: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::POST,
                &format!("{index}/_close"),
                Option::<&()>::None,
                "opensearch close index",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Cluster
    // -----------------------------------------------------------------------

    /// Get cluster health.
    pub async fn cluster_health(&self) -> Result<ClusterHealth> {
        self.transport
            .json(
                Method::GET,
                "_cluster/health",
                Option::<&()>::None,
                "opensearch cluster health",
            )
            .await
    }

    /// Get cluster state.
    pub async fn cluster_state(&self) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                "_cluster/state",
                Option::<&()>::None,
                "opensearch cluster state",
            )
            .await
    }

    /// Get cluster stats.
    pub async fn cluster_stats(&self) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                "_cluster/stats",
                Option::<&()>::None,
                "opensearch cluster stats",
            )
            .await
    }

    /// Get cluster settings.
    pub async fn cluster_settings(&self) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                "_cluster/settings",
                Option::<&()>::None,
                "opensearch cluster settings",
            )
            .await
    }

    /// Update cluster settings.
    pub async fn update_cluster_settings(&self, body: &Value) -> Result<Value> {
        self.transport
            .json(
                Method::PUT,
                "_cluster/settings",
                Some(body),
                "opensearch update cluster settings",
            )
            .await
    }

    /// Explain shard allocation.
    pub async fn allocation_explain(&self, body: &Value) -> Result<Value> {
        self.transport
            .json(
                Method::POST,
                "_cluster/allocation/explain",
                Some(body),
                "opensearch allocation explain",
            )
            .await
    }

    /// Reroute shards.
    pub async fn reroute(&self, body: &Value) -> Result<Value> {
        self.transport
            .json(
                Method::POST,
                "_cluster/reroute",
                Some(body),
                "opensearch reroute",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Nodes
    // -----------------------------------------------------------------------

    /// Get nodes info.
    pub async fn nodes_info(&self) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                "_nodes",
                Option::<&()>::None,
                "opensearch nodes info",
            )
            .await
    }

    /// Get nodes stats.
    pub async fn nodes_stats(&self) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                "_nodes/stats",
                Option::<&()>::None,
                "opensearch nodes stats",
            )
            .await
    }

    /// Get nodes hot threads (returns plain text).
    pub async fn nodes_hot_threads(&self) -> Result<String> {
        let bytes = self
            .transport
            .bytes(Method::GET, "_nodes/hot_threads", "opensearch hot threads")
            .await?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| SunbeamError::network(format!("opensearch hot threads: invalid UTF-8: {e}")))
    }

    // -----------------------------------------------------------------------
    // Cat
    // -----------------------------------------------------------------------

    /// List indices via the cat API.
    pub async fn cat_indices(&self) -> Result<Vec<CatIndex>> {
        self.transport
            .json(
                Method::GET,
                "_cat/indices?format=json",
                Option::<&()>::None,
                "opensearch cat indices",
            )
            .await
    }

    /// List nodes via the cat API.
    pub async fn cat_nodes(&self) -> Result<Vec<CatNode>> {
        self.transport
            .json(
                Method::GET,
                "_cat/nodes?format=json",
                Option::<&()>::None,
                "opensearch cat nodes",
            )
            .await
    }

    /// List shards via the cat API.
    pub async fn cat_shards(&self) -> Result<Vec<CatShard>> {
        self.transport
            .json(
                Method::GET,
                "_cat/shards?format=json",
                Option::<&()>::None,
                "opensearch cat shards",
            )
            .await
    }

    /// Cluster health via the cat API.
    pub async fn cat_health(&self) -> Result<Vec<CatHealth>> {
        self.transport
            .json(
                Method::GET,
                "_cat/health?format=json",
                Option::<&()>::None,
                "opensearch cat health",
            )
            .await
    }

    /// Allocation info via the cat API.
    pub async fn cat_allocation(&self) -> Result<Vec<CatAllocation>> {
        self.transport
            .json(
                Method::GET,
                "_cat/allocation?format=json",
                Option::<&()>::None,
                "opensearch cat allocation",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Ingest
    // -----------------------------------------------------------------------

    /// Create or update an ingest pipeline.
    pub async fn create_pipeline(&self, id: &str, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("_ingest/pipeline/{id}"),
                Some(body),
                "opensearch create pipeline",
            )
            .await
    }

    /// Get an ingest pipeline.
    pub async fn get_pipeline(&self, id: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                &format!("_ingest/pipeline/{id}"),
                Option::<&()>::None,
                "opensearch get pipeline",
            )
            .await
    }

    /// Delete an ingest pipeline.
    pub async fn delete_pipeline(&self, id: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::DELETE,
                &format!("_ingest/pipeline/{id}"),
                Option::<&()>::None,
                "opensearch delete pipeline",
            )
            .await
    }

    /// Simulate an ingest pipeline.
    pub async fn simulate_pipeline(&self, id: &str, body: &Value) -> Result<Value> {
        self.transport
            .json(
                Method::POST,
                &format!("_ingest/pipeline/{id}/_simulate"),
                Some(body),
                "opensearch simulate pipeline",
            )
            .await
    }

    /// Get all ingest pipelines.
    pub async fn get_all_pipelines(&self) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                "_ingest/pipeline",
                Option::<&()>::None,
                "opensearch get all pipelines",
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Snapshots
    // -----------------------------------------------------------------------

    /// Create or update a snapshot repository.
    pub async fn create_snapshot_repo(&self, name: &str, body: &Value) -> Result<AckResponse> {
        self.transport
            .json(
                Method::PUT,
                &format!("_snapshot/{name}"),
                Some(body),
                "opensearch create snapshot repo",
            )
            .await
    }

    /// Delete a snapshot repository.
    pub async fn delete_snapshot_repo(&self, name: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::DELETE,
                &format!("_snapshot/{name}"),
                Option::<&()>::None,
                "opensearch delete snapshot repo",
            )
            .await
    }

    /// Create a snapshot.
    pub async fn create_snapshot(
        &self,
        repo: &str,
        name: &str,
        body: &Value,
    ) -> Result<Value> {
        self.transport
            .json(
                Method::PUT,
                &format!("_snapshot/{repo}/{name}"),
                Some(body),
                "opensearch create snapshot",
            )
            .await
    }

    /// Delete a snapshot.
    pub async fn delete_snapshot(&self, repo: &str, name: &str) -> Result<AckResponse> {
        self.transport
            .json(
                Method::DELETE,
                &format!("_snapshot/{repo}/{name}"),
                Option::<&()>::None,
                "opensearch delete snapshot",
            )
            .await
    }

    /// List all snapshots in a repository.
    pub async fn list_snapshots(&self, repo: &str) -> Result<Value> {
        self.transport
            .json(
                Method::GET,
                &format!("_snapshot/{repo}/_all"),
                Option::<&()>::None,
                "opensearch list snapshots",
            )
            .await
    }

    /// Restore a snapshot.
    pub async fn restore_snapshot(
        &self,
        repo: &str,
        name: &str,
        body: &Value,
    ) -> Result<Value> {
        self.transport
            .json(
                Method::POST,
                &format!("_snapshot/{repo}/{name}/_restore"),
                Some(body),
                "opensearch restore snapshot",
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_url() {
        let c = OpenSearchClient::connect("sunbeam.pt");
        assert_eq!(c.base_url(), "https://search.sunbeam.pt");
        assert_eq!(c.service_name(), "opensearch");
    }

    #[test]
    fn test_from_parts() {
        let c = OpenSearchClient::from_parts(
            "http://localhost:9200".into(),
            AuthMethod::Bearer("tok".into()),
        );
        assert_eq!(c.base_url(), "http://localhost:9200");
    }

    #[test]
    fn test_set_token() {
        let mut c = OpenSearchClient::connect("example.com");
        c.set_token("my-token".into());
        assert!(matches!(c.transport.auth, AuthMethod::Bearer(ref s) if s == "my-token"));
    }

    #[tokio::test]
    async fn test_head_doc_unreachable() {
        let c = OpenSearchClient::from_parts(
            "http://127.0.0.1:19998".into(),
            AuthMethod::None,
        );
        let result = c.head_doc("test", "1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_index_exists_unreachable() {
        let c = OpenSearchClient::from_parts(
            "http://127.0.0.1:19998".into(),
            AuthMethod::None,
        );
        let result = c.index_exists("test").await;
        assert!(result.is_err());
    }
}
