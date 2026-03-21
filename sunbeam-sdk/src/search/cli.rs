//! CLI commands for OpenSearch.

use clap::Subcommand;
use serde_json::json;

use crate::error::Result;
use crate::output::{self, OutputFormat};

// ---------------------------------------------------------------------------
// Client helper
// ---------------------------------------------------------------------------

async fn os_client(domain: &str) -> Result<super::OpenSearchClient> {
    let token = crate::auth::get_token().await?;
    let mut c = super::OpenSearchClient::connect(domain);
    c.set_token(token);
    Ok(c)
}

// ---------------------------------------------------------------------------
// Top-level command enum
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SearchCommand {
    /// Document operations.
    Doc {
        #[command(subcommand)]
        action: DocAction,
    },

    /// Search an index.
    Query {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Query string (wrapped in query_string query).
        #[arg(short, long)]
        query: Option<String>,
        /// Raw JSON body (overrides --query).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Count documents in an index.
    Count {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Raw JSON body for the count query.
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Index management.
    Index {
        #[command(subcommand)]
        action: IndexAction,
    },

    /// Cluster operations.
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },

    /// Node operations.
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },

    /// Ingest pipeline management.
    Ingest {
        #[command(subcommand)]
        action: IngestAction,
    },

    /// Snapshot management.
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum DocAction {
    /// Get a document by ID.
    Get {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Document ID.
        #[arg(short, long)]
        id: String,
    },

    /// Index (create/replace) a document.
    Create {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Document ID.
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Update a document by ID.
    Update {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Document ID.
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Delete a document by ID.
    Delete {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Document ID.
        #[arg(short, long)]
        id: String,
    },

    /// Check if a document exists.
    Exists {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// Document ID.
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum IndexAction {
    /// List all indices.
    List,

    /// Create an index.
    Create {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// JSON body (settings/mappings, or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Get index metadata.
    Get {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Delete an index.
    Delete {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Check if an index exists.
    Exists {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Open a closed index.
    Open {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Close an index.
    Close {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Index mapping operations.
    Mapping {
        #[command(subcommand)]
        action: MappingAction,
    },

    /// Index settings operations.
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum MappingAction {
    /// Get index mapping.
    Get {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Update index mapping.
    Update {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SettingsAction {
    /// Get index settings.
    Get {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
    },

    /// Update index settings.
    Update {
        /// Index name.
        #[arg(short = 'x', long)]
        index: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Cluster
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ClusterAction {
    /// Cluster health.
    Health,

    /// Cluster state.
    State,

    /// Cluster stats.
    Stats,
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// List nodes.
    List,

    /// Node stats.
    Stats,

    /// Node info.
    Info,
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum IngestAction {
    /// Ingest pipeline management.
    Pipeline {
        #[command(subcommand)]
        action: PipelineAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum PipelineAction {
    /// List all pipelines.
    List,

    /// Create or update a pipeline.
    Create {
        /// Pipeline ID.
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Get a pipeline.
    Get {
        /// Pipeline ID.
        #[arg(short, long)]
        id: String,
    },

    /// Delete a pipeline.
    Delete {
        /// Pipeline ID.
        #[arg(short, long)]
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum SnapshotAction {
    /// List snapshots in a repository.
    List {
        /// Repository name.
        #[arg(long)]
        repo: String,
    },

    /// Create a snapshot.
    Create {
        /// Repository name.
        #[arg(long)]
        repo: String,
        /// Snapshot name.
        #[arg(long)]
        name: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Delete a snapshot.
    Delete {
        /// Repository name.
        #[arg(long)]
        repo: String,
        /// Snapshot name.
        #[arg(long)]
        name: String,
    },

    /// Restore a snapshot.
    Restore {
        /// Repository name.
        #[arg(long)]
        repo: String,
        /// Snapshot name.
        #[arg(long)]
        name: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Snapshot repository management.
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum RepoAction {
    /// Create or update a snapshot repository.
    Create {
        /// Repository name.
        #[arg(long)]
        name: String,
        /// JSON body (or "-" for stdin).
        #[arg(short, long)]
        data: Option<String>,
    },

    /// Delete a snapshot repository.
    Delete {
        /// Repository name.
        #[arg(long)]
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch(
    cmd: SearchCommand,
    client: &crate::client::SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let c = os_client(client.domain()).await?;

    match cmd {
        // -----------------------------------------------------------------
        // Documents
        // -----------------------------------------------------------------
        SearchCommand::Doc { action } => match action {
            DocAction::Get { index, id } => {
                let resp = c.get_doc(&index, &id).await?;
                output::render(&resp, fmt)
            }

            DocAction::Create { index, id, data } => {
                let body = output::read_json_input(data.as_deref())?;
                let resp = c.index_doc(&index, &id, &body).await?;
                output::render(&resp, fmt)
            }

            DocAction::Update { index, id, data } => {
                let body = output::read_json_input(data.as_deref())?;
                let resp = c.update_doc(&index, &id, &body).await?;
                output::render(&resp, fmt)
            }

            DocAction::Delete { index, id } => {
                let resp = c.delete_doc(&index, &id).await?;
                output::render(&resp, fmt)
            }

            DocAction::Exists { index, id } => {
                let exists = c.head_doc(&index, &id).await?;
                println!("{exists}");
                Ok(())
            }
        },

        // -----------------------------------------------------------------
        // Search query
        // -----------------------------------------------------------------
        SearchCommand::Query { index, query, data } => {
            let body = if let Some(d) = data.as_deref() {
                output::read_json_input(Some(d))?
            } else if let Some(ref q) = query {
                json!({ "query": { "query_string": { "query": q } } })
            } else {
                json!({ "query": { "match_all": {} } })
            };
            let resp = c.search(&index, &body).await?;
            output::render(&resp, fmt)
        }

        // -----------------------------------------------------------------
        // Count
        // -----------------------------------------------------------------
        SearchCommand::Count { index, data } => {
            let body = if let Some(d) = data.as_deref() {
                output::read_json_input(Some(d))?
            } else {
                json!({ "query": { "match_all": {} } })
            };
            let resp = c.count(&index, &body).await?;
            output::render(&resp, fmt)
        }

        // -----------------------------------------------------------------
        // Index management
        // -----------------------------------------------------------------
        SearchCommand::Index { action } => dispatch_index(&c, action, fmt).await,

        // -----------------------------------------------------------------
        // Cluster
        // -----------------------------------------------------------------
        SearchCommand::Cluster { action } => match action {
            ClusterAction::Health => {
                let resp = c.cluster_health().await?;
                output::render(&resp, fmt)
            }

            ClusterAction::State => {
                let resp = c.cluster_state().await?;
                output::render(&resp, fmt)
            }

            ClusterAction::Stats => {
                let resp = c.cluster_stats().await?;
                output::render(&resp, fmt)
            }
        },

        // -----------------------------------------------------------------
        // Nodes
        // -----------------------------------------------------------------
        SearchCommand::Node { action } => match action {
            NodeAction::List => {
                let rows = c.cat_nodes().await?;
                output::render_list(
                    &rows,
                    &["NAME", "IP", "HEAP%", "RAM%", "CPU", "ROLE", "MASTER"],
                    |n| {
                        vec![
                            n.name.clone().unwrap_or_default(),
                            n.ip.clone().unwrap_or_default(),
                            n.heap_percent.clone().unwrap_or_default(),
                            n.ram_percent.clone().unwrap_or_default(),
                            n.cpu.clone().unwrap_or_default(),
                            n.node_role.clone().unwrap_or_default(),
                            n.master.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }

            NodeAction::Stats => {
                let resp = c.nodes_stats().await?;
                output::render(&resp, fmt)
            }

            NodeAction::Info => {
                let resp = c.nodes_info().await?;
                output::render(&resp, fmt)
            }
        },

        // -----------------------------------------------------------------
        // Ingest
        // -----------------------------------------------------------------
        SearchCommand::Ingest { action } => match action {
            IngestAction::Pipeline { action } => match action {
                PipelineAction::List => {
                    let resp = c.get_all_pipelines().await?;
                    output::render(&resp, fmt)
                }

                PipelineAction::Create { id, data } => {
                    let body = output::read_json_input(data.as_deref())?;
                    let resp = c.create_pipeline(&id, &body).await?;
                    output::render(&resp, fmt)
                }

                PipelineAction::Get { id } => {
                    let resp = c.get_pipeline(&id).await?;
                    output::render(&resp, fmt)
                }

                PipelineAction::Delete { id } => {
                    let resp = c.delete_pipeline(&id).await?;
                    output::render(&resp, fmt)
                }
            },
        },

        // -----------------------------------------------------------------
        // Snapshots
        // -----------------------------------------------------------------
        SearchCommand::Snapshot { action } => dispatch_snapshot(&c, action, fmt).await,
    }
}

// ---------------------------------------------------------------------------
// Index sub-dispatch
// ---------------------------------------------------------------------------

async fn dispatch_index(
    c: &super::OpenSearchClient,
    action: IndexAction,
    fmt: OutputFormat,
) -> Result<()> {
    match action {
        IndexAction::List => {
            let rows = c.cat_indices().await?;
            output::render_list(
                &rows,
                &["HEALTH", "STATUS", "INDEX", "PRI", "REP", "DOCS", "SIZE"],
                |i| {
                    vec![
                        i.health.clone().unwrap_or_default(),
                        i.status.clone().unwrap_or_default(),
                        i.index.clone().unwrap_or_default(),
                        i.pri.clone().unwrap_or_default(),
                        i.rep.clone().unwrap_or_default(),
                        i.docs_count.clone().unwrap_or_default(),
                        i.store_size.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }

        IndexAction::Create { index, data } => {
            let body = data
                .as_deref()
                .map(|d| output::read_json_input(Some(d)))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let resp = c.create_index(&index, &body).await?;
            output::render(&resp, fmt)
        }

        IndexAction::Get { index } => {
            let resp = c.get_index(&index).await?;
            output::render(&resp, fmt)
        }

        IndexAction::Delete { index } => {
            let resp = c.delete_index(&index).await?;
            output::render(&resp, fmt)
        }

        IndexAction::Exists { index } => {
            let exists = c.index_exists(&index).await?;
            println!("{exists}");
            Ok(())
        }

        IndexAction::Open { index } => {
            let resp = c.open_index(&index).await?;
            output::render(&resp, fmt)
        }

        IndexAction::Close { index } => {
            let resp = c.close_index(&index).await?;
            output::render(&resp, fmt)
        }

        IndexAction::Mapping { action } => match action {
            MappingAction::Get { index } => {
                let resp = c.get_mapping(&index).await?;
                output::render(&resp, fmt)
            }

            MappingAction::Update { index, data } => {
                let body = output::read_json_input(data.as_deref())?;
                let resp = c.put_mapping(&index, &body).await?;
                output::render(&resp, fmt)
            }
        },

        IndexAction::Settings { action } => match action {
            SettingsAction::Get { index } => {
                let resp = c.get_settings(&index).await?;
                output::render(&resp, fmt)
            }

            SettingsAction::Update { index, data } => {
                let body = output::read_json_input(data.as_deref())?;
                let resp = c.update_settings(&index, &body).await?;
                output::render(&resp, fmt)
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Snapshot sub-dispatch
// ---------------------------------------------------------------------------

async fn dispatch_snapshot(
    c: &super::OpenSearchClient,
    action: SnapshotAction,
    fmt: OutputFormat,
) -> Result<()> {
    match action {
        SnapshotAction::List { repo } => {
            let resp = c.list_snapshots(&repo).await?;
            output::render(&resp, fmt)
        }

        SnapshotAction::Create { repo, name, data } => {
            let body = data
                .as_deref()
                .map(|d| output::read_json_input(Some(d)))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let resp = c.create_snapshot(&repo, &name, &body).await?;
            output::render(&resp, fmt)
        }

        SnapshotAction::Delete { repo, name } => {
            let resp = c.delete_snapshot(&repo, &name).await?;
            output::render(&resp, fmt)
        }

        SnapshotAction::Restore { repo, name, data } => {
            let body = data
                .as_deref()
                .map(|d| output::read_json_input(Some(d)))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let resp = c.restore_snapshot(&repo, &name, &body).await?;
            output::render(&resp, fmt)
        }

        SnapshotAction::Repo { action } => match action {
            RepoAction::Create { name, data } => {
                let body = output::read_json_input(data.as_deref())?;
                let resp = c.create_snapshot_repo(&name, &body).await?;
                output::render(&resp, fmt)
            }

            RepoAction::Delete { name } => {
                let resp = c.delete_snapshot_repo(&name).await?;
                output::render(&resp, fmt)
            }
        },
    }
}
