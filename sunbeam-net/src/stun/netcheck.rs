//! Probe DERP servers via STUN to discover the external IP:port.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{debug, trace};

use crate::proto::types::DerpMap;
use crate::stun::{self, TxId};

/// STUN netcheck client.
pub struct Client {
    /// Per-probe timeout (reserved for per-server retry logic).
    #[allow(dead_code)]
    probe_timeout: Duration,
    /// Overall timeout across all probes.
    overall_timeout: Duration,
}

/// Result of a netcheck run.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct Report {
    /// Whether we received any UDP response at all.
    pub udp: bool,
    /// Whether we have a global IPv4 address.
    pub ipv4: bool,
    /// Whether we have a global IPv6 address.
    pub ipv6: bool,
    /// Discovered global IPv4 endpoint.
    pub global_v4: Option<SocketAddr>,
    /// Discovered global IPv6 endpoint.
    pub global_v6: Option<SocketAddr>,
    /// DERP region with the lowest latency.
    pub preferred_derp: u16,
    /// Latency to each DERP region that responded.
    pub region_latency: HashMap<u16, Duration>,
    /// `Some(true)` if different STUN servers saw different source ports
    /// (symmetric NAT), `Some(false)` if consistent, `None` if we can't tell.
    pub mapping_varies: Option<bool>,
}


impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create a new netcheck client with default timeouts.
    pub fn new() -> Self {
        Self {
            probe_timeout: Duration::from_secs(3),
            overall_timeout: Duration::from_secs(5),
        }
    }

    /// Run a STUN check against every region in `derp_map` and return a
    /// [`Report`] summarising the results.
    pub async fn report(&self, derp_map: &DerpMap, socket: &UdpSocket) -> Report {
        let mut report = Report::default();

        // Collect (region_id, stun_addr) targets.
        let mut targets: Vec<(u16, SocketAddr, TxId)> = Vec::new();

        for region in derp_map.regions.values() {
            if let Some(node) = region.nodes.iter().find(|n| n.stun_port > 0) {
                let port = node.stun_port as u16;
                let resolved = crate::derp::manager::apply_host_override(&node.host_name);
                let host = &resolved;

                let ip = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                    ip
                } else {
                    // Async DNS via tokio lookup_host. Bounded by the per-probe
                    // timeout handled further below; here we use a shorter cap
                    // so one dead resolver doesn't starve the probe phase.
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        tokio::net::lookup_host((host.as_str(), port)),
                    )
                    .await
                    {
                        Ok(Ok(mut it)) => match it.next() {
                            Some(sa) => sa.ip(),
                            None => {
                                debug!(%host, "no DNS records for STUN host");
                                continue;
                            }
                        },
                        Ok(Err(e)) => {
                            debug!(%host, %e, "STUN host DNS lookup failed");
                            continue;
                        }
                        Err(_) => {
                            debug!(%host, "STUN host DNS lookup timed out");
                            continue;
                        }
                    }
                };

                let tx = TxId::random();
                targets.push((region.region_id, SocketAddr::new(ip, port), tx));
            }
        }

        if targets.is_empty() {
            return report;
        }

        // Send all STUN binding requests.
        let send_time = tokio::time::Instant::now();
        for &(_, addr, ref tx) in &targets {
            let pkt = stun::request(tx);
            match socket.send_to(&pkt, addr).await {
                Ok(_) => trace!(%addr, "sent STUN probe"),
                Err(e) => debug!(%addr, %e, "failed to send STUN probe"),
            }
        }

        // Build tx_id -> region_id lookup.
        let tx_map: HashMap<[u8; 12], u16> = targets
            .iter()
            .map(|(rid, _, tx)| (tx.0, *rid))
            .collect();

        // Collect responses until overall timeout.
        let mut buf = [0u8; 1500];
        let mut seen_ports: Vec<u16> = Vec::new();
        let deadline = send_time + self.overall_timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
                Ok(Ok((n, _src))) => {
                    let data = &buf[..n];
                    if let Some((tx, addr)) = stun::parse_response(data) {
                        self.handle_response(&tx, addr, &tx_map, send_time, &mut report, &mut seen_ports);
                    }
                }
                Ok(Err(e)) => {
                    debug!(%e, "recv error during netcheck");
                    break;
                }
                Err(_) => {
                    // Timeout
                    break;
                }
            }
        }

        // Determine mapping_varies from collected ports.
        if seen_ports.len() >= 2 {
            let all_same = seen_ports.windows(2).all(|w| w[0] == w[1]);
            report.mapping_varies = Some(!all_same);
        }

        report
    }

    fn handle_response(
        &self,
        tx: &TxId,
        addr: SocketAddr,
        tx_map: &HashMap<[u8; 12], u16>,
        send_time: tokio::time::Instant,
        report: &mut Report,
        seen_ports: &mut Vec<u16>,
    ) {
        let Some(&region_id) = tx_map.get(&tx.0) else {
            debug!("received STUN response with unknown tx_id");
            return;
        };

        let latency = send_time.elapsed();
        debug!(region_id, %addr, ?latency, "STUN response");

        report.udp = true;
        report.region_latency.insert(region_id, latency);

        match addr {
            SocketAddr::V4(_) => {
                report.ipv4 = true;
                report.global_v4 = Some(addr);
            }
            SocketAddr::V6(_) => {
                report.ipv6 = true;
                report.global_v6 = Some(addr);
            }
        }

        seen_ports.push(addr.port());

        // Update preferred_derp to the region with lowest latency.
        if report.preferred_derp == 0
            || latency
                < report
                    .region_latency
                    .get(&report.preferred_derp)
                    .copied()
                    .unwrap_or(Duration::MAX)
        {
            report.preferred_derp = region_id;
        }
    }

    /// Run a STUN check using a multiplexed socket. Sends probes via `socket`
    /// and reads responses from `response_rx` (a broadcast channel fed by the
    /// packet classifier). Multiple callers can subscribe concurrently.
    pub async fn report_mux(
        &self,
        derp_map: &DerpMap,
        socket: &UdpSocket,
        response_rx: &mut tokio::sync::broadcast::Receiver<(SocketAddr, Vec<u8>)>,
    ) -> Report {
        let mut report = Report::default();

        let mut targets: Vec<(u16, SocketAddr, TxId)> = Vec::new();
        for region in derp_map.regions.values() {
            if let Some(node) = region.nodes.iter().find(|n| n.stun_port > 0) {
                let port = node.stun_port as u16;
                let resolved = crate::derp::manager::apply_host_override(&node.host_name);
                let host = &resolved;
                let ip = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                    ip
                } else {
                    match tokio::time::timeout(
                        Duration::from_secs(2),
                        tokio::net::lookup_host((host.as_str(), port)),
                    )
                    .await
                    {
                        Ok(Ok(mut it)) => match it.next() {
                            Some(sa) => sa.ip(),
                            None => {
                                debug!(%host, "no DNS records for STUN host");
                                continue;
                            }
                        },
                        _ => {
                            debug!(%host, "STUN host DNS lookup failed");
                            continue;
                        }
                    }
                };
                targets.push((region.region_id, SocketAddr::new(ip, port), TxId::random()));
            }
        }

        if targets.is_empty() {
            return report;
        }

        let send_time = tokio::time::Instant::now();
        for &(_, addr, ref tx) in &targets {
            let pkt = stun::request(tx);
            match socket.send_to(&pkt, addr).await {
                Ok(_) => trace!(%addr, "sent STUN probe (mux)"),
                Err(e) => debug!(%addr, %e, "failed to send STUN probe"),
            }
        }

        let tx_map: HashMap<[u8; 12], u16> = targets
            .iter()
            .map(|(rid, _, tx)| (tx.0, *rid))
            .collect();

        let mut seen_ports: Vec<u16> = Vec::new();
        let deadline = send_time + self.overall_timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, response_rx.recv()).await {
                Ok(Ok((_src, data))) => {
                    if let Some((tx, addr)) = stun::parse_response(&data) {
                        self.handle_response(&tx, addr, &tx_map, send_time, &mut report, &mut seen_ports);
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                _ => break,
            }
        }

        if seen_ports.len() >= 2 {
            let all_same = seen_ports.windows(2).all(|w| w[0] == w[1]);
            report.mapping_varies = Some(!all_same);
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_no_servers() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let derp_map = DerpMap::default();
            let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let client = Client::new();
            let report = client.report(&derp_map, &socket).await;
            assert!(!report.udp);
            assert!(!report.ipv4);
            assert!(!report.ipv6);
            assert_eq!(report.preferred_derp, 0);
            assert!(report.region_latency.is_empty());
        });
    }

    #[test]
    fn test_client_default_timeouts() {
        let client = Client::new();
        assert_eq!(client.probe_timeout, Duration::from_secs(3));
        assert_eq!(client.overall_timeout, Duration::from_secs(5));
    }
}
