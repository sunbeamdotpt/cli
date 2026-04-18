use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How a service talks. Drives UI and port-default selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Http,
    #[default]
    Https,
    Grpc,
    Tcp,
}

/// Exposure tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    #[default]
    Internal,
    Public,
    VpnOnly,
}

/// One entry in the registry. Schema mirrors the `sunbeam.pt/*`
/// annotations documented in `docs/service-discovery.md`, reduced to
/// the fields the daemon actually needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEntry {
    /// Short app-wide alias — `sunbeam.pt/slug`.
    pub slug: String,
    /// Display name — `sunbeam.pt/name`.
    pub name: String,
    /// K8s namespace the Service lives in.
    pub namespace: String,
    /// Canonical cluster DNS name (`<svc>.<ns>.svc.cluster.local`).
    pub svc_dns: String,
    /// Primary port number to dial.
    pub port: u16,
    /// Source port name on the Service (preserved for diagnostics only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_name: Option<String>,
    #[serde(default)]
    pub scheme: Scheme,
    #[serde(default)]
    pub tier: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Public hostnames published by the dns-controller.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hostnames: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_interval_secs: Option<u32>,
}

/// Wire format for `services.json`. Wrapping the list in an object leaves
/// room for top-level fields (schema version, cluster id, ...) later.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RegistryFile {
    #[serde(default)]
    pub services: Vec<ServiceEntry>,
}

/// In-memory registry with a slug index.
#[derive(Debug, Clone, Default)]
pub struct ServiceRegistry {
    entries: Vec<ServiceEntry>,
    by_slug: HashMap<String, usize>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_entries(entries: Vec<ServiceEntry>) -> Self {
        let mut by_slug = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            by_slug.insert(e.slug.clone(), i);
        }
        Self { entries, by_slug }
    }

    pub fn lookup(&self, slug: &str) -> Option<&ServiceEntry> {
        self.by_slug.get(slug).map(|i| &self.entries[*i])
    }

    pub fn entries(&self) -> &[ServiceEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Regex-free slug validation: `^[a-z][a-z0-9-]{1,30}$`.
pub fn is_valid_slug(s: &str) -> bool {
    let len = s.len();
    if !(2..=31).contains(&len) {
        return false;
    }
    let mut bytes = s.bytes();
    let first = bytes.next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }
    bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Polls the registry file on a 5s cadence and updates the shared
/// registry on atomic replaces. Missing / invalid file => empty
/// registry; the next good write heals it without a restart.
pub struct RegistryWatcher {
    inner: Arc<RwLock<ServiceRegistry>>,
    _task: JoinHandle<()>,
}

impl RegistryWatcher {
    pub fn spawn(path: PathBuf, cancel: CancellationToken) -> Self {
        let inner = Arc::new(RwLock::new(ServiceRegistry::new()));
        let registry = inner.clone();
        let task = tokio::spawn(async move {
            let mut last_mtime: Option<SystemTime> = None;
            if let Some(mt) = reload(&path, &registry) {
                last_mtime = Some(mt);
            }
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                match tokio::fs::metadata(&path).await {
                    Ok(meta) => {
                        let mt = meta.modified().ok();
                        if mt != last_mtime {
                            if let Some(new_mt) = reload(&path, &registry) {
                                last_mtime = Some(new_mt);
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if last_mtime.is_some() {
                            tracing::info!(
                                "service registry file disappeared: {}",
                                path.display()
                            );
                            *registry.write().unwrap() = ServiceRegistry::new();
                            last_mtime = None;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("registry stat error: {e}");
                    }
                }
            }
        });
        Self {
            inner,
            _task: task,
        }
    }

    pub fn registry(&self) -> Arc<RwLock<ServiceRegistry>> {
        self.inner.clone()
    }
}

fn reload(path: &Path, reg: &Arc<RwLock<ServiceRegistry>>) -> Option<SystemTime> {
    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("service registry not present: {}", path.display());
            return None;
        }
        Err(e) => {
            tracing::warn!("service registry read: {e}");
            return None;
        }
    };
    let parsed: RegistryFile = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("service registry parse: {e}");
            return None;
        }
    };
    let count = parsed.services.len();
    *reg.write().unwrap() = ServiceRegistry::from_entries(parsed.services);
    tracing::info!("service registry reloaded: {count} entries");
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample() -> ServiceEntry {
        ServiceEntry {
            slug: "hydra".into(),
            name: "Hydra".into(),
            namespace: "ory".into(),
            svc_dns: "hydra-public.ory.svc.cluster.local".into(),
            port: 4444,
            port_name: Some("public".into()),
            scheme: Scheme::Https,
            tier: Tier::Internal,
            category: Some("auth".into()),
            hostnames: vec!["hydra.sunbeam.pt".into()],
            description: Some("OAuth2/OIDC".into()),
            icon: None,
            owner_team: Some("auth".into()),
            docs_url: None,
            health_path: Some("/health/ready".into()),
            health_interval_secs: Some(30),
        }
    }

    #[test]
    fn slug_validation() {
        assert!(is_valid_slug("hydra"));
        assert!(is_valid_slug("x0"));
        assert!(is_valid_slug("auth-server"));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("x"));
        assert!(!is_valid_slug("Hydra"));
        assert!(!is_valid_slug("0auth"));
        assert!(!is_valid_slug("-hydra"));
        assert!(!is_valid_slug("foo.bar"));
        assert!(!is_valid_slug("foo_bar"));
        assert!(!is_valid_slug(&"a".repeat(32)));
    }

    #[test]
    fn registry_indexes_by_slug() {
        let reg = ServiceRegistry::from_entries(vec![sample()]);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.lookup("hydra").unwrap().port, 4444);
        assert!(reg.lookup("missing").is_none());
    }

    #[test]
    fn registry_json_round_trip() {
        let file = RegistryFile {
            services: vec![sample()],
        };
        let s = serde_json::to_string(&file).unwrap();
        let parsed: RegistryFile = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.services.len(), 1);
        assert_eq!(parsed.services[0].slug, "hydra");
    }

    #[test]
    fn registry_defaults_fill_in() {
        let minimal = r#"{"services":[{
            "slug":"foo","name":"Foo","namespace":"default",
            "svc_dns":"foo.default.svc.cluster.local","port":80
        }]}"#;
        let f: RegistryFile = serde_json::from_str(minimal).unwrap();
        assert_eq!(f.services[0].scheme, Scheme::Https);
        assert_eq!(f.services[0].tier, Tier::Internal);
        assert!(f.services[0].hostnames.is_empty());
    }

    #[tokio::test]
    async fn watcher_picks_up_initial_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("services.json");
        std::fs::write(
            &path,
            serde_json::to_string(&RegistryFile {
                services: vec![sample()],
            })
            .unwrap(),
        )
        .unwrap();

        let cancel = CancellationToken::new();
        let watcher = RegistryWatcher::spawn(path, cancel.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;

        let reg = watcher.registry();
        let r = reg.read().unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r.lookup("hydra").unwrap().svc_dns, sample().svc_dns);
        cancel.cancel();
    }

    #[tokio::test]
    async fn watcher_empty_when_file_absent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("services.json");
        let cancel = CancellationToken::new();
        let watcher = RegistryWatcher::spawn(path, cancel.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(watcher.registry().read().unwrap().is_empty());
        cancel.cancel();
    }

    #[tokio::test]
    async fn watcher_ignores_invalid_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("services.json");
        std::fs::write(&path, b"not json").unwrap();
        let cancel = CancellationToken::new();
        let watcher = RegistryWatcher::spawn(path, cancel.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(watcher.registry().read().unwrap().is_empty());
        cancel.cancel();
    }
}
