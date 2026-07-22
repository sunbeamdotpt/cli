//! Dynamic service registry — discovers services from K8s labels/annotations.
//!
//! Services are found by querying Deployments, StatefulSets, DaemonSets, and
//! ConfigMaps that carry the `sunbeam.pt/service` label.
#![allow(dead_code)]

use std::collections::HashMap;

use sdk::k8s_openapi;
use sdk::kube_rs as kube;

use kube::Client;
use kube::api::{Api, ListParams};
use sdk::debug;
// ── Label / annotation keys ──────────────────────────────────────────

const LABEL_SERVICE: &str = "sunbeam.pt/service";
const LABEL_CATEGORY: &str = "sunbeam.pt/category";
const LABEL_VIRTUAL: &str = "sunbeam.pt/virtual";
const ANN_DISPLAY_NAME: &str = "sunbeam.pt/display-name";
const ANN_KV_PATH: &str = "sunbeam.pt/kv-path";
const ANN_DB_USER: &str = "sunbeam.pt/db-user";
const ANN_DB_NAME: &str = "sunbeam.pt/db-name";
const ANN_BUILD_TARGET: &str = "sunbeam.pt/build-target";
const ANN_DEPENDS_ON: &str = "sunbeam.pt/depends-on";
const ANN_HEALTH_CHECK: &str = "sunbeam.pt/health-check";
const ANN_POD_SELECTOR: &str = "sunbeam.pt/pod-selector";
const ANN_SHELL_COMMAND: &str = "sunbeam.pt/shell-command";
/// Comma-separated list of destination ports this service speaks (e.g.
/// `"5432"`, `"80,443"`). Used to tighten the SOCKS5 proxy's allow-port
/// list so the VPN only exposes the ports actually needed for dogfood.
const ANN_PORTS: &str = "sunbeam.pt/ports";

// ── Core types ───────────────────────────────────────────────────────

/// A service definition constructed from K8s resource metadata.
#[derive(Debug, Clone)]
pub struct ServiceDefinition {
    /// Name.
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// Category.
    pub category: Category,
    /// Namespace.
    pub namespace: String,
    /// Deployments.
    pub deployments: Vec<String>,
    /// Kv path.
    pub kv_path: Option<String>,
    /// Database.
    pub database: Option<DbConfig>,
    /// Build target.
    pub build_target: Option<String>,
    /// Depends on.
    pub depends_on: Vec<String>,
    /// Health.
    pub health: HealthCheck,
    /// Virtual service.
    pub virtual_service: bool,
    /// Resource kind.
    pub resource_kind: String,
    /// Label selector used to find the pod backing this service.
    /// When absent, callers fall back to `app=<first deployment>`.
    pub pod_selector: Option<String>,
    /// Command to exec for an interactive shell (e.g. `psql -U postgres`).
    /// When absent, callers fall back to `/bin/sh`.
    pub shell_command: Option<String>,
    /// TCP destination ports this service listens on inside the cluster.
    /// Populated from the `sunbeam.pt/ports` annotation (comma-separated).
    /// Drives the SOCKS5 proxy's allow-port list — the VPN only needs to
    /// permit ports that real services actually use.
    pub ports: Vec<u16>,
}

/// Database credentials for a service's CNPG-managed database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbConfig {
    /// Username.
    pub username: String,
    /// Database.
    pub database: String,
}

/// Logical grouping of services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Auth.
    Auth,
    /// Data.
    Data,
    /// Devtools.
    DevTools,
    /// Platform.
    Platform,
    /// Messaging.
    Messaging,
    /// Media.
    Media,
    /// Storage.
    Storage,
    /// Monitoring.
    Monitoring,
    /// Infra.
    Infra,
    /// Unknown.
    Unknown,
}

const ALL_CATEGORIES: &[Category] = &[
    Category::Auth,
    Category::Data,
    Category::DevTools,
    Category::Platform,
    Category::Messaging,
    Category::Media,
    Category::Storage,
    Category::Monitoring,
    Category::Infra,
];

impl Category {
    /// Parse a category from a label value (case-insensitive).
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "auth" => Self::Auth,
            "data" => Self::Data,
            "devtools" => Self::DevTools,
            "platform" => Self::Platform,
            "messaging" => Self::Messaging,
            "media" => Self::Media,
            "storage" => Self::Storage,
            "monitoring" => Self::Monitoring,
            "infra" => Self::Infra,
            _ => Self::Unknown,
        }
    }

    /// Lowercase name used for CLI matching.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Data => "data",
            Self::DevTools => "devtools",
            Self::Platform => "platform",
            Self::Messaging => "messaging",
            Self::Media => "media",
            Self::Storage => "storage",
            Self::Monitoring => "monitoring",
            Self::Infra => "infra",
            Self::Unknown => "unknown",
        }
    }

    /// Display-friendly name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Auth => "Auth",
            Self::Data => "Data",
            Self::DevTools => "DevTools",
            Self::Platform => "Platform",
            Self::Messaging => "Messaging",
            Self::Media => "Media",
            Self::Storage => "Storage",
            Self::Monitoring => "Monitoring",
            Self::Infra => "Infra",
            Self::Unknown => "Other",
        }
    }

    /// All known category variants (excluding Unknown).
    pub fn all_categories() -> &'static [Category] {
        ALL_CATEGORIES
    }
}

/// How to determine if a service is healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthCheck {
    /// Podready.
    PodReady,
    /// Http.
    Http {
        /// HTTP path to probe.
        path: String,
    },
    /// Custom.
    Custom(String),
    /// None.
    None,
}

// ── ServiceRegistry ──────────────────────────────────────────────────

/// In-memory registry of discovered services.
#[derive(Debug, Clone, Default)]
pub struct ServiceRegistry {
    /// Services.
    pub services: HashMap<String, ServiceDefinition>,
}

impl ServiceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// All services, sorted by name.
    pub fn all(&self) -> Vec<&ServiceDefinition> {
        let mut svcs: Vec<_> = self.services.values().collect();
        svcs.sort_by_key(|s| &s.name);
        svcs
    }

    /// Look up a service by exact name.
    pub fn get(&self, name: &str) -> Option<&ServiceDefinition> {
        self.services.get(name)
    }

    /// All services belonging to a given category, sorted by name.
    pub fn by_category(&self, cat: Category) -> Vec<&ServiceDefinition> {
        let mut svcs: Vec<_> = self
            .services
            .values()
            .filter(|s| s.category == cat)
            .collect();
        svcs.sort_by_key(|s| &s.name);
        svcs
    }

    /// All services in a given Kubernetes namespace, sorted by name.
    pub fn by_namespace(&self, ns: &str) -> Vec<&ServiceDefinition> {
        let mut svcs: Vec<_> = self
            .services
            .values()
            .filter(|s| s.namespace == ns)
            .collect();
        svcs.sort_by_key(|s| &s.name);
        svcs
    }

    /// Resolve a user-supplied string to one or more services.
    ///
    /// Matching order:
    /// 1. Exact service name
    /// 2. Category name (case-insensitive)
    /// 3. Namespace name (case-insensitive)
    pub fn resolve(&self, input: &str) -> Vec<&ServiceDefinition> {
        // 1. Exact name match
        if let Some(svc) = self.services.get(input) {
            return vec![svc];
        }

        let input_lower = input.to_ascii_lowercase();

        // 2. Category match
        for cat in Category::all_categories() {
            if cat.name() == input_lower {
                return self.by_category(*cat);
            }
        }

        // 3. Namespace match
        let by_ns = self.by_namespace(&input_lower);
        if !by_ns.is_empty() {
            return by_ns;
        }

        vec![]
    }

    /// Deduplicated set of destination ports across all services,
    /// sorted ascending. Used to tighten the SOCKS5 proxy's allow-port
    /// list: the VPN only permits CONNECT to ports that at least one
    /// registered service declared via `sunbeam.pt/ports`.
    ///
    /// Infrastructure ports the CLI itself needs (DNS, kube-apiserver)
    /// are NOT folded in here — callers layer them on explicitly so
    /// that a missing/stale registry can't silently lock us out of the
    /// control plane.
    pub fn aggregated_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .services
            .values()
            .flat_map(|s| s.ports.iter().copied())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    /// Unique namespaces across all services, sorted.
    pub fn namespaces(&self) -> Vec<&str> {
        let mut ns: Vec<&str> = self
            .services
            .values()
            .map(|s| s.namespace.as_str())
            .collect();
        ns.sort_unstable();
        ns.dedup();
        ns
    }
}

// ── Discovery ────────────────────────────────────────────────────────

/// Discover all services from K8s resources with `sunbeam.pt/service` labels.
#[tracing::instrument(skip(logger, client))]
pub async fn discover(
    logger: &sdk::logger::Logger,
    client: &Client,
) -> sdk::error::Result<ServiceRegistry> {
    let mut services: HashMap<String, ServiceDefinition> = HashMap::new();

    // Query Deployments
    discover_resources::<k8s_openapi::api::apps::v1::Deployment>(
        logger,
        client,
        &mut services,
        "Deployment",
    )
    .await?;

    // Query StatefulSets
    discover_resources::<k8s_openapi::api::apps::v1::StatefulSet>(
        logger,
        client,
        &mut services,
        "StatefulSet",
    )
    .await?;

    // Query DaemonSets
    discover_resources::<k8s_openapi::api::apps::v1::DaemonSet>(
        logger,
        client,
        &mut services,
        "DaemonSet",
    )
    .await?;

    // Query ConfigMaps (for virtual/external services)
    discover_resources::<k8s_openapi::api::core::v1::ConfigMap>(
        logger,
        client,
        &mut services,
        "ConfigMap",
    )
    .await?;

    Ok(ServiceRegistry { services })
}

async fn discover_resources<R>(
    logger: &sdk::logger::Logger,
    client: &Client,
    services: &mut HashMap<String, ServiceDefinition>,
    kind: &str,
) -> sdk::error::Result<()>
where
    R: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + std::fmt::Debug
        + serde::de::DeserializeOwned
        + kube::ResourceExt
        + k8s_openapi::Metadata<Ty = k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta>,
    <R as kube::Resource>::DynamicType: Default,
{
    let api: Api<R> = Api::all(client.clone());
    let lp = ListParams::default().labels(LABEL_SERVICE);
    let list = api
        .list(&lp)
        .await
        .map_err(|e| sdk::error::SunbeamError::kube(format!("discover {kind}: {e}")))?;
    debug!(
        logger,
        "discover_resources",
        kind = kind,
        count = list.items.len()
    );

    for resource in &list.items {
        let meta = resource.meta();
        let labels = meta.labels.as_ref();
        let annotations = meta.annotations.as_ref();
        let ns = meta.namespace.as_deref().unwrap_or("default");
        let resource_name = meta.name.as_deref().unwrap_or("");

        let service_name = labels
            .and_then(|l| l.get(LABEL_SERVICE))
            .cloned()
            .unwrap_or_default();

        if service_name.is_empty() {
            continue;
        }

        let category_str = labels
            .and_then(|l| l.get(LABEL_CATEGORY))
            .map(|s| s.as_str())
            .unwrap_or("unknown");

        let is_virtual = labels
            .and_then(|l| l.get(LABEL_VIRTUAL))
            .map(|v| v == "true")
            .unwrap_or(false);

        let ann = |key: &str| -> Option<String> { annotations.and_then(|a| a.get(key)).cloned() };

        // If this service already exists (e.g. multiple deployments), add the resource name
        if let Some(existing) = services.get_mut(&service_name) {
            if (kind == "Deployment" || kind == "StatefulSet" || kind == "DaemonSet") && !is_virtual
            {
                existing.deployments.push(resource_name.to_string());
            }
            continue;
        }

        let display_name = ann(ANN_DISPLAY_NAME).unwrap_or_else(|| service_name.clone());
        let kv_path = ann(ANN_KV_PATH).filter(|s| !s.is_empty());

        let database = match (ann(ANN_DB_USER), ann(ANN_DB_NAME)) {
            (Some(user), Some(db)) if !user.is_empty() && !db.is_empty() => Some(DbConfig {
                username: user,
                database: db,
            }),
            _ => None,
        };

        let build_target = ann(ANN_BUILD_TARGET).filter(|s| !s.is_empty());

        let depends_on: Vec<String> = ann(ANN_DEPENDS_ON)
            .map(|s| {
                s.split(',')
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let health = match ann(ANN_HEALTH_CHECK).as_deref() {
            Some("none") | None if is_virtual => HealthCheck::None,
            Some("cnpg") => HealthCheck::Custom("cnpg".into()),
            Some("seal-status") => HealthCheck::Custom("seal-status".into()),
            Some(path) if path.starts_with('/') => HealthCheck::Http { path: path.into() },
            _ => HealthCheck::PodReady,
        };

        let mut deployments = Vec::new();
        if (kind == "Deployment" || kind == "StatefulSet" || kind == "DaemonSet") && !is_virtual {
            deployments.push(resource_name.to_string());
        }

        let pod_selector = ann(ANN_POD_SELECTOR).filter(|s| !s.is_empty());
        let shell_command = ann(ANN_SHELL_COMMAND).filter(|s| !s.is_empty());
        let ports: Vec<u16> = ann(ANN_PORTS)
            .map(|s| {
                s.split(',')
                    .filter_map(|p| p.trim().parse::<u16>().ok())
                    .collect()
            })
            .unwrap_or_default();

        debug!(
            logger,
            "discover_resources added",
            kind = kind,
            service_name = service_name,
            ns = ns
        );
        services.insert(
            service_name.clone(),
            ServiceDefinition {
                name: service_name,
                display_name,
                category: Category::parse(category_str),
                namespace: ns.to_string(),
                deployments,
                kv_path,
                database,
                build_target,
                depends_on,
                health,
                virtual_service: is_virtual,
                resource_kind: kind.to_string(),
                pod_selector,
                shell_command,
                ports,
            },
        );
    }

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_registry() -> ServiceRegistry {
        let mut services = HashMap::new();
        services.insert(
            "hydra".into(),
            ServiceDefinition {
                name: "hydra".into(),
                display_name: "Hydra (OAuth2/OIDC)".into(),
                category: Category::Auth,
                namespace: "ory".into(),
                deployments: vec!["hydra".into()],
                kv_path: Some("hydra".into()),
                database: Some(DbConfig {
                    username: "hydra".into(),
                    database: "hydra_db".into(),
                }),
                build_target: None,
                depends_on: vec!["postgres".into(), "openbao".into()],
                health: HealthCheck::PodReady,
                virtual_service: false,
                resource_kind: "Deployment".into(),
                pod_selector: None,
                shell_command: None,
                ports: vec![],
            },
        );
        services.insert(
            "kratos".into(),
            ServiceDefinition {
                name: "kratos".into(),
                display_name: "Kratos (Identity)".into(),
                category: Category::Auth,
                namespace: "ory".into(),
                deployments: vec!["kratos".into()],
                kv_path: Some("kratos".into()),
                database: Some(DbConfig {
                    username: "kratos".into(),
                    database: "kratos_db".into(),
                }),
                build_target: None,
                depends_on: vec!["postgres".into(), "openbao".into()],
                health: HealthCheck::PodReady,
                virtual_service: false,
                resource_kind: "Deployment".into(),
                pod_selector: None,
                shell_command: None,
                ports: vec![],
            },
        );
        services.insert(
            "login-ui".into(),
            ServiceDefinition {
                name: "login-ui".into(),
                display_name: "Login UI".into(),
                category: Category::Auth,
                namespace: "ory".into(),
                deployments: vec!["login-ui".into()],
                kv_path: Some("login-ui".into()),
                database: None,
                build_target: None,
                depends_on: vec!["kratos".into()],
                health: HealthCheck::PodReady,
                virtual_service: false,
                resource_kind: "Deployment".into(),
                pod_selector: None,
                shell_command: None,
                ports: vec![],
            },
        );
        services.insert(
            "scaleway-s3".into(),
            ServiceDefinition {
                name: "scaleway-s3".into(),
                display_name: "Scaleway S3 (External)".into(),
                category: Category::Storage,
                namespace: "default".into(),
                deployments: vec![],
                kv_path: Some("scaleway-s3".into()),
                database: None,
                build_target: None,
                depends_on: vec![],
                health: HealthCheck::None,
                virtual_service: true,
                resource_kind: "ConfigMap".into(),
                pod_selector: None,
                shell_command: None,
                ports: vec![],
            },
        );
        ServiceRegistry { services }
    }

    #[test]
    fn test_get_by_name() {
        let reg = mock_registry();
        let svc = reg.get("hydra").unwrap();
        assert_eq!(svc.namespace, "ory");
        assert_eq!(svc.category, Category::Auth);
    }

    #[test]
    fn test_get_unknown() {
        let reg = mock_registry();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_resolve_exact_name() {
        let reg = mock_registry();
        let resolved = reg.resolve("hydra");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "hydra");
    }

    #[test]
    fn test_resolve_category() {
        let reg = mock_registry();
        let resolved = reg.resolve("auth");
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn test_resolve_namespace() {
        let reg = mock_registry();
        let resolved = reg.resolve("ory");
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn test_resolve_unknown() {
        let reg = mock_registry();
        assert!(reg.resolve("nonexistent").is_empty());
    }

    #[test]
    fn test_all_sorted() {
        let reg = mock_registry();
        let all = reg.all();
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn test_namespaces() {
        let reg = mock_registry();
        let ns = reg.namespaces();
        assert!(ns.contains(&"ory"));
        assert!(ns.contains(&"default"));
    }

    #[test]
    fn test_virtual_service() {
        let reg = mock_registry();
        let svc = reg.get("scaleway-s3").unwrap();
        assert!(svc.virtual_service);
        assert!(svc.deployments.is_empty());
    }

    #[test]
    fn test_category_from_str() {
        assert_eq!(Category::parse("auth"), Category::Auth);
        assert_eq!(Category::parse("Auth"), Category::Auth);
        assert_eq!(Category::parse("AUTH"), Category::Auth);
        assert_eq!(Category::parse("unknown_thing"), Category::Unknown);
    }

    #[test]
    fn test_category_roundtrip() {
        for cat in Category::all_categories() {
            assert_eq!(Category::parse(cat.name()), *cat);
        }
    }

    #[test]
    fn test_by_category() {
        let reg = mock_registry();
        let auth = reg.by_category(Category::Auth);
        assert_eq!(auth.len(), 3);
        let storage = reg.by_category(Category::Storage);
        assert_eq!(storage.len(), 1);
        assert!(storage[0].virtual_service);
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let reg = mock_registry();
        let r1 = reg.resolve("Auth");
        let r2 = reg.resolve("AUTH");
        assert_eq!(r1.len(), 3);
        assert_eq!(r2.len(), 3);
        assert_eq!(
            r1.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            r2.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_by_namespace() {
        let reg = mock_registry();
        let ory = reg.by_namespace("ory");
        assert_eq!(ory.len(), 3);
        let names: Vec<&str> = ory.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hydra"));
        assert!(names.contains(&"kratos"));
        assert!(names.contains(&"login-ui"));
    }

    #[test]
    fn test_empty_registry() {
        let reg = ServiceRegistry::new();
        assert!(reg.all().is_empty());
        assert!(reg.get("anything").is_none());
        assert!(reg.resolve("anything").is_empty());
        assert!(reg.namespaces().is_empty());
    }

    fn svc_with_ports(name: &str, ports: Vec<u16>) -> ServiceDefinition {
        ServiceDefinition {
            name: name.into(),
            display_name: name.into(),
            category: Category::Platform,
            namespace: "default".into(),
            deployments: vec![],
            kv_path: None,
            database: None,
            build_target: None,
            depends_on: vec![],
            health: HealthCheck::None,
            virtual_service: false,
            resource_kind: "Deployment".into(),
            pod_selector: None,
            shell_command: None,
            ports,
        }
    }

    #[test]
    fn aggregated_ports_is_empty_when_nothing_declared() {
        let reg = ServiceRegistry::new();
        assert!(reg.aggregated_ports().is_empty());
    }

    #[test]
    fn aggregated_ports_dedupes_and_sorts() {
        let mut reg = ServiceRegistry::new();
        reg.services
            .insert("a".into(), svc_with_ports("a", vec![443, 80]));
        reg.services
            .insert("b".into(), svc_with_ports("b", vec![5432]));
        reg.services
            .insert("c".into(), svc_with_ports("c", vec![443, 9200]));
        assert_eq!(reg.aggregated_ports(), vec![80, 443, 5432, 9200]);
    }

    #[test]
    fn aggregated_ports_ignores_services_without_ports() {
        let mut reg = ServiceRegistry::new();
        reg.services
            .insert("a".into(), svc_with_ports("a", vec![80]));
        reg.services.insert("b".into(), svc_with_ports("b", vec![]));
        assert_eq!(reg.aggregated_ports(), vec![80]);
    }

    #[test]
    fn mock_registry_has_no_ports_by_default() {
        // mock_registry pre-dates Phase 6 — make sure the migration path
        // stays explicit: old fixtures should end up with empty port sets
        // until annotations are added.
        let reg = mock_registry();
        assert!(reg.aggregated_ports().is_empty());
    }

    // ---------------------------------------------------------------------
    // Discovery against a mocked apiserver (tower-test)
    // ---------------------------------------------------------------------

    fn empty_list(kind: &str) -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "v1",
            "kind": kind,
            "metadata": {"resourceVersion": "1"},
            "items": []
        })
    }

    fn deployment_list() -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "DeploymentList",
            "metadata": {"resourceVersion": "1"},
            "items": [
                {
                    "metadata": {
                        "name": "hydra",
                        "namespace": "ory",
                        "labels": {
                            "sunbeam.pt/service": "hydra",
                            "sunbeam.pt/category": "auth"
                        },
                        "annotations": {
                            "sunbeam.pt/display-name": "Hydra (OAuth2/OIDC)",
                            "sunbeam.pt/kv-path": "hydra",
                            "sunbeam.pt/db-user": "hydra",
                            "sunbeam.pt/db-name": "hydra_db",
                            "sunbeam.pt/depends-on": "postgres, openbao",
                            "sunbeam.pt/health-check": "/health/ready",
                            "sunbeam.pt/ports": "4444, 4445"
                        }
                    },
                    "spec": {}
                },
                {
                    "metadata": {
                        "name": "unlabeled",
                        "namespace": "ory",
                        "labels": {}
                    },
                    "spec": {}
                }
            ]
        })
    }

    fn statefulset_list() -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "StatefulSetList",
            "metadata": {"resourceVersion": "1"},
            "items": [
                {
                    "metadata": {
                        "name": "postgres",
                        "namespace": "data",
                        "labels": {
                            "sunbeam.pt/service": "postgres",
                            "sunbeam.pt/category": "data"
                        },
                        "annotations": {
                            "sunbeam.pt/health-check": "cnpg"
                        }
                    },
                    "spec": {}
                }
            ]
        })
    }

    fn configmap_list() -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "v1",
            "kind": "ConfigMapList",
            "metadata": {"resourceVersion": "1"},
            "items": [
                {
                    "metadata": {
                        "name": "external-docs",
                        "namespace": "docs",
                        "labels": {
                            "sunbeam.pt/service": "docs",
                            "sunbeam.pt/category": "platform",
                            "sunbeam.pt/virtual": "true"
                        }
                    }
                }
            ]
        })
    }

    #[tokio::test]
    async fn test_discover_from_mock_apiserver() {
        use http::{Request, Response};
        use kube::client::Body;
        use tower_test::mock;

        let (mock_service, mut handle) = mock::pair::<Request<Body>, Response<Body>>();
        let client = Client::new(mock_service, "default");

        let responder = tokio::spawn(async move {
            while let Some((req, send)) = handle.next_request().await {
                let uri = req.uri().to_string();
                let body = if uri.contains("deployments") {
                    deployment_list()
                } else if uri.contains("statefulsets") {
                    statefulset_list()
                } else if uri.contains("configmaps") {
                    configmap_list()
                } else {
                    empty_list("List")
                };
                send.send_response(
                    Response::builder()
                        .status(200)
                        .body(Body::from(body.to_string().into_bytes()))
                        .unwrap(),
                );
            }
        });

        let logger = sdk::logger::Logger::new(sdk::logger::TracingSink);
        let reg = discover(&logger, &client).await.unwrap();
        responder.abort();

        // Deployment-backed service with full annotations.
        let hydra = reg.get("hydra").expect("hydra should be discovered");
        assert_eq!(hydra.namespace, "ory");
        assert_eq!(hydra.display_name, "Hydra (OAuth2/OIDC)");
        assert_eq!(hydra.category, Category::Auth);
        assert_eq!(hydra.deployments, vec!["hydra".to_string()]);
        assert_eq!(hydra.kv_path.as_deref(), Some("hydra"));
        let db = hydra.database.as_ref().expect("db config");
        assert_eq!(db.username, "hydra");
        assert_eq!(db.database, "hydra_db");
        assert_eq!(hydra.depends_on, vec!["postgres", "openbao"]);
        assert_eq!(hydra.ports, vec![4444, 4445]);
        assert!(matches!(
            hydra.health,
            HealthCheck::Http { ref path } if path == "/health/ready"
        ));

        // The unlabeled deployment must be skipped.
        assert!(reg.get("unlabeled").is_none());

        // StatefulSet with custom health check.
        let postgres = reg.get("postgres").expect("postgres should be discovered");
        assert_eq!(postgres.resource_kind, "StatefulSet");
        assert!(matches!(
            postgres.health,
            HealthCheck::Custom(ref c) if c == "cnpg"
        ));

        // Virtual service from a ConfigMap: no deployments, no health check.
        let docs = reg.get("docs").expect("docs should be discovered");
        assert!(docs.virtual_service);
        assert!(docs.deployments.is_empty());
        assert!(matches!(docs.health, HealthCheck::None));

        assert_eq!(reg.all().len(), 3);
    }

    #[tokio::test]
    async fn test_discover_merges_multiple_deployments() {
        use http::{Request, Response};
        use kube::client::Body;
        use tower_test::mock;

        let (mock_service, mut handle) = mock::pair::<Request<Body>, Response<Body>>();
        let client = Client::new(mock_service, "default");

        let two_deployments = serde_json::json!({
            "apiVersion": "apps/v1",
            "kind": "DeploymentList",
            "metadata": {"resourceVersion": "1"},
            "items": [
                {
                    "metadata": {
                        "name": "hydra",
                        "namespace": "ory",
                        "labels": {"sunbeam.pt/service": "hydra"}
                    },
                    "spec": {}
                },
                {
                    "metadata": {
                        "name": "hydra-sidecar",
                        "namespace": "ory",
                        "labels": {"sunbeam.pt/service": "hydra"}
                    },
                    "spec": {}
                }
            ]
        });

        let responder = tokio::spawn(async move {
            while let Some((req, send)) = handle.next_request().await {
                let uri = req.uri().to_string();
                let body = if uri.contains("deployments") {
                    two_deployments.clone()
                } else {
                    empty_list("List")
                };
                send.send_response(
                    Response::builder()
                        .status(200)
                        .body(Body::from(body.to_string().into_bytes()))
                        .unwrap(),
                );
            }
        });

        let logger = sdk::logger::Logger::new(sdk::logger::TracingSink);
        let reg = discover(&logger, &client).await.unwrap();
        responder.abort();

        let hydra = reg.get("hydra").expect("hydra should be discovered");
        assert_eq!(
            hydra.deployments,
            vec!["hydra".to_string(), "hydra-sidecar".to_string()]
        );
        // No category label → Unknown; no health annotation → PodReady.
        assert_eq!(hydra.category, Category::Unknown);
        assert!(matches!(hydra.health, HealthCheck::PodReady));
    }
}
