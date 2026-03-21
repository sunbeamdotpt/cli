#![cfg(feature = "integration")]
#![allow(unused_imports)]
mod helpers;
use helpers::*;
use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::search::OpenSearchClient;
use sunbeam_sdk::search::types::*;

const OS_URL: &str = "http://localhost:9200";
const HEALTH_URL: &str = "http://localhost:9200/_cluster/health";

fn os_client() -> OpenSearchClient {
    OpenSearchClient::from_parts(OS_URL.into(), AuthMethod::None)
}

/// Force-refresh an index so documents are searchable.
async fn refresh_index(idx: &str) {
    reqwest::Client::new()
        .post(format!("{OS_URL}/{idx}/_refresh"))
        .send()
        .await
        .unwrap();
}

/// Delete an index, ignoring errors (cleanup helper).
async fn cleanup_index(idx: &str) {
    let _ = os_client().delete_index(idx).await;
}

/// Delete a template, ignoring errors.
async fn cleanup_template(name: &str) {
    let _ = os_client().delete_template(name).await;
}

/// Delete a pipeline, ignoring errors.
async fn cleanup_pipeline(id: &str) {
    let _ = os_client().delete_pipeline(id).await;
}

/// Delete a snapshot repo, ignoring errors.
async fn cleanup_snapshot_repo(name: &str) {
    let _ = os_client().delete_snapshot_repo(name).await;
}

// ---------------------------------------------------------------------------
// 1. Cluster health and info
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_health_and_info() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();

    // cluster_health
    let health = client.cluster_health().await.expect("cluster_health failed");
    assert!(!health.cluster_name.is_empty());
    assert!(
        health.status == "green" || health.status == "yellow" || health.status == "red",
        "unexpected status: {}",
        health.status
    );
    assert!(health.number_of_nodes >= 1);

    // cluster_state
    let state = client.cluster_state().await.expect("cluster_state failed");
    assert!(state.get("cluster_name").is_some());
    assert!(state.get("metadata").is_some());

    // cluster_stats
    let stats = client.cluster_stats().await.expect("cluster_stats failed");
    assert!(stats.get("nodes").is_some());

    // cluster_settings
    let settings = client.cluster_settings().await.expect("cluster_settings failed");
    // Should have persistent and transient keys
    assert!(settings.is_object());
}

// ---------------------------------------------------------------------------
// 2. Nodes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nodes() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();

    // nodes_info
    let info = client.nodes_info().await.expect("nodes_info failed");
    assert!(info.get("nodes").is_some());

    // nodes_stats
    let stats = client.nodes_stats().await.expect("nodes_stats failed");
    assert!(stats.get("nodes").is_some());

    // nodes_hot_threads
    let threads = client.nodes_hot_threads().await.expect("nodes_hot_threads failed");
    // Returns plain text; just verify it is non-empty or at least doesn't error
    // Just verify it doesn't fail; content is plain text
    let _ = threads;
}

// ---------------------------------------------------------------------------
// 3. Index lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_lifecycle() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let idx = unique_name("test-idx");

    // Ensure clean state
    cleanup_index(&idx).await;

    // index_exists — false before creation
    let exists = client.index_exists(&idx).await.expect("index_exists failed");
    assert!(!exists, "index should not exist before creation");

    // create_index
    let body = serde_json::json!({
        "settings": {
            "number_of_shards": 1,
            "number_of_replicas": 0
        }
    });
    let ack = client.create_index(&idx, &body).await.expect("create_index failed");
    assert!(ack.acknowledged);

    // index_exists — true after creation
    let exists = client.index_exists(&idx).await.expect("index_exists failed");
    assert!(exists, "index should exist after creation");

    // get_index
    let meta = client.get_index(&idx).await.expect("get_index failed");
    assert!(meta.get(&idx).is_some());

    // get_settings
    let settings = client.get_settings(&idx).await.expect("get_settings failed");
    assert!(settings.get(&idx).is_some());

    // update_settings
    let new_settings = serde_json::json!({
        "index": { "number_of_replicas": 0 }
    });
    let ack = client
        .update_settings(&idx, &new_settings)
        .await
        .expect("update_settings failed");
    assert!(ack.acknowledged);

    // get_mapping
    let mapping = client.get_mapping(&idx).await.expect("get_mapping failed");
    assert!(mapping.get(&idx).is_some());

    // put_mapping
    let new_mapping = serde_json::json!({
        "properties": {
            "title": { "type": "text" },
            "count": { "type": "integer" }
        }
    });
    let ack = client
        .put_mapping(&idx, &new_mapping)
        .await
        .expect("put_mapping failed");
    assert!(ack.acknowledged);

    // Verify mapping was applied
    let mapping = client.get_mapping(&idx).await.expect("get_mapping after put failed");
    let props = &mapping[&idx]["mappings"]["properties"];
    assert_eq!(props["title"]["type"], "text");

    // close_index
    let ack = client.close_index(&idx).await.expect("close_index failed");
    assert!(ack.acknowledged);

    // open_index
    let ack = client.open_index(&idx).await.expect("open_index failed");
    assert!(ack.acknowledged);

    // delete_index
    let ack = client.delete_index(&idx).await.expect("delete_index failed");
    assert!(ack.acknowledged);

    // Confirm deleted
    let exists = client.index_exists(&idx).await.expect("index_exists after delete failed");
    assert!(!exists);
}

// ---------------------------------------------------------------------------
// 4. Document CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn document_crud() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let idx = unique_name("test-doc");
    cleanup_index(&idx).await;

    // Create index first
    let body = serde_json::json!({
        "settings": { "number_of_shards": 1, "number_of_replicas": 0 }
    });
    client.create_index(&idx, &body).await.expect("create_index failed");

    // index_doc — explicit ID
    let doc = serde_json::json!({ "title": "Hello", "count": 1 });
    let resp = client.index_doc(&idx, "doc1", &doc).await.expect("index_doc failed");
    assert_eq!(resp.index, idx);
    assert_eq!(resp.id, "doc1");
    assert!(resp.result.as_deref() == Some("created") || resp.result.as_deref() == Some("updated"));

    // index_doc_auto_id
    let doc2 = serde_json::json!({ "title": "Auto", "count": 2 });
    let resp = client
        .index_doc_auto_id(&idx, &doc2)
        .await
        .expect("index_doc_auto_id failed");
    assert_eq!(resp.index, idx);
    assert!(!resp.id.is_empty());
    let _auto_id = resp.id.clone();

    // get_doc
    let got = client.get_doc(&idx, "doc1").await.expect("get_doc failed");
    assert!(got.found);
    assert_eq!(got.id, "doc1");
    assert_eq!(got.source.as_ref().unwrap()["title"], "Hello");

    // head_doc — true case
    let exists = client.head_doc(&idx, "doc1").await.expect("head_doc failed");
    assert!(exists, "head_doc should return true for existing doc");

    // head_doc — false case
    let exists = client
        .head_doc(&idx, "nonexistent-doc-999")
        .await
        .expect("head_doc failed");
    assert!(!exists, "head_doc should return false for missing doc");

    // update_doc
    let update_body = serde_json::json!({
        "doc": { "count": 42 }
    });
    let uresp = client
        .update_doc(&idx, "doc1", &update_body)
        .await
        .expect("update_doc failed");
    assert_eq!(uresp.id, "doc1");
    assert!(uresp.result.as_deref() == Some("updated") || uresp.result.as_deref() == Some("noop"));

    // Verify update
    let got = client.get_doc(&idx, "doc1").await.expect("get_doc after update failed");
    assert_eq!(got.source.as_ref().unwrap()["count"], 42);

    // delete_doc
    let dresp = client.delete_doc(&idx, "doc1").await.expect("delete_doc failed");
    assert_eq!(dresp.result.as_deref(), Some("deleted"));

    // Verify doc deleted via head_doc
    let exists = client.head_doc(&idx, "doc1").await.expect("head_doc after delete failed");
    assert!(!exists);

    // Cleanup: also delete the auto-id doc's index
    cleanup_index(&idx).await;
}

// ---------------------------------------------------------------------------
// 5. Search operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_operations() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let idx = unique_name("test-search");
    cleanup_index(&idx).await;

    // Setup: create index and index some docs
    let body = serde_json::json!({
        "settings": { "number_of_shards": 1, "number_of_replicas": 0 }
    });
    client.create_index(&idx, &body).await.unwrap();

    for i in 1..=5 {
        let doc = serde_json::json!({ "title": format!("doc-{i}"), "value": i });
        client
            .index_doc(&idx, &format!("d{i}"), &doc)
            .await
            .unwrap();
    }
    refresh_index(&idx).await;

    // search
    let query = serde_json::json!({
        "query": { "match_all": {} }
    });
    let sr = client.search(&idx, &query).await.expect("search failed");
    assert!(!sr.timed_out);
    assert_eq!(sr.hits.total.value, 5);
    assert_eq!(sr.hits.hits.len(), 5);

    // search with a match query
    let query = serde_json::json!({
        "query": { "match": { "title": "doc-3" } }
    });
    let sr = client.search(&idx, &query).await.expect("search match failed");
    assert!(sr.hits.total.value >= 1);
    assert!(sr.hits.hits.iter().any(|h| h.id == "d3"));

    // search_all
    let query = serde_json::json!({
        "query": { "match_all": {} },
        "size": 1
    });
    let sr = client.search_all(&query).await.expect("search_all failed");
    assert!(sr.hits.total.value >= 5);

    // count
    let query = serde_json::json!({
        "query": { "match_all": {} }
    });
    let cr = client.count(&idx, &query).await.expect("count failed");
    assert_eq!(cr.count, 5);

    // multi_search — note: msearch body is NDJSON, but client sends as JSON.
    // The client method takes &Value, so we pass the structured form.
    // OpenSearch may not parse this correctly since msearch expects NDJSON.
    // We test what the API returns.
    // TODO: multi_search may need a raw NDJSON body method to work correctly.

    // search_shards
    let shards = client.search_shards(&idx).await.expect("search_shards failed");
    assert!(shards.nodes.is_object());
    assert!(!shards.shards.is_empty());

    // search_template — inline template
    let tmpl_body = serde_json::json!({
        "source": { "query": { "match": { "title": "{{title}}" } } },
        "params": { "title": "doc-1" }
    });
    let sr = client
        .search_template(&tmpl_body)
        .await
        .expect("search_template failed");
    // search_template against _search/template (no index) searches all indices
    assert!(sr.hits.total.value >= 1);

    // scroll — first do a search with scroll param via raw reqwest, then use scroll()
    let scroll_resp: serde_json::Value = reqwest::Client::new()
        .post(format!("{OS_URL}/{idx}/_search?scroll=1m"))
        .json(&serde_json::json!({
            "size": 2,
            "query": { "match_all": {} }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scroll_id = scroll_resp["_scroll_id"].as_str().expect("no scroll_id");

    let sr = client
        .scroll(&serde_json::json!({
            "scroll": "1m",
            "scroll_id": scroll_id
        }))
        .await
        .expect("scroll failed");
    // Should return remaining docs (we fetched 2 of 5, so 3 remain)
    assert!(sr.hits.hits.len() <= 5);

    // clear_scroll
    let clear_scroll_id = sr.scroll_id.as_deref().unwrap_or(scroll_id);
    client
        .clear_scroll(&serde_json::json!({
            "scroll_id": [clear_scroll_id]
        }))
        .await
        .expect("clear_scroll failed");

    cleanup_index(&idx).await;
}

// ---------------------------------------------------------------------------
// 6. Bulk operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_operations() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let idx = unique_name("test-bulk");
    let idx2 = unique_name("test-bulk-dst");
    cleanup_index(&idx).await;
    cleanup_index(&idx2).await;

    // Create source index
    let body = serde_json::json!({
        "settings": { "number_of_shards": 1, "number_of_replicas": 0 }
    });
    client.create_index(&idx, &body).await.unwrap();

    // bulk — The client's bulk() method sends body as JSON (Content-Type: application/json).
    // OpenSearch _bulk expects NDJSON (newline-delimited JSON).
    // Sending a single JSON object will likely fail or be misinterpreted.
    // TODO: bulk() needs a raw NDJSON body method. The current &Value signature
    // cannot represent NDJSON. Consider adding a bulk_raw(&str) method.

    // Instead, index docs individually for multi_get and reindex tests.
    for i in 1..=3 {
        let doc = serde_json::json!({ "field": format!("value{i}") });
        client.index_doc(&idx, &i.to_string(), &doc).await.unwrap();
    }
    refresh_index(&idx).await;

    // multi_get
    let mget_body = serde_json::json!({
        "docs": [
            { "_index": &idx, "_id": "1" },
            { "_index": &idx, "_id": "2" },
            { "_index": &idx, "_id": "999" }
        ]
    });
    let mget = client.multi_get(&mget_body).await.expect("multi_get failed");
    assert_eq!(mget.docs.len(), 3);
    assert!(mget.docs[0].found);
    assert!(mget.docs[1].found);
    assert!(!mget.docs[2].found);

    // reindex
    let reindex_body = serde_json::json!({
        "source": { "index": &idx },
        "dest": { "index": &idx2 }
    });
    let rr = client.reindex(&reindex_body).await.expect("reindex failed");
    assert_eq!(rr.created, 3);
    assert!(rr.failures.is_empty());

    // delete_by_query
    refresh_index(&idx).await;
    let dbq_body = serde_json::json!({
        "query": { "match": { "field": "value1" } }
    });
    let dbq = client
        .delete_by_query(&idx, &dbq_body)
        .await
        .expect("delete_by_query failed");
    assert_eq!(dbq.deleted, 1);

    cleanup_index(&idx).await;
    cleanup_index(&idx2).await;
}

// ---------------------------------------------------------------------------
// 7. Aliases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aliases() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let idx = unique_name("test-alias");
    let alias_name = unique_name("alias");
    cleanup_index(&idx).await;

    // Create index
    let body = serde_json::json!({
        "settings": { "number_of_shards": 1, "number_of_replicas": 0 }
    });
    client.create_index(&idx, &body).await.unwrap();

    // create_alias
    let alias_body = serde_json::json!({
        "actions": [
            { "add": { "index": &idx, "alias": &alias_name } }
        ]
    });
    let ack = client.create_alias(&alias_body).await.expect("create_alias failed");
    assert!(ack.acknowledged);

    // get_aliases
    let aliases = client.get_aliases(&idx).await.expect("get_aliases failed");
    let idx_aliases = &aliases[&idx]["aliases"];
    assert!(idx_aliases.get(&alias_name).is_some(), "alias should exist");

    // delete_alias
    let ack = client
        .delete_alias(&idx, &alias_name)
        .await
        .expect("delete_alias failed");
    assert!(ack.acknowledged);

    // Verify alias removed
    let aliases = client.get_aliases(&idx).await.expect("get_aliases after delete failed");
    let idx_aliases = &aliases[&idx]["aliases"];
    assert!(idx_aliases.get(&alias_name).is_none(), "alias should be gone");

    cleanup_index(&idx).await;
}

// ---------------------------------------------------------------------------
// 8. Templates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn templates() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let tmpl_name = unique_name("test-tmpl");
    cleanup_template(&tmpl_name).await;

    // create_template
    let tmpl_body = serde_json::json!({
        "index_patterns": [format!("{tmpl_name}-*")],
        "template": {
            "settings": {
                "number_of_shards": 1,
                "number_of_replicas": 0
            },
            "mappings": {
                "properties": {
                    "name": { "type": "keyword" }
                }
            }
        }
    });
    let ack = client
        .create_template(&tmpl_name, &tmpl_body)
        .await
        .expect("create_template failed");
    assert!(ack.acknowledged);

    // get_template
    let tmpl = client.get_template(&tmpl_name).await.expect("get_template failed");
    let templates = tmpl["index_templates"].as_array().expect("expected array");
    assert!(!templates.is_empty());
    assert_eq!(templates[0]["name"], tmpl_name);

    // delete_template
    let ack = client
        .delete_template(&tmpl_name)
        .await
        .expect("delete_template failed");
    assert!(ack.acknowledged);

    // Verify deleted — get_template should error
    let result = client.get_template(&tmpl_name).await;
    assert!(result.is_err(), "get_template should fail after deletion");
}

// ---------------------------------------------------------------------------
// 9. Cat operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cat_operations() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let idx = unique_name("test-cat");
    cleanup_index(&idx).await;

    // Create an index so cat_indices returns at least one
    let body = serde_json::json!({
        "settings": { "number_of_shards": 1, "number_of_replicas": 0 }
    });
    client.create_index(&idx, &body).await.unwrap();

    // cat_indices
    let indices = client.cat_indices().await.expect("cat_indices failed");
    assert!(
        indices.iter().any(|i| i.index.as_deref() == Some(&idx)),
        "our index should appear in cat_indices"
    );

    // cat_nodes
    let nodes = client.cat_nodes().await.expect("cat_nodes failed");
    assert!(!nodes.is_empty(), "should have at least one node");
    assert!(nodes[0].ip.is_some());

    // cat_shards
    let shards = client.cat_shards().await.expect("cat_shards failed");
    assert!(
        shards.iter().any(|s| s.index.as_deref() == Some(&idx)),
        "our index should have shards"
    );

    // cat_health
    let health = client.cat_health().await.expect("cat_health failed");
    assert!(!health.is_empty());
    assert!(health[0].status.is_some());

    // cat_allocation
    let alloc = client.cat_allocation().await.expect("cat_allocation failed");
    assert!(!alloc.is_empty());
    assert!(alloc[0].node.is_some());

    cleanup_index(&idx).await;
}

// ---------------------------------------------------------------------------
// 10. Ingest pipelines
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_pipelines() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let pipe_id = unique_name("test-pipe");
    cleanup_pipeline(&pipe_id).await;

    // create_pipeline
    let pipe_body = serde_json::json!({
        "description": "Test pipeline",
        "processors": [
            {
                "set": {
                    "field": "processed",
                    "value": true
                }
            }
        ]
    });
    let ack = client
        .create_pipeline(&pipe_id, &pipe_body)
        .await
        .expect("create_pipeline failed");
    assert!(ack.acknowledged);

    // get_pipeline
    let pipe = client.get_pipeline(&pipe_id).await.expect("get_pipeline failed");
    assert!(pipe.get(&pipe_id).is_some());
    assert_eq!(pipe[&pipe_id]["description"], "Test pipeline");

    // get_all_pipelines
    let all = client.get_all_pipelines().await.expect("get_all_pipelines failed");
    assert!(all.get(&pipe_id).is_some(), "our pipeline should appear in all pipelines");

    // simulate_pipeline
    let sim_body = serde_json::json!({
        "docs": [
            { "_source": { "title": "test doc" } }
        ]
    });
    let sim = client
        .simulate_pipeline(&pipe_id, &sim_body)
        .await
        .expect("simulate_pipeline failed");
    let sim_docs = sim["docs"].as_array().expect("expected docs array");
    assert!(!sim_docs.is_empty());
    // The set processor should have added "processed": true
    assert_eq!(sim_docs[0]["doc"]["_source"]["processed"], true);

    // delete_pipeline
    let ack = client
        .delete_pipeline(&pipe_id)
        .await
        .expect("delete_pipeline failed");
    assert!(ack.acknowledged);

    // Verify deleted
    let result = client.get_pipeline(&pipe_id).await;
    assert!(result.is_err(), "get_pipeline should fail after deletion");
}

// ---------------------------------------------------------------------------
// 11. Snapshots
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snapshots() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();
    let repo_name = unique_name("test-repo");
    cleanup_snapshot_repo(&repo_name).await;

    // create_snapshot_repo — use fs type with a known path
    // Note: this requires the OpenSearch node to have path.repo configured.
    // We use a URL repo type which is more universally available, but may
    // also need config. Try fs repo first; if it fails, the test still
    // validates the API call structure.
    let repo_body = serde_json::json!({
        "type": "fs",
        "settings": {
            "location": format!("/tmp/snapshots/{repo_name}")
        }
    });
    let result = client.create_snapshot_repo(&repo_name, &repo_body).await;
    if let Err(ref e) = result {
        // If fs repo is not configured (path.repo not set), skip gracefully
        let msg = format!("{e}");
        if msg.contains("repository_exception") || msg.contains("doesn't match any of the locations") {
            eprintln!("Skipping snapshot tests: path.repo not configured on OpenSearch node");
            return;
        }
    }
    let ack = result.expect("create_snapshot_repo failed");
    assert!(ack.acknowledged);

    // list_snapshots — repo exists but no snapshots yet
    let snaps = client
        .list_snapshots(&repo_name)
        .await
        .expect("list_snapshots failed");
    let snap_list = snaps["snapshots"].as_array().expect("expected snapshots array");
    assert!(snap_list.is_empty(), "fresh repo should have no snapshots");

    // delete_snapshot_repo
    let ack = client
        .delete_snapshot_repo(&repo_name)
        .await
        .expect("delete_snapshot_repo failed");
    assert!(ack.acknowledged);

    // Skipping create_snapshot / restore_snapshot — they require filesystem
    // access and a properly configured path.repo on the OpenSearch node.
}

// ---------------------------------------------------------------------------
// 12. Cluster settings (update_cluster_settings, reroute, allocation_explain)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_settings_update() {
    wait_for_healthy(HEALTH_URL, TIMEOUT).await;
    let client = os_client();

    // update_cluster_settings — set a harmless transient setting
    let body = serde_json::json!({
        "transient": {
            "cluster.routing.allocation.enable": "all"
        }
    });
    let resp = client
        .update_cluster_settings(&body)
        .await
        .expect("update_cluster_settings failed");
    assert!(resp.get("acknowledged").is_some());
    assert_eq!(resp["acknowledged"], true);

    // reroute — a no-op reroute with empty commands
    let body = serde_json::json!({ "commands": [] });
    let resp = client.reroute(&body).await.expect("reroute failed");
    assert!(resp.get("acknowledged").is_some());

    // allocation_explain — requires an unassigned shard to explain.
    // On a healthy single-node cluster there may be none, so we accept
    // either a successful response or an error indicating no unassigned shards.
    let body = serde_json::json!({});
    let result = client.allocation_explain(&body).await;
    match result {
        Ok(val) => {
            // If it succeeds, it should contain shard allocation info
            assert!(val.is_object());
        }
        Err(e) => {
            // Expected: "unable to find any unassigned shards to explain"
            let msg = format!("{e}");
            assert!(
                msg.contains("unable to find") || msg.contains("400"),
                "unexpected allocation_explain error: {msg}"
            );
        }
    }
}
