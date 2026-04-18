use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::client::{DerpClient, DerpTlsMode};
use crate::keys::NodeKeys;
use crate::proto::types::DerpMap;

/// Manages DERP relay connections across multiple regions.
///
/// Each region runs a single task that multiplexes reads and writes on one
/// `DerpClient`. When the client errors the task exits; the manager detects
/// the dead channel on the next `send_to_peer` and reconnects lazily.
pub(crate) struct DerpManager {
    home_region: u16,
    active: HashMap<u16, ActiveRegion>,
    /// Learned routes: peer_key -> region they were last seen on.
    peer_route: HashMap<[u8; 32], u16>,
    url_scheme: String,
    keys: Arc<NodeKeys>,
    tls_mode: DerpTlsMode,
    derp_map: DerpMap,
    /// Channel to deliver received packets to the WG loop.
    /// Tuple: (region_id, src_key, data).
    in_tx: mpsc::Sender<(u16, [u8; 32], Vec<u8>)>,
    cancel: CancellationToken,
}

struct ActiveRegion {
    out_tx: mpsc::Sender<([u8; 32], Vec<u8>)>,
    task: JoinHandle<()>,
    region_cancel: CancellationToken,
    last_write: Instant,
}

impl DerpManager {
    /// Create a new manager. Does NOT connect to anything yet -- call
    /// `ensure_home()` after construction.
    pub fn new(
        home_region: u16,
        derp_map: DerpMap,
        keys: Arc<NodeKeys>,
        tls_mode: DerpTlsMode,
        url_scheme: &str,
        in_tx: mpsc::Sender<(u16, [u8; 32], Vec<u8>)>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            home_region,
            active: HashMap::new(),
            peer_route: HashMap::new(),
            url_scheme: url_scheme.to_owned(),
            keys,
            tls_mode,
            derp_map,
            in_tx,
            cancel,
        }
    }

    /// Ensure the home region connection is active. Spawns reader+writer if
    /// not already running.
    pub async fn ensure_home(&mut self) {
        if self.active.contains_key(&self.home_region) {
            return;
        }
        if let Err(e) = self.connect_region(self.home_region).await {
            tracing::warn!("failed to connect home DERP region {}: {e}", self.home_region);
        }
    }

    /// Send a packet to a peer via DERP. Routes to the peer's learned
    /// region or falls back to home.
    pub async fn send_to_peer(&mut self, peer_key: &[u8; 32], data: Vec<u8>) {
        let target = self.peer_route.get(peer_key).copied().unwrap_or(self.home_region);

        // Ensure we have a connection to the target region.
        if !self.active.contains_key(&target)
            && let Err(e) = self.connect_region(target).await {
                tracing::warn!("DERP connect to region {target} failed: {e}");
                // Fall back to home if target != home.
                if target != self.home_region
                    && let Some(home) = self.active.get_mut(&self.home_region) {
                        home.last_write = Instant::now();
                        let _ = home.out_tx.send((*peer_key, data)).await;
                    }
                return;
            }

        if let Some(region) = self.active.get_mut(&target) {
            region.last_write = Instant::now();
            if region.out_tx.send((*peer_key, data.clone())).await.is_err() {
                // Channel closed -- region task died, remove it.
                tracing::warn!("DERP region {target} channel closed, removing");
                self.remove_region(target);
                // Retry once on home if this wasn't already the home region.
                if target != self.home_region
                    && let Some(home) = self.active.get_mut(&self.home_region) {
                        home.last_write = Instant::now();
                        let _ = home.out_tx.send((*peer_key, data)).await;
                    }
            }
        }
    }

    /// Record that a peer was seen on a specific region.
    pub fn learn_peer_route(&mut self, peer_key: &[u8; 32], region: u16) {
        self.peer_route.insert(*peer_key, region);
    }

    /// Update home region (e.g. after netcheck determines a closer one).
    pub async fn set_home_region(&mut self, region: u16) {
        if self.home_region == region {
            return;
        }
        self.home_region = region;
        self.ensure_home().await;
    }

    /// Update the DERP map (e.g. from a new netmap push).
    pub fn update_derp_map(&mut self, map: DerpMap) {
        self.derp_map = map;
    }

    /// Drop non-home regions that have been idle for more than 60 seconds.
    pub fn clean_stale(&mut self) {
        let now = Instant::now();
        let stale_timeout = Duration::from_secs(60);
        let stale: Vec<u16> = self
            .active
            .iter()
            .filter(|(id, region)| {
                **id != self.home_region && now.duration_since(region.last_write) > stale_timeout
            })
            .map(|(&id, _)| id)
            .collect();
        for id in stale {
            tracing::debug!("cleaning stale DERP connection to region {id}");
            self.remove_region(id);
        }
    }

    /// Number of active region connections.
    #[allow(dead_code)]
    pub fn active_region_count(&self) -> usize {
        self.active.len()
    }

    /// Shut down all connections.
    pub async fn shutdown(&mut self) {
        let ids: Vec<u16> = self.active.keys().copied().collect();
        for id in ids {
            self.remove_region(id);
        }
    }

    // ---- internal ----

    /// Look up the first node in a region and build a DERP URL for it.
    fn region_url(&self, region_id: u16) -> Option<String> {
        for region in self.derp_map.regions.values() {
            if region.region_id == region_id
                && let Some(node) = region.nodes.first() {
                    let host = apply_host_override(&node.host_name);
                    return Some(format!(
                        "{}://{}:{}",
                        self.url_scheme, host, node.derp_port,
                    ));
                }
        }
        None
    }

    /// Connect to a region, spawn the combined read/write task.
    async fn connect_region(&mut self, region_id: u16) -> crate::Result<()> {
        let url = self.region_url(region_id).ok_or_else(|| {
            crate::Error::Derp(format!("no DERP node found for region {region_id}"))
        })?;

        tracing::info!("connecting to DERP region {region_id} at {url}");
        let client = DerpClient::connect_with_tls(&url, &self.keys, self.tls_mode).await?;

        let (out_tx, out_rx) = mpsc::channel::<([u8; 32], Vec<u8>)>(64);
        let region_cancel = self.cancel.child_token();
        let in_tx = self.in_tx.clone();
        let cancel = region_cancel.clone();

        let task = tokio::spawn(run_region_loop(region_id, client, out_rx, in_tx, cancel));

        self.active.insert(
            region_id,
            ActiveRegion {
                out_tx,
                task,
                region_cancel,
                last_write: Instant::now(),
            },
        );
        Ok(())
    }

    /// Cancel and remove a region, aborting its task.
    fn remove_region(&mut self, id: u16) {
        if let Some(region) = self.active.remove(&id) {
            region.region_cancel.cancel();
            region.task.abort();
        }
    }
}

/// Test hook: remap a DERP host name via env var.
///
/// `SUNBEAM_NET_DERP_HOST_OVERRIDE` takes a comma-separated list of
/// `from=to` pairs — e.g. `headscale=127.0.0.1`. Used by integration
/// tests where the client runs on the host but the DerpMap advertises
/// docker-internal hostnames. No-op when the env var is unset, which
/// is the only production path.
pub(crate) fn apply_host_override(host: &str) -> String {
    let Ok(spec) = std::env::var("SUNBEAM_NET_DERP_HOST_OVERRIDE") else {
        return host.to_owned();
    };
    for pair in spec.split(',') {
        if let Some((from, to)) = pair.split_once('=')
            && from.trim() == host
        {
            return to.trim().to_owned();
        }
    }
    host.to_owned()
}

/// Single task per region: multiplexes reads and writes on one `DerpClient`.
async fn run_region_loop(
    region_id: u16,
    mut client: DerpClient,
    mut out_rx: mpsc::Receiver<([u8; 32], Vec<u8>)>,
    in_tx: mpsc::Sender<(u16, [u8; 32], Vec<u8>)>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            msg = out_rx.recv() => {
                match msg {
                    Some((dest_key, data)) => {
                        if let Err(e) = client.send_packet(&dest_key, &data).await {
                            tracing::warn!("DERP region {region_id} send error: {e}");
                            return;
                        }
                    }
                    None => return,
                }
            }
            result = client.recv_packet() => {
                match result {
                    Ok((src_key, data)) => {
                        if in_tx.send((region_id, src_key, data)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("DERP region {region_id} recv error: {e}");
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::types::{DerpNode, DerpRegion};

    fn test_derp_map() -> DerpMap {
        let mut regions = HashMap::new();
        regions.insert(
            "1".to_string(),
            DerpRegion {
                region_id: 1,
                region_code: "us-east".into(),
                region_name: "US East".into(),
                nodes: vec![DerpNode {
                    name: "1a".into(),
                    region_id: 1,
                    host_name: "derp1.example.com".into(),
                    ipv4: "1.2.3.4".into(),
                    ipv6: "::1".into(),
                    derp_port: 443,
                    stun_port: 3478,
                    stun_only: None,
                }],
            },
        );
        regions.insert(
            "2".to_string(),
            DerpRegion {
                region_id: 2,
                region_code: "eu-west".into(),
                region_name: "EU West".into(),
                nodes: vec![DerpNode {
                    name: "2a".into(),
                    region_id: 2,
                    host_name: "derp2.example.com".into(),
                    ipv4: "5.6.7.8".into(),
                    ipv6: "::2".into(),
                    derp_port: 443,
                    stun_port: 3478,
                    stun_only: None,
                }],
            },
        );
        DerpMap { regions }
    }

    type RecvPkt = (u16, [u8; 32], Vec<u8>);

    fn make_manager() -> (DerpManager, mpsc::Receiver<RecvPkt>) {
        let (in_tx, in_rx) = mpsc::channel(64);
        let keys = Arc::new(NodeKeys::generate());
        let cancel = CancellationToken::new();
        let mgr = DerpManager::new(
            1,
            test_derp_map(),
            keys,
            DerpTlsMode::default(),
            "https",
            in_tx,
            cancel,
        );
        (mgr, in_rx)
    }

    /// Helper: insert a fake ActiveRegion backed by a channel pair.
    /// Returns the receiver so the test can inspect sent packets.
    fn insert_fake_region(
        mgr: &mut DerpManager,
        region_id: u16,
        last_write: Instant,
    ) -> mpsc::Receiver<([u8; 32], Vec<u8>)> {
        let (out_tx, out_rx) = mpsc::channel(64);
        let region_cancel = mgr.cancel.child_token();
        // Spawn a no-op task that just waits for cancellation.
        let cancel = region_cancel.clone();
        let task = tokio::spawn(async move {
            cancel.cancelled().await;
        });
        mgr.active.insert(
            region_id,
            ActiveRegion {
                out_tx,
                task,
                region_cancel,
                last_write,
            },
        );
        out_rx
    }

    #[test]
    fn test_new_manager() {
        let (mgr, _rx) = make_manager();
        assert_eq!(mgr.home_region, 1);
        assert!(mgr.active.is_empty());
        assert!(mgr.peer_route.is_empty());
        assert_eq!(mgr.active_region_count(), 0);
    }

    #[test]
    fn test_learn_peer_route() {
        let (mut mgr, _rx) = make_manager();
        let peer = [0xAA; 32];
        mgr.learn_peer_route(&peer, 2);
        assert_eq!(mgr.peer_route.get(&peer), Some(&2));
    }

    #[test]
    fn test_learn_peer_route_overwrite() {
        let (mut mgr, _rx) = make_manager();
        let peer = [0xBB; 32];
        mgr.learn_peer_route(&peer, 1);
        assert_eq!(mgr.peer_route.get(&peer), Some(&1));
        mgr.learn_peer_route(&peer, 3);
        assert_eq!(mgr.peer_route.get(&peer), Some(&3));
    }

    #[tokio::test]
    async fn test_clean_stale_keeps_home() {
        let (mut mgr, _rx) = make_manager();
        // Insert home region with an old last_write.
        let old = Instant::now() - Duration::from_secs(120);
        let _home_rx = insert_fake_region(&mut mgr, 1, old);
        assert_eq!(mgr.active_region_count(), 1);
        mgr.clean_stale();
        // Home region must survive even though it's old.
        assert_eq!(mgr.active_region_count(), 1);
        assert!(mgr.active.contains_key(&1));
    }

    #[tokio::test]
    async fn test_clean_stale_removes_old() {
        let (mut mgr, _rx) = make_manager();
        let old = Instant::now() - Duration::from_secs(120);
        let _home_rx = insert_fake_region(&mut mgr, 1, Instant::now());
        let _r2_rx = insert_fake_region(&mut mgr, 2, old);
        assert_eq!(mgr.active_region_count(), 2);
        mgr.clean_stale();
        assert_eq!(mgr.active_region_count(), 1);
        assert!(mgr.active.contains_key(&1));
        assert!(!mgr.active.contains_key(&2));
    }

    #[tokio::test]
    async fn test_send_to_unknown_peer_uses_home() {
        let (mut mgr, _rx) = make_manager();
        let mut home_rx = insert_fake_region(&mut mgr, 1, Instant::now());
        let peer = [0xCC; 32];
        let payload = vec![1, 2, 3, 4];

        mgr.send_to_peer(&peer, payload.clone()).await;

        // The packet should arrive on the home region channel.
        let (dest, data) = home_rx.try_recv().expect("packet should be on home channel");
        assert_eq!(dest, peer);
        assert_eq!(data, payload);
    }

    #[tokio::test]
    async fn test_send_to_learned_peer_uses_learned_region() {
        let (mut mgr, _rx) = make_manager();
        let _home_rx = insert_fake_region(&mut mgr, 1, Instant::now());
        let mut r2_rx = insert_fake_region(&mut mgr, 2, Instant::now());
        let peer = [0xDD; 32];
        mgr.learn_peer_route(&peer, 2);

        let payload = vec![5, 6, 7];
        mgr.send_to_peer(&peer, payload.clone()).await;

        let (dest, data) = r2_rx.try_recv().expect("packet should be on region 2 channel");
        assert_eq!(dest, peer);
        assert_eq!(data, payload);
    }

    #[tokio::test]
    async fn test_region_url_lookup() {
        let (mgr, _rx) = make_manager();
        assert_eq!(
            mgr.region_url(1),
            Some("https://derp1.example.com:443".into())
        );
        assert_eq!(
            mgr.region_url(2),
            Some("https://derp2.example.com:443".into())
        );
        assert_eq!(mgr.region_url(99), None);
    }

    #[tokio::test]
    async fn test_shutdown_clears_all() {
        let (mut mgr, _rx) = make_manager();
        let _home_rx = insert_fake_region(&mut mgr, 1, Instant::now());
        let _r2_rx = insert_fake_region(&mut mgr, 2, Instant::now());
        assert_eq!(mgr.active_region_count(), 2);
        mgr.shutdown().await;
        assert_eq!(mgr.active_region_count(), 0);
    }

    #[tokio::test]
    async fn test_update_derp_map() {
        let (mut mgr, _rx) = make_manager();
        assert_eq!(mgr.derp_map.regions.len(), 2);
        mgr.update_derp_map(DerpMap {
            regions: HashMap::new(),
        });
        assert!(mgr.derp_map.regions.is_empty());
    }

    #[tokio::test]
    async fn test_set_home_region_noop_if_same() {
        let (mut mgr, _rx) = make_manager();
        // Should not attempt connection if region unchanged.
        mgr.set_home_region(1).await;
        assert!(mgr.active.is_empty());
    }

    #[tokio::test]
    async fn test_clean_stale_keeps_recent_non_home() {
        let (mut mgr, _rx) = make_manager();
        let _home_rx = insert_fake_region(&mut mgr, 1, Instant::now());
        let _r2_rx = insert_fake_region(&mut mgr, 2, Instant::now());
        assert_eq!(mgr.active_region_count(), 2);
        mgr.clean_stale();
        // Both should survive -- region 2 was written to recently.
        assert_eq!(mgr.active_region_count(), 2);
    }
}
