//! OpenSearch response types.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Document responses
// ---------------------------------------------------------------------------

/// Response from index / delete / update operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexResponse {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_version", default)]
    pub version: Option<u64>,
    pub result: Option<String>,
    #[serde(rename = "_shards", default)]
    pub shards: Option<ShardInfo>,
    #[serde(rename = "_seq_no", default)]
    pub seq_no: Option<u64>,
    #[serde(rename = "_primary_term", default)]
    pub primary_term: Option<u64>,
}

/// Shard success/failure counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardInfo {
    pub total: u32,
    pub successful: u32,
    pub failed: u32,
}

/// Response from GET `{index}/_doc/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResponse {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_version", default)]
    pub version: Option<u64>,
    #[serde(rename = "_seq_no", default)]
    pub seq_no: Option<u64>,
    #[serde(rename = "_primary_term", default)]
    pub primary_term: Option<u64>,
    pub found: bool,
    #[serde(rename = "_source", default)]
    pub source: Option<serde_json::Value>,
}

/// Response from DELETE `{index}/_doc/{id}`.
pub type DeleteResponse = IndexResponse;

/// Response from POST `{index}/_update/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResponse {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_version", default)]
    pub version: Option<u64>,
    pub result: Option<String>,
    #[serde(rename = "_shards", default)]
    pub shards: Option<ShardInfo>,
}

// ---------------------------------------------------------------------------
// Bulk
// ---------------------------------------------------------------------------

/// Response from POST `_bulk`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkResponse {
    pub took: u64,
    pub errors: bool,
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Multi-get
// ---------------------------------------------------------------------------

/// Response from POST `_mget`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiGetResponse {
    pub docs: Vec<GetResponse>,
}

// ---------------------------------------------------------------------------
// Reindex
// ---------------------------------------------------------------------------

/// Response from POST `_reindex`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReindexResponse {
    pub took: u64,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub deleted: u64,
    #[serde(default)]
    pub failures: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Delete by query
// ---------------------------------------------------------------------------

/// Response from POST `{index}/_delete_by_query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteByQueryResponse {
    pub took: u64,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub deleted: u64,
    #[serde(default)]
    pub failures: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Response from POST `{index}/_search` and `_search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub took: u64,
    pub timed_out: bool,
    #[serde(rename = "_shards", default)]
    pub shards: Option<ShardInfo>,
    pub hits: HitsEnvelope,
    #[serde(default)]
    pub aggregations: Option<serde_json::Value>,
    #[serde(rename = "_scroll_id", default)]
    pub scroll_id: Option<String>,
}

/// Top-level hits wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitsEnvelope {
    pub total: HitsTotal,
    #[serde(default)]
    pub max_score: Option<f64>,
    #[serde(default)]
    pub hits: Vec<Hit>,
}

/// Total hit count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitsTotal {
    pub value: u64,
    #[serde(default)]
    pub relation: Option<String>,
}

/// A single search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_score", default)]
    pub score: Option<f64>,
    #[serde(rename = "_source", default)]
    pub source: Option<serde_json::Value>,
    #[serde(default)]
    pub highlight: Option<serde_json::Value>,
    #[serde(default)]
    pub sort: Option<Vec<serde_json::Value>>,
}

/// Response from POST `_msearch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSearchResponse {
    pub responses: Vec<SearchResponse>,
}

/// Response from POST `{index}/_count`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountResponse {
    pub count: u64,
    #[serde(rename = "_shards", default)]
    pub shards: Option<ShardInfo>,
}

/// Response from GET `{index}/_search_shards`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardsResponse {
    #[serde(default)]
    pub nodes: serde_json::Value,
    #[serde(default)]
    pub shards: Vec<Vec<serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// Index management
// ---------------------------------------------------------------------------

/// Acknowledged response from index management operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckResponse {
    pub acknowledged: bool,
    #[serde(default)]
    pub shards_acknowledged: Option<bool>,
    #[serde(default)]
    pub index: Option<String>,
}

// ---------------------------------------------------------------------------
// Cluster
// ---------------------------------------------------------------------------

/// Response from GET `_cluster/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterHealth {
    pub cluster_name: String,
    pub status: String,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub number_of_nodes: u32,
    #[serde(default)]
    pub number_of_data_nodes: u32,
    #[serde(default)]
    pub active_primary_shards: u32,
    #[serde(default)]
    pub active_shards: u32,
    #[serde(default)]
    pub relocating_shards: u32,
    #[serde(default)]
    pub initializing_shards: u32,
    #[serde(default)]
    pub unassigned_shards: u32,
}

// ---------------------------------------------------------------------------
// Cat responses
// ---------------------------------------------------------------------------

/// A row from `_cat/indices?format=json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatIndex {
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub index: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub pri: Option<String>,
    #[serde(default)]
    pub rep: Option<String>,
    #[serde(rename = "docs.count", default)]
    pub docs_count: Option<String>,
    #[serde(rename = "docs.deleted", default)]
    pub docs_deleted: Option<String>,
    #[serde(rename = "store.size", default)]
    pub store_size: Option<String>,
    #[serde(rename = "pri.store.size", default)]
    pub pri_store_size: Option<String>,
}

/// A row from `_cat/nodes?format=json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatNode {
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "heap.percent", default)]
    pub heap_percent: Option<String>,
    #[serde(rename = "ram.percent", default)]
    pub ram_percent: Option<String>,
    #[serde(rename = "cpu", default)]
    pub cpu: Option<String>,
    #[serde(rename = "node.role", default)]
    pub node_role: Option<String>,
    #[serde(default)]
    pub master: Option<String>,
}

/// A row from `_cat/shards?format=json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatShard {
    #[serde(default)]
    pub index: Option<String>,
    #[serde(default)]
    pub shard: Option<String>,
    #[serde(default)]
    pub prirep: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub docs: Option<String>,
    #[serde(default)]
    pub store: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
}

/// A row from `_cat/health?format=json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatHealth {
    #[serde(default)]
    pub cluster: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(rename = "node.total", default)]
    pub node_total: Option<String>,
    #[serde(rename = "node.data", default)]
    pub node_data: Option<String>,
    #[serde(default)]
    pub shards: Option<String>,
    #[serde(default)]
    pub pri: Option<String>,
    #[serde(default)]
    pub relo: Option<String>,
    #[serde(default)]
    pub init: Option<String>,
    #[serde(default)]
    pub unassign: Option<String>,
}

/// A row from `_cat/allocation?format=json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatAllocation {
    #[serde(default)]
    pub shards: Option<String>,
    #[serde(rename = "disk.indices", default)]
    pub disk_indices: Option<String>,
    #[serde(rename = "disk.used", default)]
    pub disk_used: Option<String>,
    #[serde(rename = "disk.avail", default)]
    pub disk_avail: Option<String>,
    #[serde(rename = "disk.total", default)]
    pub disk_total: Option<String>,
    #[serde(rename = "disk.percent", default)]
    pub disk_percent: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_index_response() {
        let json = r#"{
            "_index": "test",
            "_id": "1",
            "_version": 1,
            "result": "created",
            "_shards": {"total": 2, "successful": 1, "failed": 0},
            "_seq_no": 0,
            "_primary_term": 1
        }"#;
        let resp: IndexResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.index, "test");
        assert_eq!(resp.id, "1");
        assert_eq!(resp.result.as_deref(), Some("created"));
    }

    #[test]
    fn deserialize_get_response_found() {
        let json = r#"{
            "_index": "test",
            "_id": "1",
            "_version": 1,
            "found": true,
            "_source": {"title": "Hello"}
        }"#;
        let resp: GetResponse = serde_json::from_str(json).unwrap();
        assert!(resp.found);
        assert!(resp.source.is_some());
    }

    #[test]
    fn deserialize_get_response_not_found() {
        let json = r#"{
            "_index": "test",
            "_id": "999",
            "found": false
        }"#;
        let resp: GetResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.found);
        assert!(resp.source.is_none());
    }

    #[test]
    fn deserialize_search_response() {
        let json = r#"{
            "took": 5,
            "timed_out": false,
            "_shards": {"total": 5, "successful": 5, "failed": 0},
            "hits": {
                "total": {"value": 1, "relation": "eq"},
                "max_score": 1.0,
                "hits": [{
                    "_index": "test",
                    "_id": "1",
                    "_score": 1.0,
                    "_source": {"title": "Hello"}
                }]
            }
        }"#;
        let resp: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.took, 5);
        assert!(!resp.timed_out);
        assert_eq!(resp.hits.total.value, 1);
        assert_eq!(resp.hits.hits.len(), 1);
    }

    #[test]
    fn deserialize_bulk_response() {
        let json = r#"{
            "took": 30,
            "errors": false,
            "items": [{"index": {"_index": "test", "_id": "1", "result": "created"}}]
        }"#;
        let resp: BulkResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.took, 30);
        assert!(!resp.errors);
        assert_eq!(resp.items.len(), 1);
    }

    #[test]
    fn deserialize_count_response() {
        let json = r#"{
            "count": 42,
            "_shards": {"total": 5, "successful": 5, "failed": 0}
        }"#;
        let resp: CountResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.count, 42);
    }

    #[test]
    fn deserialize_ack_response() {
        let json = r#"{
            "acknowledged": true,
            "shards_acknowledged": true,
            "index": "my-index"
        }"#;
        let resp: AckResponse = serde_json::from_str(json).unwrap();
        assert!(resp.acknowledged);
        assert_eq!(resp.index.as_deref(), Some("my-index"));
    }

    #[test]
    fn deserialize_cluster_health() {
        let json = r#"{
            "cluster_name": "opensearch-cluster",
            "status": "green",
            "timed_out": false,
            "number_of_nodes": 3,
            "number_of_data_nodes": 3,
            "active_primary_shards": 10,
            "active_shards": 20,
            "relocating_shards": 0,
            "initializing_shards": 0,
            "unassigned_shards": 0
        }"#;
        let resp: ClusterHealth = serde_json::from_str(json).unwrap();
        assert_eq!(resp.cluster_name, "opensearch-cluster");
        assert_eq!(resp.status, "green");
        assert_eq!(resp.number_of_nodes, 3);
    }

    #[test]
    fn deserialize_cat_index() {
        let json = r#"{
            "health": "green",
            "status": "open",
            "index": "my-index",
            "uuid": "abc123",
            "pri": "1",
            "rep": "1",
            "docs.count": "100",
            "store.size": "10mb"
        }"#;
        let resp: CatIndex = serde_json::from_str(json).unwrap();
        assert_eq!(resp.index.as_deref(), Some("my-index"));
        assert_eq!(resp.docs_count.as_deref(), Some("100"));
    }

    #[test]
    fn deserialize_reindex_response() {
        let json = r#"{
            "took": 100,
            "timed_out": false,
            "total": 50,
            "updated": 0,
            "created": 50,
            "deleted": 0,
            "failures": []
        }"#;
        let resp: ReindexResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.took, 100);
        assert_eq!(resp.created, 50);
    }

    #[test]
    fn deserialize_delete_by_query_response() {
        let json = r#"{
            "took": 10,
            "timed_out": false,
            "total": 5,
            "deleted": 5,
            "failures": []
        }"#;
        let resp: DeleteByQueryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.deleted, 5);
    }

    #[test]
    fn deserialize_multi_get_response() {
        let json = r#"{
            "docs": [{
                "_index": "test",
                "_id": "1",
                "found": true,
                "_source": {"title": "Hello"}
            }]
        }"#;
        let resp: MultiGetResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.docs.len(), 1);
        assert!(resp.docs[0].found);
    }
}
