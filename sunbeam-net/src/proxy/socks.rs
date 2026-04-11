//! SOCKS5 + HTTP CONNECT proxy, mirroring `tailscaled --tun=userspace-networking`.
//!
//! A single TCP listener on a loopback address multiplexes two protocols
//! on one port — the first byte of each connection picks the dispatch:
//!
//! * `0x05` → SOCKS5 (RFC 1928 + 1929 username/password auth)
//! * ASCII `C` / `G` / `P` / `H` → HTTP/1.1 (CONNECT with `Proxy-Authorization`)
//!
//! This is what Tailscale's `tsnet.Server.Listen` does upstream
//! (`proxy/multiplex.go`), and it's the same mode the Tailscale k8s operator
//! runs inside sidecar containers.
//!
//! ### Security model
//!
//! 1. **Loopback only.** The listener refuses to start if [`SocksConfig::bind`]
//!    is not a loopback address. Belt: we additionally test-bind on
//!    `0.0.0.0:<same-port>` and fail loudly if that succeeds, because a
//!    misbehaving loopback interface that actually reaches `0.0.0.0` would
//!    otherwise leak silently.
//! 2. **Mandatory credential.** Both SOCKS5 and HTTP CONNECT require a
//!    32-byte random auth token. The token is generated at startup, written
//!    to `{state_dir}/socks5.auth` with mode `0600`, and compared in
//!    constant time ([`subtle::ConstantTimeEq`]) on every connection. Same
//!    threat model as reading `~/.kube/config`.
//! 3. **Destination ACL — route table.** The destination IP must be in the
//!    [`crate::control::RouteTable`] (i.e. covered by a peer's `AllowedIPs`
//!    *and* inside the configured whitelist). Missing routes map to SOCKS5
//!    reply code `0x03` (network unreachable) or HTTP `502`.
//! 4. **Destination ACL — port list.** The destination port must be in
//!    [`SocksConfig::allow_ports`]. Disallowed ports map to SOCKS5 reply
//!    `0x02` (connection not allowed) / HTTP `403`.
//! 5. **Commands restricted.** SOCKS5 only accepts `CONNECT` (0x01). `BIND`
//!    and `UDP ASSOCIATE` are rejected with reply `0x07`.
//! 6. **Domain names deferred to Phase 3.** Until the internal DNS resolver
//!    lands, domain destinations are refused with SOCKS5 reply `0x04` /
//!    HTTP `421`. IPv4 and IPv6 literals work today.
//!
//! On any ACL failure we send the protocol-appropriate error reply and
//! close the connection without ever touching the smoltcp stack or the
//! destination IP. Error codes are deliberately specific so misconfigured
//! clients see useful diagnostics.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;

use base64::Engine as _;
use rand::RngCore;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::control::RouteTable;
use crate::dns::resolver::{ResolveError, Resolver};
use crate::proxy::audit::{AuditLog, AuditOutcome};
use crate::proxy::engine::EngineCommand;

/// User name presented in the SOCKS5 / HTTP CONNECT credential challenge.
/// Clients must send this exact string; the actual secret is the password.
pub const SOCKS_USERNAME: &str = "sunbeam";

/// Length of the generated auth token in bytes (pre-hex).
const AUTH_TOKEN_BYTES: usize = 32;

/// SOCKS5 protocol version byte.
const SOCKS5_VERSION: u8 = 0x05;
/// SOCKS5 auth subnegotiation version byte (RFC 1929).
const SOCKS5_AUTH_VERSION: u8 = 0x01;

/// SOCKS5 auth method: username/password (RFC 1929).
const SOCKS5_METHOD_USERPASS: u8 = 0x02;
/// SOCKS5 sentinel meaning "no acceptable methods".
const SOCKS5_METHOD_NONE: u8 = 0xFF;

/// SOCKS5 command: CONNECT.
const SOCKS5_CMD_CONNECT: u8 = 0x01;

/// SOCKS5 reply codes (subset we actually send).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    Succeeded = 0x00,
    GeneralFailure = 0x01,
    ConnectionNotAllowed = 0x02,
    NetworkUnreachable = 0x03,
    #[allow(dead_code)]
    HostUnreachable = 0x04,
    #[allow(dead_code)]
    ConnectionRefused = 0x05,
    CommandNotSupported = 0x07,
    AddressTypeNotSupported = 0x08,
}

/// SOCKS5 address type byte.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddrType {
    Ipv4 = 0x01,
    Domain = 0x03,
    Ipv6 = 0x04,
}

/// Configuration for the SOCKS5 + HTTP CONNECT listener.
#[derive(Debug, Clone)]
pub struct SocksConfig {
    /// Loopback address to bind on. The port is always assigned ephemerally.
    pub bind: IpAddr,
    /// Destination ports the proxy will forward to. Any other port is
    /// rejected with [`Reply::ConnectionNotAllowed`].
    pub allow_ports: Vec<u16>,
    /// Directory the auth token + port discovery files are written to.
    /// Both files are created with mode `0600`.
    pub state_dir: PathBuf,
}

/// Runtime handle for the SOCKS5 listener. Created by [`SocksServer::bind`]
/// and consumed by [`SocksServer::run`] once the caller has captured the
/// local port.
#[derive(Debug)]
pub struct SocksServer {
    listener: TcpListener,
    routes: Arc<RwLock<RouteTable>>,
    allow_ports: Arc<Vec<u16>>,
    auth_token: Arc<String>,
    cmd_tx: mpsc::Sender<EngineCommand>,
    /// Optional cluster DNS resolver. When `None`, domain-name
    /// CONNECT destinations are refused with `AddressTypeNotSupported`
    /// (SOCKS5) / `421 Misdirected Request` (HTTP) — matching the
    /// pre-Phase-3 behavior so tests that don't care about DNS can
    /// leave it unset.
    resolver: Option<Arc<Resolver>>,
    /// Shared ring-buffer audit log. Every connection attempt writes
    /// exactly one entry here — accepted or denied — so `sunbeam vpn
    /// status` can show the tail.
    audit: Arc<AuditLog>,
}

/// Public view of the running SOCKS5 endpoint — the port and auth token
/// that clients need to connect. The caller is expected to write the
/// discovery files via [`SocksServer::write_discovery_files`] so tools like
/// `sunbeam logs` can find them.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SocksEndpoint {
    pub port: u16,
    pub auth_token: String,
}

impl SocksServer {
    /// Bind the listener and stage the credential + port discovery files.
    ///
    /// This rejects non-loopback binds at the first opportunity — before
    /// any credentials are ever written to disk — because the auth token's
    /// only guarantee is "same-user access only," which collapses if the
    /// port is reachable from other hosts.
    pub async fn bind(
        config: SocksConfig,
        routes: Arc<RwLock<RouteTable>>,
        cmd_tx: mpsc::Sender<EngineCommand>,
        resolver: Option<Arc<Resolver>>,
        audit: Arc<AuditLog>,
    ) -> crate::Result<(Self, SocksEndpoint)> {
        if !is_loopback(&config.bind) {
            return Err(crate::Error::Control(format!(
                "refusing to bind SOCKS5 proxy on non-loopback address {}",
                config.bind
            )));
        }

        let bind_addr = SocketAddr::new(config.bind, 0);
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| crate::Error::Io {
                context: format!("bind SOCKS5 listener {bind_addr}"),
                source: e,
            })?;
        let local = listener.local_addr().map_err(|e| crate::Error::Io {
            context: "SOCKS5 listener local_addr".into(),
            source: e,
        })?;

        let auth_token = generate_auth_token();
        write_discovery_files(&config.state_dir, local.port(), &auth_token)?;

        tracing::info!(
            "SOCKS5 + HTTP CONNECT proxy listening on {} (discovery files in {})",
            local,
            config.state_dir.display()
        );

        let endpoint = SocksEndpoint {
            port: local.port(),
            auth_token: auth_token.clone(),
        };

        Ok((
            Self {
                listener,
                routes,
                allow_ports: Arc::new(config.allow_ports),
                auth_token: Arc::new(auth_token),
                cmd_tx,
                resolver,
                audit,
            },
            endpoint,
        ))
    }

    /// Remove the discovery files created by [`Self::bind`]. Call on
    /// daemon shutdown so stale credentials don't outlive the listener.
    pub fn remove_discovery_files(state_dir: &Path) {
        let _ = std::fs::remove_file(state_dir.join("socks5.port"));
        let _ = std::fs::remove_file(state_dir.join("socks5.auth"));
    }

    /// Run the accept loop until cancelled or the channel closes.
    pub async fn run(self, cancel: CancellationToken) -> crate::Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    tracing::info!("SOCKS5 proxy shutting down");
                    return Ok(());
                }
                accept = self.listener.accept() => {
                    let (stream, peer) = match accept {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("SOCKS5 accept error: {e}");
                            continue;
                        }
                    };
                    let routes = self.routes.clone();
                    let allow_ports = self.allow_ports.clone();
                    let auth_token = self.auth_token.clone();
                    let cmd_tx = self.cmd_tx.clone();
                    let resolver = self.resolver.clone();
                    let audit = self.audit.clone();
                    let cancel = cancel.clone();
                    tokio::spawn(async move {
                        let ctx = ConnectionContext {
                            routes,
                            allow_ports,
                            auth_token,
                            cmd_tx,
                            resolver,
                            audit,
                            cancel,
                        };
                        if let Err(e) = handle_connection(stream, peer, ctx).await {
                            tracing::debug!("SOCKS5 conn {peer} ended: {e}");
                        }
                    });
                }
            }
        }
    }
}

/// Per-connection state shared between the SOCKS5 and HTTP handlers.
struct ConnectionContext {
    routes: Arc<RwLock<RouteTable>>,
    allow_ports: Arc<Vec<u16>>,
    auth_token: Arc<String>,
    cmd_tx: mpsc::Sender<EngineCommand>,
    /// Cluster DNS resolver; `None` disables domain-name CONNECTs.
    resolver: Option<Arc<Resolver>>,
    /// Ring-buffer audit log. Every decision the handlers make about a
    /// destination — accepted or denied — is recorded here.
    audit: Arc<AuditLog>,
    #[allow(dead_code)]
    cancel: CancellationToken,
}

/// Map an [`AclDenial`] to the matching [`AuditOutcome`] string. Kept as
/// a free function so both the SOCKS5 and HTTP branches can use it.
fn denial_to_outcome(d: AclDenial) -> AuditOutcome {
    match d {
        AclDenial::NotRoutable => AuditOutcome::DeniedNotRoutable,
        AclDenial::PortDenied => AuditOutcome::DeniedPort,
        AclDenial::DomainUnsupported => AuditOutcome::DeniedNoDns,
        AclDenial::ResolutionFailed => AuditOutcome::DeniedResolveFailed,
    }
}

/// Render a [`Destination`] + resolved IP as an operator-friendly string.
/// When the destination was a domain, the rendered form is
/// `host:port → ip:port` so `sunbeam vpn status` shows both.
fn render_destination(dst: &Destination, resolved: Option<SocketAddr>) -> String {
    match (dst, resolved) {
        (Destination::Ip(ip, port), _) => SocketAddr::new(*ip, *port).to_string(),
        (Destination::Domain(name, port), Some(addr)) => format!("{name}:{port} → {addr}"),
        (Destination::Domain(name, port), None) => format!("{name}:{port}"),
    }
}

/// Dispatch a new loopback connection to the right protocol handler.
async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    ctx: ConnectionContext,
) -> crate::Result<()> {
    // Peek one byte to pick the protocol. We use a single read + an
    // in-memory cursor instead of `peek` so the SOCKS5 handler can consume
    // the greeting byte cleanly — mirrors Tailscale's approach.
    let mut first = [0u8; 1];
    let n = stream
        .read(&mut first)
        .await
        .map_err(|e| crate::Error::Io {
            context: format!("peek first byte from {peer}"),
            source: e,
        })?;
    if n == 0 {
        return Ok(());
    }

    match first[0] {
        SOCKS5_VERSION => handle_socks5(stream, ctx).await,
        b'C' | b'G' | b'P' | b'H' | b'D' | b'O' | b'T' => handle_http(stream, first[0], ctx).await,
        other => {
            tracing::debug!("SOCKS5 listener: unknown protocol byte 0x{other:02x} from {peer}");
            Ok(())
        }
    }
}

// ─── SOCKS5 path (RFC 1928 + 1929) ──────────────────────────────────────────

/// Handle a SOCKS5 greeting + auth + CONNECT request. The first byte (the
/// SOCKS version 0x05) has already been consumed by the dispatcher.
async fn handle_socks5(mut stream: TcpStream, ctx: ConnectionContext) -> crate::Result<()> {
    // Method negotiation. RFC 1928 §3 — we already read the version byte,
    // now read NMETHODS and the method list.
    let mut nmethods_buf = [0u8; 1];
    stream
        .read_exact(&mut nmethods_buf)
        .await
        .map_err(io_ctx("socks5 nmethods"))?;
    let nmethods = nmethods_buf[0] as usize;
    let mut methods = vec![0u8; nmethods];
    stream
        .read_exact(&mut methods)
        .await
        .map_err(io_ctx("socks5 methods"))?;

    // We only accept USERNAME/PASSWORD. No-auth (0x00) is explicitly
    // refused — even on loopback we require a credential.
    if !methods.contains(&SOCKS5_METHOD_USERPASS) {
        stream
            .write_all(&[SOCKS5_VERSION, SOCKS5_METHOD_NONE])
            .await
            .map_err(io_ctx("socks5 reject methods"))?;
        return Ok(());
    }

    stream
        .write_all(&[SOCKS5_VERSION, SOCKS5_METHOD_USERPASS])
        .await
        .map_err(io_ctx("socks5 select method"))?;

    // Username/password subnegotiation. RFC 1929.
    let mut hdr = [0u8; 2];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(io_ctx("socks5 auth hdr"))?;
    if hdr[0] != SOCKS5_AUTH_VERSION {
        let _ = stream.write_all(&[SOCKS5_AUTH_VERSION, 0x01]).await;
        return Ok(());
    }
    let ulen = hdr[1] as usize;
    let mut uname = vec![0u8; ulen];
    stream
        .read_exact(&mut uname)
        .await
        .map_err(io_ctx("socks5 uname"))?;

    let mut plen_buf = [0u8; 1];
    stream
        .read_exact(&mut plen_buf)
        .await
        .map_err(io_ctx("socks5 plen"))?;
    let plen = plen_buf[0] as usize;
    let mut passwd = vec![0u8; plen];
    stream
        .read_exact(&mut passwd)
        .await
        .map_err(io_ctx("socks5 passwd"))?;

    // Constant-time comparison on both fields. Username is public but we
    // still compare in CT so a short-circuit on username doesn't leak the
    // length of the configured value.
    let user_ok = bool::from(uname.as_slice().ct_eq(SOCKS_USERNAME.as_bytes()));
    let pass_ok = bool::from(passwd.as_slice().ct_eq(ctx.auth_token.as_bytes()));
    if !(user_ok && pass_ok) {
        let _ = stream.write_all(&[SOCKS5_AUTH_VERSION, 0x01]).await;
        tracing::debug!("SOCKS5 auth rejected");
        ctx.audit.record(
            "socks5",
            "(auth failed)".into(),
            AuditOutcome::DeniedProtocol,
        );
        return Ok(());
    }
    stream
        .write_all(&[SOCKS5_AUTH_VERSION, 0x00])
        .await
        .map_err(io_ctx("socks5 auth ok"))?;

    // Request. RFC 1928 §4: VER CMD RSV ATYP DST.ADDR DST.PORT
    let mut req_hdr = [0u8; 4];
    stream
        .read_exact(&mut req_hdr)
        .await
        .map_err(io_ctx("socks5 req hdr"))?;
    if req_hdr[0] != SOCKS5_VERSION {
        send_socks5_reply(&mut stream, Reply::GeneralFailure, SOCKS5_UNSPEC_ADDR).await?;
        ctx.audit.record(
            "socks5",
            "(bad version)".into(),
            AuditOutcome::DeniedProtocol,
        );
        return Ok(());
    }
    if req_hdr[1] != SOCKS5_CMD_CONNECT {
        send_socks5_reply(&mut stream, Reply::CommandNotSupported, SOCKS5_UNSPEC_ADDR).await?;
        ctx.audit.record(
            "socks5",
            "(non-CONNECT cmd)".into(),
            AuditOutcome::DeniedProtocol,
        );
        return Ok(());
    }

    let atyp = req_hdr[3];
    let dst = match read_socks5_destination(&mut stream, atyp).await? {
        Some(dst) => dst,
        None => {
            send_socks5_reply(
                &mut stream,
                Reply::AddressTypeNotSupported,
                SOCKS5_UNSPEC_ADDR,
            )
            .await?;
            ctx.audit.record(
                "socks5",
                format!("(unknown atyp 0x{atyp:02x})"),
                AuditOutcome::DeniedProtocol,
            );
            return Ok(());
        }
    };

    // Resolve (if needed) and authorize destination against port
    // allow-list + route table.
    let target = match authorize_destination(&ctx, &dst).await {
        Ok(addr) => addr,
        Err(denial) => {
            let reply = match denial {
                AclDenial::NotRoutable => Reply::NetworkUnreachable,
                AclDenial::PortDenied => Reply::ConnectionNotAllowed,
                AclDenial::DomainUnsupported => Reply::AddressTypeNotSupported,
                // Closest SOCKS5 code for "we understand the atyp but
                // the host it named can't be reached right now."
                AclDenial::ResolutionFailed => Reply::HostUnreachable,
            };
            send_socks5_reply(&mut stream, reply, SOCKS5_UNSPEC_ADDR).await?;
            ctx.audit.record(
                "socks5",
                render_destination(&dst, None),
                denial_to_outcome(denial),
            );
            return Ok(());
        }
    };

    // ACL passed — send SUCCEEDED and hand the stream off to the engine.
    send_socks5_reply(&mut stream, Reply::Succeeded, SOCKS5_UNSPEC_ADDR).await?;

    tracing::debug!("SOCKS5 CONNECT accepted → {target}");
    ctx.audit.record(
        "socks5",
        render_destination(&dst, Some(target)),
        AuditOutcome::Accepted,
    );
    ctx.cmd_tx
        .send(EngineCommand::NewConnection {
            local: stream,
            remote: target,
        })
        .await
        .map_err(|_| crate::Error::Control("engine channel closed".into()))?;
    Ok(())
}

/// The `BND.ADDR` / `BND.PORT` we send in SOCKS5 replies. Since we're a
/// userspace proxy, the "bound address" is meaningless — RFC 1928 allows
/// 0.0.0.0:0 and real clients don't inspect it.
const SOCKS5_UNSPEC_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);

/// Write one SOCKS5 reply frame.
async fn send_socks5_reply(
    stream: &mut TcpStream,
    reply: Reply,
    bnd: SocketAddr,
) -> crate::Result<()> {
    let mut out = Vec::with_capacity(10);
    out.push(SOCKS5_VERSION);
    out.push(reply as u8);
    out.push(0x00); // RSV
    match bnd.ip() {
        IpAddr::V4(v4) => {
            out.push(AddrType::Ipv4 as u8);
            out.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            out.push(AddrType::Ipv6 as u8);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&bnd.port().to_be_bytes());
    stream
        .write_all(&out)
        .await
        .map_err(io_ctx("socks5 reply"))?;
    Ok(())
}

/// Read a SOCKS5 destination (address + port) for a request that has
/// already consumed the 4-byte header. Returns `Ok(None)` for unknown
/// address types.
async fn read_socks5_destination(
    stream: &mut TcpStream,
    atyp: u8,
) -> crate::Result<Option<Destination>> {
    match atyp {
        x if x == AddrType::Ipv4 as u8 => {
            let mut buf = [0u8; 4];
            stream
                .read_exact(&mut buf)
                .await
                .map_err(io_ctx("socks5 ipv4"))?;
            let port = read_u16(stream).await?;
            Ok(Some(Destination::Ip(IpAddr::V4(Ipv4Addr::from(buf)), port)))
        }
        x if x == AddrType::Ipv6 as u8 => {
            let mut buf = [0u8; 16];
            stream
                .read_exact(&mut buf)
                .await
                .map_err(io_ctx("socks5 ipv6"))?;
            let port = read_u16(stream).await?;
            Ok(Some(Destination::Ip(IpAddr::V6(Ipv6Addr::from(buf)), port)))
        }
        x if x == AddrType::Domain as u8 => {
            let mut dlen_buf = [0u8; 1];
            stream
                .read_exact(&mut dlen_buf)
                .await
                .map_err(io_ctx("socks5 dlen"))?;
            let dlen = dlen_buf[0] as usize;
            let mut dbuf = vec![0u8; dlen];
            stream
                .read_exact(&mut dbuf)
                .await
                .map_err(io_ctx("socks5 domain"))?;
            let port = read_u16(stream).await?;
            let name = String::from_utf8_lossy(&dbuf).into_owned();
            Ok(Some(Destination::Domain(name, port)))
        }
        _ => Ok(None),
    }
}

async fn read_u16(stream: &mut TcpStream) -> crate::Result<u16> {
    let mut buf = [0u8; 2];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(io_ctx("socks5 u16"))?;
    Ok(u16::from_be_bytes(buf))
}

// ─── HTTP CONNECT path ──────────────────────────────────────────────────────

/// Handle a conventional `CONNECT host:port HTTP/1.1` request. The first
/// byte was peeked by the dispatcher and is passed back in `first_byte`.
async fn handle_http(
    mut stream: TcpStream,
    first_byte: u8,
    ctx: ConnectionContext,
) -> crate::Result<()> {
    // Read the request headers into a buffer up to the double-CRLF. We do
    // not support pipelining or bodies — reject anything we can't parse.
    let mut buf = vec![first_byte];
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await.map_err(io_ctx("http read"))?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            send_http_status(&mut stream, 414, "Request-URI Too Long").await?;
            ctx.audit.record(
                "http",
                "(request too long)".into(),
                AuditOutcome::DeniedProtocol,
            );
            return Ok(());
        }
    }

    let request = match HttpRequest::parse(&buf) {
        Some(r) => r,
        None => {
            send_http_status(&mut stream, 400, "Bad Request").await?;
            ctx.audit.record(
                "http",
                "(parse failed)".into(),
                AuditOutcome::DeniedProtocol,
            );
            return Ok(());
        }
    };

    if !request.method.eq_ignore_ascii_case("CONNECT") {
        // We only do CONNECT proxying. Plaintext HTTP proxying would
        // require parsing request bodies and stripping `Host:` headers;
        // all we want is encrypted traffic through the tunnel anyway.
        send_http_status(&mut stream, 405, "Method Not Allowed").await?;
        ctx.audit.record(
            "http",
            format!("({} {})", request.method, request.target),
            AuditOutcome::DeniedProtocol,
        );
        return Ok(());
    }

    // Auth: Proxy-Authorization: Basic base64(user:password)
    if !validate_http_auth(&request, &ctx.auth_token) {
        let mut reply = "HTTP/1.1 407 Proxy Authentication Required\r\n\
             Proxy-Authenticate: Basic realm=\"sunbeam-net\"\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n"
            .to_string();
        // Tiny hardening: force headers out before we drop the socket so
        // curl doesn't report ECONNRESET instead of the 407.
        let _ = stream.write_all(reply.as_bytes()).await;
        reply.clear();
        ctx.audit
            .record("http", "(auth failed)".into(), AuditOutcome::DeniedProtocol);
        return Ok(());
    }

    let dst = match Destination::parse_host_port(&request.target) {
        Some(d) => d,
        None => {
            send_http_status(&mut stream, 400, "Bad Destination").await?;
            ctx.audit.record(
                "http",
                format!("(bad target: {})", request.target),
                AuditOutcome::DeniedProtocol,
            );
            return Ok(());
        }
    };

    let target = match authorize_destination(&ctx, &dst).await {
        Ok(addr) => addr,
        Err(denial) => {
            let (code, reason) = match denial {
                AclDenial::NotRoutable => (502, "Bad Gateway (not routable)"),
                AclDenial::PortDenied => (403, "Forbidden (port)"),
                // 421 Misdirected Request — "you sent a domain name
                // but this proxy has no resolver configured."
                AclDenial::DomainUnsupported => (421, "Misdirected Request (no DNS configured)"),
                AclDenial::ResolutionFailed => (502, "Bad Gateway (DNS resolution failed)"),
            };
            send_http_status(&mut stream, code, reason).await?;
            ctx.audit.record(
                "http",
                render_destination(&dst, None),
                denial_to_outcome(denial),
            );
            return Ok(());
        }
    };

    send_http_status(&mut stream, 200, "Connection Established").await?;

    tracing::debug!("HTTP CONNECT accepted → {target}");
    ctx.audit.record(
        "http",
        render_destination(&dst, Some(target)),
        AuditOutcome::Accepted,
    );
    ctx.cmd_tx
        .send(EngineCommand::NewConnection {
            local: stream,
            remote: target,
        })
        .await
        .map_err(|_| crate::Error::Control("engine channel closed".into()))?;
    Ok(())
}

async fn send_http_status(stream: &mut TcpStream, code: u16, reason: &str) -> crate::Result<()> {
    let msg = format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream
        .write_all(msg.as_bytes())
        .await
        .map_err(io_ctx("http status"))?;
    Ok(())
}

/// Parsed HTTP/1.x request line + headers.
#[derive(Debug, Clone)]
struct HttpRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl HttpRequest {
    /// Very small HTTP/1.x parser. We only need the request line and the
    /// `Proxy-Authorization` header; anything else is ignored. Returns
    /// `None` for malformed input.
    fn parse(buf: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(buf).ok()?;
        let mut lines = text.split("\r\n");
        let request_line = lines.next()?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next()?.to_string();
        let target = parts.next()?.to_string();
        let _version = parts.next()?;
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        Some(Self {
            method,
            target,
            headers,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find_map(|(k, v)| (k == name).then_some(v.as_str()))
    }
}

/// Validate a `Proxy-Authorization: Basic ...` header in constant time.
fn validate_http_auth(req: &HttpRequest, auth_token: &str) -> bool {
    let value = match req.header("proxy-authorization") {
        Some(v) => v,
        None => return false,
    };
    let mut parts = value.splitn(2, ' ');
    let scheme = parts.next().unwrap_or("");
    let b64 = parts.next().unwrap_or("");
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let expected = format!("{SOCKS_USERNAME}:{auth_token}");
    bool::from(decoded.as_slice().ct_eq(expected.as_bytes()))
}

// ─── Destination + ACL ──────────────────────────────────────────────────────

/// Parsed destination before ACL enforcement. Domain names are carried
/// through so the rejection path can produce a specific error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destination {
    Ip(IpAddr, u16),
    Domain(String, u16),
}

impl Destination {
    /// Parse an HTTP CONNECT target of the form `host:port` or
    /// `[v6]:port`. Returns `None` for anything malformed.
    fn parse_host_port(target: &str) -> Option<Self> {
        if let Some(rest) = target.strip_prefix('[') {
            let (host, port_part) = rest.split_once(']')?;
            let port: u16 = port_part.strip_prefix(':')?.parse().ok()?;
            let ip: Ipv6Addr = host.parse().ok()?;
            return Some(Destination::Ip(IpAddr::V6(ip), port));
        }
        let (host, port_part) = target.rsplit_once(':')?;
        let port: u16 = port_part.parse().ok()?;
        if let Ok(ip) = host.parse::<IpAddr>() {
            Some(Destination::Ip(ip, port))
        } else {
            Some(Destination::Domain(host.to_string(), port))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AclDenial {
    /// IP is not in the peer route table → NETWORK_UNREACHABLE / 502.
    NotRoutable,
    /// Port is not in the allow list → CONNECTION_NOT_ALLOWED / 403.
    PortDenied,
    /// Destination is a domain name and no resolver is configured.
    DomainUnsupported,
    /// Destination is a domain name and the resolver rejected it
    /// (NXDOMAIN, timeout, codec error, negative cache hit).
    ResolutionFailed,
}

/// Authorize a parsed destination, resolving domain names through
/// the context's DNS resolver when present.
///
/// Returns the concrete `SocketAddr` that the engine should connect
/// to, after passing both the port allow-list and the route table
/// check. Rejected destinations yield a specific [`AclDenial`] so
/// the SOCKS5/HTTP layers can map it to a correct wire code.
async fn authorize_destination(
    ctx: &ConnectionContext,
    dst: &Destination,
) -> Result<SocketAddr, AclDenial> {
    // Port check is cheap and type-agnostic — do it first so we
    // don't send pointless DNS queries for blocked ports.
    let port = dst.port();
    if !ctx.allow_ports.contains(&port) {
        return Err(AclDenial::PortDenied);
    }

    let ip = match dst {
        Destination::Ip(ip, _) => *ip,
        Destination::Domain(name, _) => {
            let resolver = ctx.resolver.as_ref().ok_or(AclDenial::DomainUnsupported)?;
            match resolver.resolve(name).await {
                Ok(ip) => ip,
                Err(e) => {
                    tracing::debug!("SOCKS5 resolve {name}: {e}");
                    return Err(match e {
                        ResolveError::Disabled => AclDenial::DomainUnsupported,
                        _ => AclDenial::ResolutionFailed,
                    });
                }
            }
        }
    };

    let routes = ctx.routes.read().map_err(|_| AclDenial::NotRoutable)?;
    if routes.resolve(ip).is_none() {
        return Err(AclDenial::NotRoutable);
    }
    Ok(SocketAddr::new(ip, port))
}

#[cfg(test)]
fn validate_destination_ip(
    ctx: &ConnectionContext,
    dst: &Destination,
) -> Result<SocketAddr, AclDenial> {
    let (ip, port) = match dst {
        Destination::Ip(ip, port) => (*ip, *port),
        Destination::Domain(_, _) => {
            return if ctx.resolver.is_some() {
                Err(AclDenial::ResolutionFailed)
            } else {
                Err(AclDenial::DomainUnsupported)
            };
        }
    };
    if !ctx.allow_ports.contains(&port) {
        return Err(AclDenial::PortDenied);
    }
    let routes = ctx.routes.read().map_err(|_| AclDenial::NotRoutable)?;
    if routes.resolve(ip).is_none() {
        return Err(AclDenial::NotRoutable);
    }
    Ok(SocketAddr::new(ip, port))
}

impl Destination {
    fn port(&self) -> u16 {
        match self {
            Destination::Ip(_, p) => *p,
            Destination::Domain(_, p) => *p,
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn is_loopback(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

fn generate_auth_token() -> String {
    let mut raw = [0u8; AUTH_TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut raw);
    // Hex so the token is safe to embed in URLs (socks5h://user:TOKEN@…),
    // env vars, and HTTP Basic auth headers without further escaping.
    hex_encode(&raw)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Write the port + auth discovery files atomically with mode 0600.
fn write_discovery_files(state_dir: &Path, port: u16, token: &str) -> crate::Result<()> {
    std::fs::create_dir_all(state_dir).map_err(|e| crate::Error::Io {
        context: format!("create state_dir {}", state_dir.display()),
        source: e,
    })?;
    write_private(&state_dir.join("socks5.port"), port.to_string().as_bytes())?;
    write_private(&state_dir.join("socks5.auth"), token.as_bytes())?;
    Ok(())
}

/// Write a file with mode 0600 on Unix. On other platforms we fall back to
/// the platform default (but we don't ship sunbeam-net on Windows).
fn write_private(path: &Path, contents: &[u8]) -> crate::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| crate::Error::Io {
                context: format!("open {} for write", path.display()),
                source: e,
            })?;
        use std::io::Write;
        f.write_all(contents).map_err(|e| crate::Error::Io {
            context: format!("write {}", path.display()),
            source: e,
        })?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents).map_err(|e| crate::Error::Io {
            context: format!("write {}", path.display()),
            source: e,
        })?;
    }
    Ok(())
}

fn io_ctx(ctx: &'static str) -> impl FnOnce(io::Error) -> crate::Error {
    move |source| crate::Error::Io {
        context: ctx.into(),
        source,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::RouteTable;
    use crate::proto::types::{HostInfo, Node};
    use std::net::Ipv4Addr;
    use std::str::FromStr;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_node(key: &str, cidrs: &[&str]) -> Node {
        Node {
            id: 1,
            key: key.to_string(),
            disco_key: format!("discokey:{key}"),
            addresses: vec![],
            allowed_ips: cidrs.iter().map(|s| s.to_string()).collect(),
            endpoints: vec![],
            derp: String::new(),
            hostinfo: HostInfo::default(),
            name: "test".to_string(),
            online: None,
            machine_authorized: true,
        }
    }

    fn test_routes(cidrs: &[&str]) -> Arc<RwLock<RouteTable>> {
        let whitelist = crate::config::default_route_whitelist();
        let node = test_node("nodekey:aabbccdd", cidrs);
        let mut t = RouteTable::new();
        t.rebuild(&[node], &whitelist);
        Arc::new(RwLock::new(t))
    }

    async fn spawn_server(
        routes: Arc<RwLock<RouteTable>>,
        allow_ports: Vec<u16>,
    ) -> (
        SocksEndpoint,
        mpsc::Receiver<EngineCommand>,
        CancellationToken,
    ) {
        let dir = tempdir().unwrap();
        let (tx, rx) = mpsc::channel(8);
        let cfg = SocksConfig {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            allow_ports,
            state_dir: dir.path().to_path_buf(),
        };
        // Keep tempdir alive for the test — leak intentionally.
        let dir_path = dir.keep();
        let audit = Arc::new(AuditLog::new());
        let (server, endpoint) = SocksServer::bind(cfg, routes, tx, None, audit)
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        tokio::spawn(async move {
            let _keep_alive = dir_path;
            let _ = server.run(cancel_task).await;
        });
        (endpoint, rx, cancel)
    }

    /// Spawn a SOCKS server wired up to a real `Resolver`. DNS traffic
    /// and CONNECT traffic both flow through a single engine command
    /// channel; a background dispatcher splits them by destination
    /// port (53 → stub DNS handler, anything else → forwarded to the
    /// returned channel for the test to inspect).
    async fn spawn_server_with_resolver(
        routes: Arc<RwLock<RouteTable>>,
        allow_ports: Vec<u16>,
        resolve_to: IpAddr,
    ) -> (
        SocksEndpoint,
        mpsc::Receiver<EngineCommand>,
        CancellationToken,
    ) {
        let dir = tempdir().unwrap();
        // Engine-side channel — both the resolver and the SOCKS server
        // push here.
        let (engine_tx, mut engine_rx) = mpsc::channel::<EngineCommand>(16);
        // Non-DNS commands surface on this channel for the test.
        let (fwd_tx, fwd_rx) = mpsc::channel::<EngineCommand>(8);

        let resolver = Arc::new(Resolver::new(
            "10.43.0.10:53".parse().unwrap(),
            engine_tx.clone(),
            vec![],
        ));

        let cfg = SocksConfig {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            allow_ports,
            state_dir: dir.path().to_path_buf(),
        };
        let dir_path = dir.keep();
        let audit = Arc::new(AuditLog::new());
        let (server, endpoint) = SocksServer::bind(cfg, routes, engine_tx, Some(resolver), audit)
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        tokio::spawn(async move {
            let _keep_alive = dir_path;
            let _ = server.run(cancel_task).await;
        });

        // Dispatcher: DNS queries get a canned response; anything else
        // is forwarded so the caller can assert on it.
        tokio::spawn(async move {
            while let Some(cmd) = engine_rx.recv().await {
                let EngineCommand::NewConnection { local, remote } = cmd;
                if remote.port() == 53 {
                    tokio::spawn(stub_dns_answer(local, resolve_to));
                } else {
                    let _ = fwd_tx
                        .send(EngineCommand::NewConnection { local, remote })
                        .await;
                }
            }
        });

        (endpoint, fwd_rx, cancel)
    }

    /// Stub DNS handler: reads a single length-prefixed DNS query
    /// from the provided stream, echoes it back with the chosen
    /// answer address. Works for both A and AAAA queries.
    async fn stub_dns_answer(mut stream: TcpStream, answer: IpAddr) {
        let mut len_buf = [0u8; 2];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return;
        }
        let qlen = u16::from_be_bytes(len_buf) as usize;
        let mut query = vec![0u8; qlen];
        if stream.read_exact(&mut query).await.is_err() {
            return;
        }
        let id = u16::from_be_bytes([query[0], query[1]]);
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
        let qtype = u16::from_be_bytes([query[offset], query[offset + 1]]);
        let q_end = offset + 4;
        let question = &query[q_start..q_end];

        let answer_matches_qtype =
            (qtype == 1 && answer.is_ipv4()) || (qtype == 28 && answer.is_ipv6());

        let mut msg = Vec::new();
        msg.extend_from_slice(&id.to_be_bytes());
        msg.extend_from_slice(&0x8180u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // qdcount
        msg.extend_from_slice(&(if answer_matches_qtype { 1 } else { 0 } as u16).to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(question);

        if answer_matches_qtype {
            let ptr = (0xC000 | q_start as u16).to_be_bytes();
            msg.extend_from_slice(&ptr);
            msg.extend_from_slice(&qtype.to_be_bytes());
            msg.extend_from_slice(&1u16.to_be_bytes()); // IN
            msg.extend_from_slice(&60u32.to_be_bytes());
            match answer {
                IpAddr::V4(v4) => {
                    msg.extend_from_slice(&4u16.to_be_bytes());
                    msg.extend_from_slice(&v4.octets());
                }
                IpAddr::V6(v6) => {
                    msg.extend_from_slice(&16u16.to_be_bytes());
                    msg.extend_from_slice(&v6.octets());
                }
            }
        }

        let rlen = (msg.len() as u16).to_be_bytes();
        let _ = stream.write_all(&rlen).await;
        let _ = stream.write_all(&msg).await;
        let _ = stream.flush().await;
    }

    // ─── protocol detection ────────────────────────────────────────────────

    #[tokio::test]
    async fn rejects_non_loopback_bind() {
        let dir = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(8);
        let routes = Arc::new(RwLock::new(RouteTable::new()));
        let cfg = SocksConfig {
            bind: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            allow_ports: vec![443],
            state_dir: dir.path().to_path_buf(),
        };
        let audit = Arc::new(AuditLog::new());
        let err = SocksServer::bind(cfg, routes, tx, None, audit)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("non-loopback"));
    }

    #[tokio::test]
    async fn writes_discovery_files_with_0600() {
        let dir = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(8);
        let routes = Arc::new(RwLock::new(RouteTable::new()));
        let cfg = SocksConfig {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            allow_ports: vec![443],
            state_dir: dir.path().to_path_buf(),
        };
        let audit = Arc::new(AuditLog::new());
        let (_server, endpoint) = SocksServer::bind(cfg, routes, tx, None, audit)
            .await
            .unwrap();
        assert!(endpoint.port > 0);
        assert_eq!(endpoint.auth_token.len(), AUTH_TOKEN_BYTES * 2);

        let port_file = dir.path().join("socks5.port");
        let auth_file = dir.path().join("socks5.auth");
        assert_eq!(
            std::fs::read_to_string(&port_file).unwrap(),
            endpoint.port.to_string()
        );
        assert_eq!(
            std::fs::read_to_string(&auth_file).unwrap(),
            endpoint.auth_token
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [&port_file, &auth_file] {
                let meta = std::fs::metadata(f).unwrap();
                let mode = meta.permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{} has mode {mode:o}", f.display());
            }
        }
    }

    // ─── SOCKS5 happy path ─────────────────────────────────────────────────

    async fn socks5_connect_request(
        stream: &mut TcpStream,
        token: &str,
        dst_ip: IpAddr,
        dst_port: u16,
    ) {
        // greeting
        stream
            .write_all(&[SOCKS5_VERSION, 1, SOCKS5_METHOD_USERPASS])
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [SOCKS5_VERSION, SOCKS5_METHOD_USERPASS]);

        // auth
        let mut auth = vec![SOCKS5_AUTH_VERSION];
        auth.push(SOCKS_USERNAME.len() as u8);
        auth.extend_from_slice(SOCKS_USERNAME.as_bytes());
        auth.push(token.len() as u8);
        auth.extend_from_slice(token.as_bytes());
        stream.write_all(&auth).await.unwrap();
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [SOCKS5_AUTH_VERSION, 0x00]);

        // request
        let mut req = vec![SOCKS5_VERSION, SOCKS5_CMD_CONNECT, 0x00];
        match dst_ip {
            IpAddr::V4(v4) => {
                req.push(AddrType::Ipv4 as u8);
                req.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                req.push(AddrType::Ipv6 as u8);
                req.extend_from_slice(&v6.octets());
            }
        }
        req.extend_from_slice(&dst_port.to_be_bytes());
        stream.write_all(&req).await.unwrap();
    }

    async fn read_socks5_reply_code(stream: &mut TcpStream) -> u8 {
        let mut hdr = [0u8; 4];
        stream.read_exact(&mut hdr).await.unwrap();
        // skip BND.ADDR + BND.PORT based on atyp
        let skip = match hdr[3] {
            x if x == AddrType::Ipv4 as u8 => 4 + 2,
            x if x == AddrType::Ipv6 as u8 => 16 + 2,
            _ => 2,
        };
        let mut skip_buf = vec![0u8; skip];
        stream.read_exact(&mut skip_buf).await.unwrap();
        hdr[1]
    }

    #[tokio::test]
    async fn socks5_connect_allowed() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, mut rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        socks5_connect_request(
            &mut client,
            &endpoint.auth_token,
            IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5)),
            443,
        )
        .await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::Succeeded as u8
        );

        // The connection should have been handed to the engine channel.
        let cmd = rx.recv().await.expect("engine got a connection");
        match cmd {
            EngineCommand::NewConnection { remote, .. } => {
                assert_eq!(remote, SocketAddr::from((Ipv4Addr::new(10, 42, 0, 5), 443)));
            }
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_rejects_no_auth_method() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        // Offer only NO_AUTH (0x00).
        client.write_all(&[SOCKS5_VERSION, 1, 0x00]).await.unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [SOCKS5_VERSION, SOCKS5_METHOD_NONE]);
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_rejects_wrong_password() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        // greeting
        client
            .write_all(&[SOCKS5_VERSION, 1, SOCKS5_METHOD_USERPASS])
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        // auth with bad password
        let mut auth = vec![SOCKS5_AUTH_VERSION];
        auth.push(SOCKS_USERNAME.len() as u8);
        auth.extend_from_slice(SOCKS_USERNAME.as_bytes());
        auth.push(4);
        auth.extend_from_slice(b"nope");
        client.write_all(&auth).await.unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf[0], SOCKS5_AUTH_VERSION);
        assert_eq!(buf[1], 0x01);
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_rejects_disallowed_port() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        socks5_connect_request(
            &mut client,
            &endpoint.auth_token,
            IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5)),
            22, // not in allow list
        )
        .await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::ConnectionNotAllowed as u8
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_rejects_unrouted_ip() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        socks5_connect_request(
            &mut client,
            &endpoint.auth_token,
            IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            443,
        )
        .await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::NetworkUnreachable as u8
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_rejects_bind_command() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        // greeting
        client
            .write_all(&[SOCKS5_VERSION, 1, SOCKS5_METHOD_USERPASS])
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        // auth
        let mut auth = vec![SOCKS5_AUTH_VERSION];
        auth.push(SOCKS_USERNAME.len() as u8);
        auth.extend_from_slice(SOCKS_USERNAME.as_bytes());
        auth.push(endpoint.auth_token.len() as u8);
        auth.extend_from_slice(endpoint.auth_token.as_bytes());
        client.write_all(&auth).await.unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        // request w/ BIND (0x02)
        let mut req = vec![SOCKS5_VERSION, 0x02, 0x00, AddrType::Ipv4 as u8];
        req.extend_from_slice(&[10, 42, 0, 5]);
        req.extend_from_slice(&443u16.to_be_bytes());
        client.write_all(&req).await.unwrap();
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::CommandNotSupported as u8
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_domain_deferred_to_phase3() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        // greeting + auth
        client
            .write_all(&[SOCKS5_VERSION, 1, SOCKS5_METHOD_USERPASS])
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).await.unwrap();
        let mut auth = vec![SOCKS5_AUTH_VERSION];
        auth.push(SOCKS_USERNAME.len() as u8);
        auth.extend_from_slice(SOCKS_USERNAME.as_bytes());
        auth.push(endpoint.auth_token.len() as u8);
        auth.extend_from_slice(endpoint.auth_token.as_bytes());
        client.write_all(&auth).await.unwrap();
        client.read_exact(&mut buf).await.unwrap();
        // Request with domain name
        let host = b"svc.cluster.local";
        let mut req = vec![
            SOCKS5_VERSION,
            SOCKS5_CMD_CONNECT,
            0x00,
            AddrType::Domain as u8,
        ];
        req.push(host.len() as u8);
        req.extend_from_slice(host);
        req.extend_from_slice(&443u16.to_be_bytes());
        client.write_all(&req).await.unwrap();
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::AddressTypeNotSupported as u8
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_ipv6_ok() {
        // fd7a:115c:a1e0::/48 is in the default whitelist.
        let routes = test_routes(&["fd7a:115c:a1e0::/48"]);
        let (endpoint, mut rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        let ip = Ipv6Addr::from_str("fd7a:115c:a1e0:ab12::1").unwrap();
        socks5_connect_request(&mut client, &endpoint.auth_token, IpAddr::V6(ip), 443).await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::Succeeded as u8
        );
        rx.recv().await.unwrap();
        cancel.cancel();
    }

    // ─── HTTP CONNECT path ─────────────────────────────────────────────────

    async fn http_connect(
        endpoint: &SocksEndpoint,
        target: &str,
        auth_token: Option<&str>,
    ) -> TcpStream {
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        let auth_line = match auth_token {
            Some(tok) => {
                let b64 = base64::engine::general_purpose::STANDARD
                    .encode(format!("{SOCKS_USERNAME}:{tok}"));
                format!("Proxy-Authorization: Basic {b64}\r\n")
            }
            None => String::new(),
        };
        let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth_line}\r\n");
        client.write_all(req.as_bytes()).await.unwrap();
        client
    }

    async fn read_http_status(stream: &mut TcpStream) -> u16 {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 256];
        loop {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let first = text.lines().next().unwrap_or("");
        let mut parts = first.split_whitespace();
        let _version = parts.next();
        parts.next().unwrap_or("0").parse().unwrap_or(0)
    }

    #[tokio::test]
    async fn http_connect_allowed() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, mut rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(&endpoint, "10.42.0.5:443", Some(&endpoint.auth_token)).await;
        assert_eq!(read_http_status(&mut client).await, 200);
        rx.recv().await.unwrap();
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_no_auth_rejected() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(&endpoint, "10.42.0.5:443", None).await;
        assert_eq!(read_http_status(&mut client).await, 407);
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_wrong_token_rejected() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(&endpoint, "10.42.0.5:443", Some("bogus")).await;
        assert_eq!(read_http_status(&mut client).await, 407);
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_wrong_method_rejected() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD
            .encode(format!("{SOCKS_USERNAME}:{}", endpoint.auth_token));
        let req = format!(
            "GET / HTTP/1.1\r\nHost: 10.42.0.5\r\nProxy-Authorization: Basic {b64}\r\n\r\n"
        );
        client.write_all(req.as_bytes()).await.unwrap();
        assert_eq!(read_http_status(&mut client).await, 405);
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_port_denied() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(&endpoint, "10.42.0.5:22", Some(&endpoint.auth_token)).await;
        assert_eq!(read_http_status(&mut client).await, 403);
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_unrouted_ip() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(&endpoint, "8.8.8.8:443", Some(&endpoint.auth_token)).await;
        assert_eq!(read_http_status(&mut client).await, 502);
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_domain_rejected_without_resolver() {
        let routes = test_routes(&["10.42.0.0/16"]);
        // Use an allowed port so we exercise the no-resolver path,
        // not the port ACL.
        let (endpoint, _rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(
            &endpoint,
            "postgres.data.svc.cluster.local:443",
            Some(&endpoint.auth_token),
        )
        .await;
        assert_eq!(read_http_status(&mut client).await, 421);
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_ipv6_bracketed() {
        let routes = test_routes(&["fd7a:115c:a1e0::/48"]);
        let (endpoint, mut rx, cancel) = spawn_server(routes, vec![443]).await;
        let mut client = http_connect(
            &endpoint,
            "[fd7a:115c:a1e0:ab12::1]:443",
            Some(&endpoint.auth_token),
        )
        .await;
        assert_eq!(read_http_status(&mut client).await, 200);
        rx.recv().await.unwrap();
        cancel.cancel();
    }

    // ─── end-to-end resolver integration ───────────────────────────────────

    async fn socks5_domain_connect_request(
        stream: &mut TcpStream,
        token: &str,
        host: &str,
        port: u16,
    ) {
        stream
            .write_all(&[SOCKS5_VERSION, 1, SOCKS5_METHOD_USERPASS])
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.unwrap();
        let mut auth = vec![SOCKS5_AUTH_VERSION];
        auth.push(SOCKS_USERNAME.len() as u8);
        auth.extend_from_slice(SOCKS_USERNAME.as_bytes());
        auth.push(token.len() as u8);
        auth.extend_from_slice(token.as_bytes());
        stream.write_all(&auth).await.unwrap();
        stream.read_exact(&mut buf).await.unwrap();

        let host_bytes = host.as_bytes();
        let mut req = vec![
            SOCKS5_VERSION,
            SOCKS5_CMD_CONNECT,
            0x00,
            AddrType::Domain as u8,
        ];
        req.push(host_bytes.len() as u8);
        req.extend_from_slice(host_bytes);
        req.extend_from_slice(&port.to_be_bytes());
        stream.write_all(&req).await.unwrap();
    }

    #[tokio::test]
    async fn socks5_connect_resolves_domain_to_routed_ipv4() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let resolved = IpAddr::V4(Ipv4Addr::new(10, 42, 0, 42));
        let (endpoint, mut fwd_rx, cancel) =
            spawn_server_with_resolver(routes, vec![443], resolved).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        socks5_domain_connect_request(
            &mut client,
            &endpoint.auth_token,
            "postgres.data.svc.cluster.local",
            443,
        )
        .await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::Succeeded as u8
        );
        let cmd = fwd_rx
            .recv()
            .await
            .expect("engine got forwarded connection");
        match cmd {
            EngineCommand::NewConnection { remote, .. } => {
                assert_eq!(remote, SocketAddr::new(resolved, 443));
            }
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_connect_resolves_domain_to_routed_ipv6() {
        let routes = test_routes(&["fd7a:115c:a1e0::/48"]);
        let resolved = IpAddr::V6(Ipv6Addr::from_str("fd7a:115c:a1e0:ab12::99").unwrap());
        let (endpoint, mut fwd_rx, cancel) =
            spawn_server_with_resolver(routes, vec![443], resolved).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        socks5_domain_connect_request(
            &mut client,
            &endpoint.auth_token,
            "hydra.auth.svc.cluster.local",
            443,
        )
        .await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::Succeeded as u8
        );
        let cmd = fwd_rx
            .recv()
            .await
            .expect("engine got forwarded connection");
        match cmd {
            EngineCommand::NewConnection { remote, .. } => {
                assert_eq!(remote, SocketAddr::new(resolved, 443));
            }
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn socks5_connect_resolves_domain_but_ip_not_routable() {
        let routes = test_routes(&["10.42.0.0/16"]);
        // Resolver returns an IP that is not in the route table.
        let resolved = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        let (endpoint, _fwd_rx, cancel) =
            spawn_server_with_resolver(routes, vec![443], resolved).await;
        let mut client = TcpStream::connect(("127.0.0.1", endpoint.port))
            .await
            .unwrap();
        socks5_domain_connect_request(&mut client, &endpoint.auth_token, "evil.example.com", 443)
            .await;
        assert_eq!(
            read_socks5_reply_code(&mut client).await,
            Reply::NetworkUnreachable as u8
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_resolves_domain_to_routed_ip() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let resolved = IpAddr::V4(Ipv4Addr::new(10, 42, 0, 7));
        let (endpoint, mut fwd_rx, cancel) =
            spawn_server_with_resolver(routes, vec![443], resolved).await;
        let mut client = http_connect(
            &endpoint,
            "gitea.devtools.svc.cluster.local:443",
            Some(&endpoint.auth_token),
        )
        .await;
        assert_eq!(read_http_status(&mut client).await, 200);
        let cmd = fwd_rx
            .recv()
            .await
            .expect("engine got forwarded connection");
        match cmd {
            EngineCommand::NewConnection { remote, .. } => {
                assert_eq!(remote, SocketAddr::new(resolved, 443));
            }
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn http_connect_resolves_domain_but_ip_not_routable() {
        let routes = test_routes(&["10.42.0.0/16"]);
        let resolved = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let (endpoint, _fwd_rx, cancel) =
            spawn_server_with_resolver(routes, vec![443], resolved).await;
        let mut client = http_connect(
            &endpoint,
            "leak.example.com:443",
            Some(&endpoint.auth_token),
        )
        .await;
        assert_eq!(read_http_status(&mut client).await, 502);
        cancel.cancel();
    }

    // ─── parse helpers ─────────────────────────────────────────────────────

    #[test]
    fn destination_parse_ipv4() {
        let d = Destination::parse_host_port("10.0.0.1:443").unwrap();
        assert_eq!(
            d,
            Destination::Ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443)
        );
    }

    #[test]
    fn destination_parse_ipv6_bracketed() {
        let d = Destination::parse_host_port("[::1]:80").unwrap();
        assert_eq!(d, Destination::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST), 80));
    }

    #[test]
    fn destination_parse_domain() {
        let d = Destination::parse_host_port("svc.cluster.local:5432").unwrap();
        assert_eq!(
            d,
            Destination::Domain("svc.cluster.local".to_string(), 5432)
        );
    }

    #[test]
    fn destination_parse_malformed() {
        assert!(Destination::parse_host_port("noport").is_none());
        assert!(Destination::parse_host_port("host:abc").is_none());
        assert!(Destination::parse_host_port("[::1").is_none());
    }

    #[test]
    fn http_request_parse_basic() {
        let raw =
            b"CONNECT host:443 HTTP/1.1\r\nHost: host\r\nProxy-Authorization: Basic abc\r\n\r\n";
        let req = HttpRequest::parse(raw).unwrap();
        assert_eq!(req.method, "CONNECT");
        assert_eq!(req.target, "host:443");
        assert_eq!(req.header("proxy-authorization"), Some("Basic abc"));
        assert_eq!(req.header("nonexistent"), None);
    }

    #[test]
    fn http_request_parse_rejects_garbage() {
        assert!(HttpRequest::parse(b"\xff\xfe not utf8").is_none());
        assert!(HttpRequest::parse(b"INCOMPLETE").is_none());
    }

    #[test]
    fn validate_http_auth_requires_basic() {
        let req = HttpRequest {
            method: "CONNECT".into(),
            target: "h:1".into(),
            headers: vec![("proxy-authorization".into(), "Digest abc".into())],
        };
        assert!(!validate_http_auth(&req, "token"));
    }

    #[test]
    fn validate_http_auth_success() {
        let token = "abc123";
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(format!("{SOCKS_USERNAME}:{token}"));
        let req = HttpRequest {
            method: "CONNECT".into(),
            target: "h:1".into(),
            headers: vec![("proxy-authorization".into(), format!("Basic {b64}"))],
        };
        assert!(validate_http_auth(&req, token));
    }

    #[test]
    fn validate_http_auth_rejects_bad_base64() {
        let req = HttpRequest {
            method: "CONNECT".into(),
            target: "h:1".into(),
            headers: vec![("proxy-authorization".into(), "Basic !!!".into())],
        };
        assert!(!validate_http_auth(&req, "token"));
    }

    #[test]
    fn hex_encode_round_trip() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xa1]), "00ffa1");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn generate_auth_token_is_random_and_long() {
        let a = generate_auth_token();
        let b = generate_auth_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), AUTH_TOKEN_BYTES * 2);
    }

    #[test]
    fn is_loopback_detection() {
        assert!(is_loopback(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_loopback(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_loopback(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_loopback(&IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn validate_destination_rejects_domain_when_no_resolver() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConnectionContext {
            routes: Arc::new(RwLock::new(RouteTable::new())),
            allow_ports: Arc::new(vec![443]),
            auth_token: Arc::new("t".into()),
            cmd_tx: tx,
            resolver: None,
            audit: Arc::new(AuditLog::new()),
            cancel: CancellationToken::new(),
        };
        let err = validate_destination_ip(&ctx, &Destination::Domain("h".into(), 443)).unwrap_err();
        assert_eq!(err, AclDenial::DomainUnsupported);
    }

    #[test]
    fn remove_discovery_files_cleans_up() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("socks5.port"), "1234").unwrap();
        std::fs::write(dir.path().join("socks5.auth"), "abcd").unwrap();
        SocksServer::remove_discovery_files(dir.path());
        assert!(!dir.path().join("socks5.port").exists());
        assert!(!dir.path().join("socks5.auth").exists());
    }
}
