//! CLI commands for monitoring services: Prometheus, Loki, and Grafana.

use clap::Subcommand;

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::output::{self, OutputFormat};

// ===========================================================================
// Top-level MonCommand
// ===========================================================================

#[derive(Subcommand, Debug)]
pub enum MonCommand {
    /// Prometheus queries, targets, rules and status.
    Prometheus {
        #[command(subcommand)]
        action: PrometheusAction,
    },
    /// Loki log queries, labels, and ingestion.
    Loki {
        #[command(subcommand)]
        action: LokiAction,
    },
    /// Grafana dashboards, datasources, folders, annotations, alerts, and org.
    Grafana {
        #[command(subcommand)]
        action: GrafanaAction,
    },
}

// ===========================================================================
// Prometheus
// ===========================================================================

#[derive(Subcommand, Debug)]
pub enum PrometheusAction {
    /// Execute an instant PromQL query.
    Query {
        /// PromQL expression.
        #[arg(short, long, alias = "expr", short_alias = 'e')]
        query: String,
        /// Evaluation timestamp (RFC-3339 or Unix).
        #[arg(long)]
        time: Option<String>,
    },
    /// Execute a range PromQL query.
    QueryRange {
        /// PromQL expression.
        #[arg(short, long, alias = "expr", short_alias = 'e')]
        query: String,
        /// Start timestamp.
        #[arg(long)]
        start: String,
        /// End timestamp.
        #[arg(long)]
        end: String,
        /// Query resolution step (e.g. "15s").
        #[arg(long)]
        step: String,
    },
    /// Find series by label matchers.
    Series {
        /// One or more series selectors (e.g. `up{job="node"}`).
        #[arg(short = 'm', long = "match", required = true)]
        match_params: Vec<String>,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// List all label names.
    Labels {
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// List values for a specific label.
    LabelValues {
        /// Label name.
        #[arg(short, long)]
        label: String,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// Show current target discovery status.
    Targets,
    /// List alerting and recording rules.
    Rules,
    /// List active alerts.
    Alerts,
    /// Show Prometheus runtime information.
    Status,
    /// Show per-metric metadata.
    Metadata {
        /// Filter by metric name.
        #[arg(short, long)]
        metric: Option<String>,
    },
}

// ===========================================================================
// Loki
// ===========================================================================

#[derive(Subcommand, Debug)]
pub enum LokiAction {
    /// Execute an instant LogQL query.
    Query {
        /// LogQL expression.
        #[arg(short, long, alias = "expr", short_alias = 'e')]
        query: String,
        /// Maximum number of entries to return.
        #[arg(short, long)]
        limit: Option<u32>,
        /// Evaluation timestamp.
        #[arg(long)]
        time: Option<String>,
    },
    /// Execute a range LogQL query.
    QueryRange {
        /// LogQL expression.
        #[arg(short, long, alias = "expr", short_alias = 'e')]
        query: String,
        /// Start timestamp.
        #[arg(long)]
        start: String,
        /// End timestamp.
        #[arg(long)]
        end: String,
        /// Maximum number of entries to return.
        #[arg(short, long)]
        limit: Option<u32>,
        /// Query resolution step.
        #[arg(long)]
        step: Option<String>,
    },
    /// List all label names.
    Labels {
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// List values for a specific label.
    LabelValues {
        /// Label name.
        #[arg(short, long)]
        label: String,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// Find series by label matchers.
    Series {
        /// One or more series selectors.
        #[arg(short = 'm', long = "match", required = true)]
        match_params: Vec<String>,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// Push log entries from JSON.
    Push {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Show index statistics.
    IndexStats,
    /// Detect log patterns.
    Patterns {
        /// LogQL expression.
        #[arg(short, long, alias = "expr", short_alias = 'e')]
        query: String,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// Check Loki readiness.
    Ready,
}

// ===========================================================================
// Grafana
// ===========================================================================

#[derive(Subcommand, Debug)]
pub enum GrafanaAction {
    /// Dashboard management.
    Dashboard {
        #[command(subcommand)]
        action: GrafanaDashboardAction,
    },
    /// Datasource management.
    Datasource {
        #[command(subcommand)]
        action: GrafanaDatasourceAction,
    },
    /// Folder management.
    Folder {
        #[command(subcommand)]
        action: GrafanaFolderAction,
    },
    /// Annotation management.
    Annotation {
        #[command(subcommand)]
        action: GrafanaAnnotationAction,
    },
    /// Alert rule management.
    Alert {
        #[command(subcommand)]
        action: GrafanaAlertAction,
    },
    /// Organization settings.
    Org {
        #[command(subcommand)]
        action: GrafanaOrgAction,
    },
}

// ---------------------------------------------------------------------------
// Grafana Dashboard
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GrafanaDashboardAction {
    /// List all dashboards.
    List,
    /// Get a dashboard by UID.
    Get {
        #[arg(long)]
        uid: String,
    },
    /// Create a dashboard from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a dashboard from JSON.
    Update {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a dashboard by UID.
    Delete {
        #[arg(long)]
        uid: String,
    },
    /// Search dashboards by query string.
    Search {
        #[arg(short, long)]
        query: String,
    },
}

// ---------------------------------------------------------------------------
// Grafana Datasource
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GrafanaDatasourceAction {
    /// List all datasources.
    List,
    /// Get a datasource by numeric ID.
    Get {
        #[arg(long)]
        id: u64,
    },
    /// Create a datasource from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a datasource from JSON.
    Update {
        #[arg(long)]
        id: u64,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a datasource by numeric ID.
    Delete {
        #[arg(long)]
        id: u64,
    },
}

// ---------------------------------------------------------------------------
// Grafana Folder
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GrafanaFolderAction {
    /// List all folders.
    List,
    /// Get a folder by UID.
    Get {
        #[arg(long)]
        uid: String,
    },
    /// Create a folder from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a folder from JSON.
    Update {
        #[arg(long)]
        uid: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a folder by UID.
    Delete {
        #[arg(long)]
        uid: String,
    },
}

// ---------------------------------------------------------------------------
// Grafana Annotation
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GrafanaAnnotationAction {
    /// List annotations (optional query-string filter).
    List {
        /// Raw query-string params (e.g. "from=1234&to=5678").
        #[arg(long)]
        params: Option<String>,
    },
    /// Create an annotation from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete an annotation by ID.
    Delete {
        #[arg(long)]
        id: u64,
    },
}

// ---------------------------------------------------------------------------
// Grafana Alert
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GrafanaAlertAction {
    /// List all provisioned alert rules.
    List,
    /// Create a provisioned alert rule from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a provisioned alert rule from JSON.
    Update {
        #[arg(long)]
        uid: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a provisioned alert rule by UID.
    Delete {
        #[arg(long)]
        uid: String,
    },
}

// ---------------------------------------------------------------------------
// Grafana Org
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum GrafanaOrgAction {
    /// Get the current organization.
    Get,
    /// Update the current organization from JSON.
    Update {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

// ===========================================================================
// Dispatch
// ===========================================================================

pub async fn dispatch(
    cmd: MonCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    match cmd {
        MonCommand::Prometheus { action } => dispatch_prometheus(action, client, fmt).await,
        MonCommand::Loki { action } => dispatch_loki(action, client, fmt).await,
        MonCommand::Grafana { action } => dispatch_grafana(action, client, fmt).await,
    }
}

// ===========================================================================
// Prometheus dispatch
// ===========================================================================

async fn dispatch_prometheus(
    action: PrometheusAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let prom = client.prometheus().await?;
    match action {
        PrometheusAction::Query { query, time } => {
            let res = prom.query(&query, time.as_deref()).await?;
            output::render(&res, fmt)
        }
        PrometheusAction::QueryRange {
            query,
            start,
            end,
            step,
        } => {
            let res = prom.query_range(&query, &start, &end, &step).await?;
            output::render(&res, fmt)
        }
        PrometheusAction::Series {
            match_params,
            start,
            end,
        } => {
            let refs: Vec<&str> = match_params.iter().map(|s| s.as_str()).collect();
            let res = prom
                .series(&refs, start.as_deref(), end.as_deref())
                .await?;
            output::render(&res, fmt)
        }
        PrometheusAction::Labels { start, end } => {
            let res = prom.labels(start.as_deref(), end.as_deref()).await?;
            if let Some(labels) = &res.data {
                output::render_list(
                    labels,
                    &["LABEL"],
                    |l| vec![l.clone()],
                    fmt,
                )
            } else {
                output::render(&res, fmt)
            }
        }
        PrometheusAction::LabelValues { label, start, end } => {
            let res = prom
                .label_values(&label, start.as_deref(), end.as_deref())
                .await?;
            if let Some(values) = &res.data {
                output::render_list(
                    values,
                    &["VALUE"],
                    |v| vec![v.clone()],
                    fmt,
                )
            } else {
                output::render(&res, fmt)
            }
        }
        PrometheusAction::Targets => {
            let res = prom.targets().await?;
            output::render(&res, fmt)
        }
        PrometheusAction::Rules => {
            let res = prom.rules().await?;
            output::render(&res, fmt)
        }
        PrometheusAction::Alerts => {
            let res = prom.alerts().await?;
            output::render(&res, fmt)
        }
        PrometheusAction::Status => {
            let res = prom.runtime_info().await?;
            output::render(&res, fmt)
        }
        PrometheusAction::Metadata { metric } => {
            let res = prom.metadata(metric.as_deref()).await?;
            output::render(&res, fmt)
        }
    }
}

// ===========================================================================
// Loki dispatch
// ===========================================================================

async fn dispatch_loki(
    action: LokiAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let loki = client.loki().await?;
    match action {
        LokiAction::Query { query, limit, time } => {
            let res = loki.query(&query, limit, time.as_deref()).await?;
            output::render(&res, fmt)
        }
        LokiAction::QueryRange {
            query,
            start,
            end,
            limit,
            step,
        } => {
            let res = loki
                .query_range(&query, &start, &end, limit, step.as_deref())
                .await?;
            output::render(&res, fmt)
        }
        LokiAction::Labels { start, end } => {
            let res = loki.labels(start.as_deref(), end.as_deref()).await?;
            if let Some(labels) = &res.data {
                output::render_list(
                    labels,
                    &["LABEL"],
                    |l| vec![l.clone()],
                    fmt,
                )
            } else {
                output::render(&res, fmt)
            }
        }
        LokiAction::LabelValues { label, start, end } => {
            let res = loki
                .label_values(&label, start.as_deref(), end.as_deref())
                .await?;
            if let Some(values) = &res.data {
                output::render_list(
                    values,
                    &["VALUE"],
                    |v| vec![v.clone()],
                    fmt,
                )
            } else {
                output::render(&res, fmt)
            }
        }
        LokiAction::Series {
            match_params,
            start,
            end,
        } => {
            let refs: Vec<&str> = match_params.iter().map(|s| s.as_str()).collect();
            let res = loki
                .series(&refs, start.as_deref(), end.as_deref())
                .await?;
            output::render(&res, fmt)
        }
        LokiAction::Push { data } => {
            let json = output::read_json_input(data.as_deref())?;
            loki.push(&json).await?;
            output::ok("Pushed log entries");
            Ok(())
        }
        LokiAction::IndexStats => {
            let res = loki.index_stats().await?;
            output::render(&res, fmt)
        }
        LokiAction::Patterns { query, start, end } => {
            let res = loki
                .detect_patterns(&query, start.as_deref(), end.as_deref())
                .await?;
            output::render(&res, fmt)
        }
        LokiAction::Ready => {
            let res = loki.ready().await?;
            output::render(&res, fmt)
        }
    }
}

// ===========================================================================
// Grafana dispatch
// ===========================================================================

async fn dispatch_grafana(
    action: GrafanaAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    match action {
        GrafanaAction::Dashboard { action } => {
            dispatch_grafana_dashboard(action, client, fmt).await
        }
        GrafanaAction::Datasource { action } => {
            dispatch_grafana_datasource(action, client, fmt).await
        }
        GrafanaAction::Folder { action } => {
            dispatch_grafana_folder(action, client, fmt).await
        }
        GrafanaAction::Annotation { action } => {
            dispatch_grafana_annotation(action, client, fmt).await
        }
        GrafanaAction::Alert { action } => {
            dispatch_grafana_alert(action, client, fmt).await
        }
        GrafanaAction::Org { action } => {
            dispatch_grafana_org(action, client, fmt).await
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana Dashboard dispatch
// ---------------------------------------------------------------------------

async fn dispatch_grafana_dashboard(
    action: GrafanaDashboardAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let grafana = client.grafana().await?;
    match action {
        GrafanaDashboardAction::List => {
            let items = grafana.list_dashboards().await?;
            output::render_list(
                &items,
                &["UID", "TITLE", "URL", "TAGS"],
                |d| {
                    vec![
                        d.uid.clone(),
                        d.title.clone(),
                        d.url.clone().unwrap_or_default(),
                        d.tags.join(", "),
                    ]
                },
                fmt,
            )
        }
        GrafanaDashboardAction::Get { uid } => {
            let item = grafana.get_dashboard(&uid).await?;
            output::render(&item, fmt)
        }
        GrafanaDashboardAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.create_dashboard(&json).await?;
            output::render(&item, fmt)
        }
        GrafanaDashboardAction::Update { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.update_dashboard(&json).await?;
            output::render(&item, fmt)
        }
        GrafanaDashboardAction::Delete { uid } => {
            grafana.delete_dashboard(&uid).await?;
            output::ok(&format!("Deleted dashboard {uid}"));
            Ok(())
        }
        GrafanaDashboardAction::Search { query } => {
            let items = grafana.search_dashboards(&query).await?;
            output::render_list(
                &items,
                &["UID", "TITLE", "URL", "TAGS"],
                |d| {
                    vec![
                        d.uid.clone(),
                        d.title.clone(),
                        d.url.clone().unwrap_or_default(),
                        d.tags.join(", "),
                    ]
                },
                fmt,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana Datasource dispatch
// ---------------------------------------------------------------------------

async fn dispatch_grafana_datasource(
    action: GrafanaDatasourceAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let grafana = client.grafana().await?;
    match action {
        GrafanaDatasourceAction::List => {
            let items = grafana.list_datasources().await?;
            output::render_list(
                &items,
                &["ID", "UID", "NAME", "TYPE", "URL"],
                |d| {
                    vec![
                        d.id.map_or("-".into(), |id| id.to_string()),
                        d.uid.clone().unwrap_or_default(),
                        d.name.clone(),
                        d.kind.clone(),
                        d.url.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        GrafanaDatasourceAction::Get { id } => {
            let item = grafana.get_datasource(id).await?;
            output::render(&item, fmt)
        }
        GrafanaDatasourceAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.create_datasource(&json).await?;
            output::render(&item, fmt)
        }
        GrafanaDatasourceAction::Update { id, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.update_datasource(id, &json).await?;
            output::render(&item, fmt)
        }
        GrafanaDatasourceAction::Delete { id } => {
            grafana.delete_datasource(id).await?;
            output::ok(&format!("Deleted datasource {id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana Folder dispatch
// ---------------------------------------------------------------------------

async fn dispatch_grafana_folder(
    action: GrafanaFolderAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let grafana = client.grafana().await?;
    match action {
        GrafanaFolderAction::List => {
            let items = grafana.list_folders().await?;
            output::render_list(
                &items,
                &["UID", "TITLE", "URL"],
                |f| {
                    vec![
                        f.uid.clone(),
                        f.title.clone(),
                        f.url.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        GrafanaFolderAction::Get { uid } => {
            let item = grafana.get_folder(&uid).await?;
            output::render(&item, fmt)
        }
        GrafanaFolderAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.create_folder(&json).await?;
            output::render(&item, fmt)
        }
        GrafanaFolderAction::Update { uid, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.update_folder(&uid, &json).await?;
            output::render(&item, fmt)
        }
        GrafanaFolderAction::Delete { uid } => {
            grafana.delete_folder(&uid).await?;
            output::ok(&format!("Deleted folder {uid}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana Annotation dispatch
// ---------------------------------------------------------------------------

async fn dispatch_grafana_annotation(
    action: GrafanaAnnotationAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let grafana = client.grafana().await?;
    match action {
        GrafanaAnnotationAction::List { params } => {
            let items = grafana.list_annotations(params.as_deref()).await?;
            output::render_list(
                &items,
                &["ID", "TEXT", "TAGS"],
                |a| {
                    vec![
                        a.id.map_or("-".into(), |id| id.to_string()),
                        a.text.clone().unwrap_or_default(),
                        a.tags.join(", "),
                    ]
                },
                fmt,
            )
        }
        GrafanaAnnotationAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.create_annotation(&json).await?;
            output::render(&item, fmt)
        }
        GrafanaAnnotationAction::Delete { id } => {
            grafana.delete_annotation(id).await?;
            output::ok(&format!("Deleted annotation {id}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana Alert dispatch
// ---------------------------------------------------------------------------

async fn dispatch_grafana_alert(
    action: GrafanaAlertAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let grafana = client.grafana().await?;
    match action {
        GrafanaAlertAction::List => {
            let items = grafana.get_alert_rules().await?;
            output::render_list(
                &items,
                &["UID", "TITLE", "CONDITION", "FOLDER", "GROUP"],
                |r| {
                    vec![
                        r.uid.clone().unwrap_or_default(),
                        r.title.clone().unwrap_or_default(),
                        r.condition.clone().unwrap_or_default(),
                        r.folder_uid.clone().unwrap_or_default(),
                        r.rule_group.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        GrafanaAlertAction::Create { data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.create_alert_rule(&json).await?;
            output::render(&item, fmt)
        }
        GrafanaAlertAction::Update { uid, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = grafana.update_alert_rule(&uid, &json).await?;
            output::render(&item, fmt)
        }
        GrafanaAlertAction::Delete { uid } => {
            grafana.delete_alert_rule(&uid).await?;
            output::ok(&format!("Deleted alert rule {uid}"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Grafana Org dispatch
// ---------------------------------------------------------------------------

async fn dispatch_grafana_org(
    action: GrafanaOrgAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let grafana = client.grafana().await?;
    match action {
        GrafanaOrgAction::Get => {
            let item = grafana.get_current_org().await?;
            output::render(&item, fmt)
        }
        GrafanaOrgAction::Update { data } => {
            let json = output::read_json_input(data.as_deref())?;
            grafana.update_org(&json).await?;
            output::ok("Updated organization");
            Ok(())
        }
    }
}
