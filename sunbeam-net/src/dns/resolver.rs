//! Cluster DNS resolver that sends TCP queries through the VPN engine.
//!
//! The resolver has two collaborators:
//!
//! * a **DNS server address** — usually the cluster CoreDNS service IP
//!   (10.43.0.10:53 on k3s, 10.96.0.10:53 on kubeadm); this lives inside
//!   the tailnet and is only reachable over the tunnel.
//! * an **engine command channel** — the same
//!   [`crate::proxy::engine::EngineCommand::NewConnection`] channel the
//!   SOCKS5 proxy uses. We borrow it to ask the engine to open a virtual
//!   TCP socket to `dns_server:53`, bridging one side of a local TCP
//!   pair through smoltcp + WireGuard. The resolver holds the other
//!   side and speaks the DNS-over-TCP length-prefixed framing on it.
//!
//! Every `resolve()` call issues an A and an AAAA query concurrently
//! and returns the first usable answer, preferring IPv6 when both
//! succeed. The dual-stack path is unconditional: the cluster is about
//! to move to dual-stack, and once that lands any name may have only
//! an AAAA record, only an A record, or both.
//!
//! Results are cached by name with the TTL from the CoreDNS response
//! clamped to a sane minimum/maximum. Negative caching is deliberately
//! short so a service that just came up is reachable promptly; SOCKS5
//! CONNECTs that fail due to a transient NXDOMAIN will re-resolve
//! within `NEGATIVE_TTL`.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::codec::{DnsError, IpRecord, TYPE_A, TYPE_AAAA, decode_first_ip_record, encode_query};
use crate::proxy::engine::EngineCommand;

/// Maximum DNS response size we'll accept (RFC 1035 legacy UDP limit
/// is 512; EDNS0 bumps it to 4096; over TCP it's bounded by the 16-bit
/// length prefix, so 64KiB in theory. We cap at 4KiB — anything larger
/// is either broken or adversarial for our use case).
const MAX_RESPONSE_BYTES: usize = 4096;

/// Hard floor on cache TTL. CoreDNS sometimes returns `ttl=0` for
/// pod records during a rollout; if we honored that literally we'd
/// hammer DNS on every single CONNECT. 5 seconds gives enough of a
/// cushion for bursty CLI tools without masking real churn.
const MIN_POSITIVE_TTL: Duration = Duration::from_secs(5);

/// Hard cap on cache TTL. Upstream can set absurdly high TTLs; we
/// don't want stale entries hanging around for hours when an IP
/// flip happens.
const MAX_POSITIVE_TTL: Duration = Duration::from_secs(300);

/// How long to remember "this name doesn't resolve" before asking
/// again. Short on purpose — see module doc.
const NEGATIVE_TTL: Duration = Duration::from_secs(2);

/// Time budget for a single DNS query (connect + write + read).
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Errors surfaced by the resolver.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[allow(dead_code)]
    #[error("resolver not configured (no dns_server)")]
    /// Disabled.
    Disabled,
    #[error("name is empty")]
    /// Emptyname.
    EmptyName,
    #[error("engine command channel closed")]
    /// Engineclosed.
    EngineClosed,
    #[error("local TCP pair setup: {0}")]
    /// Localpair.
    LocalPair(std::io::Error),
    #[error("DNS I/O: {0}")]
    /// Io.
    Io(std::io::Error),
    #[error("DNS query timed out")]
    /// Timeout.
    Timeout,
    #[error("DNS response too large ({0} > {MAX_RESPONSE_BYTES})")]
    /// Responsetoolarge.
    ResponseTooLarge(usize),
    #[error("DNS codec: {0}")]
    /// Codec.
    Codec(#[from] DnsError),
    #[error("negative cache hit")]
    /// Negativecached.
    NegativeCached,
}

/// Cluster DNS resolver.
#[derive(Debug)]
pub struct Resolver {
    dns_server: SocketAddr,
    engine_tx: mpsc::Sender<EngineCommand>,
    search_domains: Vec<String>,
    cache: RwLock<HashMap<String, CacheEntry>>,
    /// Monotonically increasing transaction id. We don't bother
    /// randomizing — the response goes over a private TCP socket
    /// that only we wrote a query to, so id guessing isn't a
    /// meaningful threat here.
    next_id: std::sync::atomic::AtomicU16,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    result: Result<IpAddr, ()>,
    expires_at: Instant,
}

impl Resolver {
    /// Create a new resolver.
    ///
    /// `search_domains` are appended (in order) to names that don't
    /// contain a dot. Pass `["svc.cluster.local", "cluster.local"]`
    /// to let users type `hydra` and have it become
    /// `hydra.svc.cluster.local`.
    pub fn new(
        dns_server: SocketAddr,
        engine_tx: mpsc::Sender<EngineCommand>,
        search_domains: Vec<String>,
    ) -> Self {
        Self {
            dns_server,
            engine_tx,
            search_domains,
            cache: RwLock::new(HashMap::new()),
            next_id: std::sync::atomic::AtomicU16::new(1),
        }
    }

    /// Resolve a name to an IP address (IPv6 preferred), consulting
    /// the cache first.
    ///
    /// Tries the bare name first, then each search domain, and
    /// returns the first positive answer. Negative cache entries
    /// short-circuit a full re-query for a few seconds.
    pub async fn resolve(&self, name: &str) -> Result<IpAddr, ResolveError> {
        if name.is_empty() {
            return Err(ResolveError::EmptyName);
        }

        if let Some(cached) = self.cache_get(name) {
            return cached;
        }

        let candidates = self.build_candidates(name);
        let mut last_err: Option<ResolveError> = None;
        for candidate in &candidates {
            match self.query_dual_stack(candidate).await {
                Ok(rec) => {
                    self.cache_set_positive(name, rec);
                    if candidate != name {
                        self.cache_set_positive(candidate, rec);
                    }
                    return Ok(rec.addr);
                }
                Err(e) => last_err = Some(e),
            }
        }

        self.cache_set_negative(name);
        Err(last_err.unwrap_or(ResolveError::EmptyName))
    }

    /// Build the sequence of names to try. Always starts with the
    /// verbatim name; if the name has no dots, each search domain
    /// is appended to it in order.
    fn build_candidates(&self, name: &str) -> Vec<String> {
        let mut out = vec![name.to_string()];
        if !name.contains('.') {
            for sd in &self.search_domains {
                out.push(format!("{name}.{sd}"));
            }
        }
        out
    }

    fn cache_get(&self, name: &str) -> Option<Result<IpAddr, ResolveError>> {
        let cache = self.cache.read().ok()?;
        let entry = cache.get(name)?;
        if entry.expires_at <= Instant::now() {
            return None;
        }
        Some(match entry.result {
            Ok(ip) => Ok(ip),
            Err(()) => Err(ResolveError::NegativeCached),
        })
    }

    fn cache_set_positive(&self, name: &str, rec: IpRecord) {
        let ttl = Duration::from_secs(rec.ttl as u64)
            .max(MIN_POSITIVE_TTL)
            .min(MAX_POSITIVE_TTL);
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(
                name.to_string(),
                CacheEntry {
                    result: Ok(rec.addr),
                    expires_at: Instant::now() + ttl,
                },
            );
        }
    }

    fn cache_set_negative(&self, name: &str) {
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(
                name.to_string(),
                CacheEntry {
                    result: Err(()),
                    expires_at: Instant::now() + NEGATIVE_TTL,
                },
            );
        }
    }

    /// Purge all cache entries. Called when the VPN reconnects and
    /// netmap-learned peer IPs may have changed.
    #[allow(dead_code)]
    pub fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }

    /// Issue A and AAAA queries for `name` concurrently. Prefers
    /// IPv6 when both succeed; falls back to IPv4 otherwise. If
    /// both queries fail, returns the AAAA error (or the A error
    /// when AAAA wasn't itself informative).
    async fn query_dual_stack(&self, name: &str) -> Result<IpRecord, ResolveError> {
        let (aaaa, a) = tokio::join!(
            self.query_once(name, TYPE_AAAA),
            self.query_once(name, TYPE_A),
        );
        match (aaaa, a) {
            (Ok(rec), _) => Ok(rec),
            (Err(_), Ok(rec)) => Ok(rec),
            (Err(e6), Err(_)) => Err(e6),
        }
    }

    /// Issue a single DNS query of the given record type over a
    /// fresh virtual TCP socket.
    async fn query_once(&self, name: &str, qtype: u16) -> Result<IpRecord, ResolveError> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let query = encode_query(id, name, qtype)?;

        // Open a local TCP pair. The engine expects a real TcpStream
        // on the local side (because it uses try_read/try_write
        // directly), so we bind a loopback listener, connect to
        // ourselves, and give the engine the accept-side stream.
        let (client, server) = local_tcp_pair().await.map_err(ResolveError::LocalPair)?;

        self.engine_tx
            .send(EngineCommand::NewConnection {
                local: server,
                remote: self.dns_server,
            })
            .await
            .map_err(|_| ResolveError::EngineClosed)?;

        let fut = async move {
            let mut client = client;
            let len = query.len() as u16;
            client
                .write_all(&len.to_be_bytes())
                .await
                .map_err(ResolveError::Io)?;
            client.write_all(&query).await.map_err(ResolveError::Io)?;
            client.flush().await.map_err(ResolveError::Io)?;

            let mut len_buf = [0u8; 2];
            client
                .read_exact(&mut len_buf)
                .await
                .map_err(ResolveError::Io)?;
            let resp_len = u16::from_be_bytes(len_buf) as usize;
            if resp_len > MAX_RESPONSE_BYTES {
                return Err(ResolveError::ResponseTooLarge(resp_len));
            }
            let mut resp = vec![0u8; resp_len];
            client
                .read_exact(&mut resp)
                .await
                .map_err(ResolveError::Io)?;

            Ok::<_, ResolveError>(decode_first_ip_record(&resp, id, qtype)?)
        };

        tokio::time::timeout(QUERY_TIMEOUT, fut)
            .await
            .map_err(|_| ResolveError::Timeout)?
    }
}

/// Create a pair of connected TcpStreams on the loopback interface.
/// The engine accepts `TcpStream` on the proxy path, so the simplest
/// way to get a stream we can talk to the engine through is to bind
/// a tiny local listener and make a self-connection.
async fn local_tcp_pair() -> Result<(TcpStream, TcpStream), std::io::Error> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let addr = listener.local_addr()?;
    let (client_res, accept_res) = tokio::join!(TcpStream::connect(addr), listener.accept());
    let client = client_res?;
    let (server, _) = accept_res?;
    Ok((client, server))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    /// What a stub engine should play back for a given query type.
    #[derive(Debug, Clone, Copy)]
    enum StubAnswer {
        /// Successful answer.
        Ok { addr: IpAddr, ttl: u32 },
        /// NXDOMAIN (RCODE=3).
        Nx,
        /// No answer — queries with this type get a header-only reply
        /// with RCODE=0 and ancount=0, which the codec reports as
        /// `NoAnswer`.
        Empty,
    }

    /// Configuration for `run_stub_engine`: how to answer each query
    /// type. Anything not specified defaults to an empty answer.
    #[derive(Debug, Clone, Copy, Default)]
    struct StubPlan {
        a: Option<StubAnswer>,
        aaaa: Option<StubAnswer>,
    }

    /// A stub "engine" that, on every NewConnection request, accepts
    /// the server-side stream, reads the client's query, and plays
    /// back whatever the plan dictates for that query's type.
    async fn run_stub_engine(mut rx: mpsc::Receiver<EngineCommand>, plan: StubPlan) {
        while let Some(cmd) = rx.recv().await {
            let EngineCommand::NewConnection { mut local, .. } = cmd;
            tokio::spawn(async move {
                let mut len_buf = [0u8; 2];
                if local.read_exact(&mut len_buf).await.is_err() {
                    return;
                }
                let qlen = u16::from_be_bytes(len_buf) as usize;
                let mut query = vec![0u8; qlen];
                if local.read_exact(&mut query).await.is_err() {
                    return;
                }
                let id = u16::from_be_bytes([query[0], query[1]]);
                let qtype = extract_qtype(&query);

                let answer = match qtype {
                    TYPE_A => plan.a,
                    TYPE_AAAA => plan.aaaa,
                    _ => None,
                }
                .unwrap_or(StubAnswer::Empty);

                let resp = render_answer(id, &query, qtype, answer);
                let rlen = resp.len() as u16;
                let _ = local.write_all(&rlen.to_be_bytes()).await;
                let _ = local.write_all(&resp).await;
                let _ = local.flush().await;
            });
        }
    }

    /// Walk a query's question section to find the qtype field.
    fn extract_qtype(query: &[u8]) -> u16 {
        let q_start = 12;
        let mut offset = q_start;
        loop {
            let len = query[offset];
            if len == 0 {
                offset += 1;
                break;
            }
            offset += 1 + len as usize;
        }
        u16::from_be_bytes([query[offset], query[offset + 1]])
    }

    /// Extract the `[qname, qtype, qclass]` slice from a query.
    fn question_section(query: &[u8]) -> &[u8] {
        let q_start = 12;
        let mut offset = q_start;
        loop {
            let len = query[offset];
            if len == 0 {
                offset += 1;
                break;
            }
            offset += 1 + len as usize;
        }
        let q_end = offset + 4;
        &query[q_start..q_end]
    }

    /// Build a response. For `Ok` answers we append a compressed
    /// answer RR; for `Nx` we set RCODE=3; for `Empty` we return
    /// a header-only NOERROR reply.
    fn render_answer(id: u16, query: &[u8], qtype: u16, answer: StubAnswer) -> Vec<u8> {
        let question = question_section(query);
        let q_start: u16 = 12;

        let mut msg = Vec::new();
        msg.extend_from_slice(&id.to_be_bytes());

        match answer {
            StubAnswer::Ok { .. } => {
                msg.extend_from_slice(&0x8180u16.to_be_bytes());
                msg.extend_from_slice(&1u16.to_be_bytes()); // qd
                msg.extend_from_slice(&1u16.to_be_bytes()); // an
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(question);
            }
            StubAnswer::Nx => {
                msg.extend_from_slice(&0x8183u16.to_be_bytes()); // RCODE=3
                msg.extend_from_slice(&1u16.to_be_bytes());
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(question);
                return msg;
            }
            StubAnswer::Empty => {
                msg.extend_from_slice(&0x8180u16.to_be_bytes());
                msg.extend_from_slice(&1u16.to_be_bytes());
                msg.extend_from_slice(&0u16.to_be_bytes()); // ancount=0
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(&0u16.to_be_bytes());
                msg.extend_from_slice(question);
                return msg;
            }
        }

        let StubAnswer::Ok { addr, ttl } = answer else {
            unreachable!();
        };

        // Answer section — compression pointer to the qname.
        let ptr = (0xC000 | q_start).to_be_bytes();
        msg.extend_from_slice(&ptr);
        msg.extend_from_slice(&qtype.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // IN
        msg.extend_from_slice(&ttl.to_be_bytes());
        match addr {
            IpAddr::V4(v4) => {
                msg.extend_from_slice(&4u16.to_be_bytes());
                msg.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                msg.extend_from_slice(&16u16.to_be_bytes());
                msg.extend_from_slice(&v6.octets());
            }
        }
        msg
    }

    fn stub_dns_addr() -> SocketAddr {
        // The engine stub doesn't actually connect anywhere — it
        // just reads the server-side stream. So any placeholder
        // address works.
        SocketAddr::from(([10, 43, 0, 10], 53))
    }

    fn plan_a_only(addr: Ipv4Addr, ttl: u32) -> StubPlan {
        StubPlan {
            a: Some(StubAnswer::Ok {
                addr: IpAddr::V4(addr),
                ttl,
            }),
            aaaa: Some(StubAnswer::Empty),
        }
    }

    fn plan_aaaa_only(addr: Ipv6Addr, ttl: u32) -> StubPlan {
        StubPlan {
            a: Some(StubAnswer::Empty),
            aaaa: Some(StubAnswer::Ok {
                addr: IpAddr::V6(addr),
                ttl,
            }),
        }
    }

    fn plan_dual(v4: Ipv4Addr, v6: Ipv6Addr, ttl: u32) -> StubPlan {
        StubPlan {
            a: Some(StubAnswer::Ok {
                addr: IpAddr::V4(v4),
                ttl,
            }),
            aaaa: Some(StubAnswer::Ok {
                addr: IpAddr::V6(v6),
                ttl,
            }),
        }
    }

    #[tokio::test]
    async fn resolve_happy_path_v4_only() {
        let (tx, rx) = mpsc::channel(8);
        let answer = Ipv4Addr::new(10, 42, 5, 7);
        tokio::spawn(run_stub_engine(rx, plan_a_only(answer, 60)));

        let resolver = Resolver::new(stub_dns_addr(), tx, vec!["svc.cluster.local".into()]);
        let ip = resolver.resolve("hydra.ory").await.unwrap();
        assert_eq!(ip, IpAddr::V4(answer));
    }

    #[tokio::test]
    async fn resolve_happy_path_v6_only() {
        let (tx, rx) = mpsc::channel(8);
        let answer = Ipv6Addr::new(0xfd7a, 0x115c, 0xa1e0, 0xab12, 0, 0, 0, 1);
        tokio::spawn(run_stub_engine(rx, plan_aaaa_only(answer, 60)));

        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let ip = resolver
            .resolve("hydra.ory.svc.cluster.local")
            .await
            .unwrap();
        assert_eq!(ip, IpAddr::V6(answer));
    }

    #[tokio::test]
    async fn resolve_prefers_v6_when_both_available() {
        let (tx, rx) = mpsc::channel(8);
        let v4 = Ipv4Addr::new(10, 42, 0, 1);
        let v6 = Ipv6Addr::new(0xfd7a, 0, 0, 0, 0, 0, 0, 1);
        tokio::spawn(run_stub_engine(rx, plan_dual(v4, v6, 60)));

        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let ip = resolver.resolve("dual.example").await.unwrap();
        assert_eq!(ip, IpAddr::V6(v6));
    }

    #[tokio::test]
    async fn resolve_uses_cache_second_time() {
        let (tx, rx) = mpsc::channel(8);
        let answer = Ipv4Addr::new(10, 42, 0, 1);
        tokio::spawn(run_stub_engine(rx, plan_a_only(answer, 60)));

        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let first = resolver.resolve("foo.example").await.unwrap();
        assert_eq!(first, IpAddr::V4(answer));
        let second = resolver.resolve("foo.example").await.unwrap();
        assert_eq!(second, IpAddr::V4(answer));
    }

    #[tokio::test]
    async fn resolve_search_domain_appended_for_bare_name() {
        let (tx, rx) = mpsc::channel(8);
        let answer = Ipv4Addr::new(10, 42, 9, 9);
        tokio::spawn(run_stub_engine(rx, plan_a_only(answer, 60)));

        let resolver = Resolver::new(
            stub_dns_addr(),
            tx,
            vec!["svc.cluster.local".into(), "cluster.local".into()],
        );
        let ip = resolver.resolve("hydra").await.unwrap();
        assert_eq!(ip, IpAddr::V4(answer));
    }

    #[tokio::test]
    async fn resolve_rejects_empty_name() {
        let (tx, _rx) = mpsc::channel(4);
        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let err = resolver.resolve("").await.unwrap_err();
        assert!(matches!(err, ResolveError::EmptyName));
    }

    #[tokio::test]
    async fn resolve_timeouts_when_engine_stalls() {
        // Engine never responds — just hold every incoming stream
        // open without reading or writing.
        let (tx, mut rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Some(cmd) = rx.recv().await {
                let EngineCommand::NewConnection { local, .. } = cmd;
                held.push(local);
            }
        });

        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let start = Instant::now();
        let err = resolver.resolve("foo.bar").await.unwrap_err();
        assert!(matches!(err, ResolveError::Timeout));
        assert!(start.elapsed() >= QUERY_TIMEOUT);
    }

    #[tokio::test]
    async fn resolve_caches_negative_result_after_failure() {
        // Stub engine that answers with NXDOMAIN for both A and AAAA.
        let (tx, rx) = mpsc::channel(8);
        let plan = StubPlan {
            a: Some(StubAnswer::Nx),
            aaaa: Some(StubAnswer::Nx),
        };
        tokio::spawn(run_stub_engine(rx, plan));

        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let e1 = resolver.resolve("ghost.nowhere").await.unwrap_err();
        assert!(matches!(e1, ResolveError::Codec(DnsError::Rcode(3))));
        let e2 = resolver.resolve("ghost.nowhere").await.unwrap_err();
        assert!(matches!(e2, ResolveError::NegativeCached));
    }

    #[tokio::test]
    async fn invalidate_cache_forces_refresh() {
        let (tx, rx) = mpsc::channel(8);
        let answer = Ipv4Addr::new(10, 42, 1, 1);
        tokio::spawn(run_stub_engine(rx, plan_a_only(answer, 60)));

        let resolver = Resolver::new(stub_dns_addr(), tx, vec![]);
        let first = resolver.resolve("foo.bar").await.unwrap();
        assert_eq!(first, IpAddr::V4(answer));
        resolver.invalidate_cache();
        let second = resolver.resolve("foo.bar").await.unwrap();
        assert_eq!(second, IpAddr::V4(answer));
    }

    #[tokio::test]
    async fn local_tcp_pair_is_connected() {
        let (mut client, mut server) = local_tcp_pair().await.unwrap();
        client.write_all(b"ping").await.unwrap();
        client.flush().await.unwrap();
        let mut buf = [0u8; 4];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[test]
    fn build_candidates_bare_name_gets_search_domains() {
        let (tx, _rx) = mpsc::channel(4);
        let r = Resolver::new(
            stub_dns_addr(),
            tx,
            vec!["svc.cluster.local".into(), "cluster.local".into()],
        );
        let c = r.build_candidates("hydra");
        assert_eq!(
            c,
            vec!["hydra", "hydra.svc.cluster.local", "hydra.cluster.local"]
        );
    }

    #[test]
    fn build_candidates_dotted_name_unchanged() {
        let (tx, _rx) = mpsc::channel(4);
        let r = Resolver::new(stub_dns_addr(), tx, vec!["svc.cluster.local".into()]);
        let c = r.build_candidates("foo.bar");
        assert_eq!(c, vec!["foo.bar"]);
    }

    #[test]
    fn cache_entry_respects_min_ttl() {
        let (tx, _rx) = mpsc::channel(4);
        let r = Resolver::new(stub_dns_addr(), tx, vec![]);
        r.cache_set_positive(
            "foo",
            IpRecord {
                addr: IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
                ttl: 0,
            },
        );
        let cache = r.cache.read().unwrap();
        let entry = cache.get("foo").unwrap();
        assert!(entry.expires_at > Instant::now() + Duration::from_millis(100));
    }

    #[test]
    fn cache_entry_respects_max_ttl() {
        let (tx, _rx) = mpsc::channel(4);
        let r = Resolver::new(stub_dns_addr(), tx, vec![]);
        r.cache_set_positive(
            "foo",
            IpRecord {
                addr: IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
                ttl: 999_999,
            },
        );
        let cache = r.cache.read().unwrap();
        let entry = cache.get("foo").unwrap();
        // Must not exceed MAX_POSITIVE_TTL (+ a little schedule slop).
        assert!(entry.expires_at <= Instant::now() + MAX_POSITIVE_TTL + Duration::from_secs(1));
    }
}
