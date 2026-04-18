//! `sunbeam connect` / `sunbeam disconnect` / `sunbeam vpn ...`
//!
//! `sunbeam connect` re-execs the current binary with a hidden
//! `__vpn-daemon` subcommand and detaches it (stdio → /dev/null + a log
//! file). The detached child runs the actual `sunbeam-net` daemon and
//! listens on the IPC control socket. The user-facing process polls the
//! socket until the daemon reaches Running, prints status, and exits.
//!
//! This shape avoids forking from inside the tokio runtime.

use crate::config::active_context;
use crate::error::{Result, SunbeamError};
use crate::output::{ok, step, warn};
use crate::vpn_env::vpn_state_dir;

/// Run `sunbeam connect`.
///
/// Default mode spawns a backgrounded daemon and returns once it reaches
/// Running. With `--foreground`, runs the daemon in-process and blocks
/// until SIGINT or SIGTERM.
pub async fn cmd_connect(foreground: bool) -> Result<()> {
    let ctx = active_context();
    if ctx.vpn_url.is_empty() {
        return Err(SunbeamError::Other(
            "no VPN configured for this context — set vpn-url and vpn-auth-key in config".into(),
        ));
    }
    if ctx.vpn_auth_key.is_empty() {
        return Err(SunbeamError::Other(
            "no VPN auth key for this context — set vpn-auth-key in config".into(),
        ));
    }

    let state_dir = vpn_state_dir()?;
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        SunbeamError::Other(format!("create vpn state dir {}: {e}", state_dir.display()))
    })?;

    if foreground {
        return run_daemon_foreground().await;
    }

    spawn_background_daemon(&state_dir).await
}

/// Spawn a detached daemon child and wait for it to reach Running.
async fn spawn_background_daemon(state_dir: &std::path::Path) -> Result<()> {
    // Refuse to start a second daemon if one is already running.
    let socket = state_dir.join("daemon.sock");
    let probe = sunbeam_net::IpcClient::new(&socket);
    if probe.socket_exists() {
        if let Ok(status) = probe.status().await {
            warn(&format!(
                "VPN daemon already running ({status}). Use `sunbeam disconnect` first."
            ));
            return Ok(());
        }
        // Stale socket — clean it up so the new daemon can rebind.
        let _ = std::fs::remove_file(&socket);
    }

    // Re-exec ourselves with the hidden subcommand.
    let exe = std::env::current_exe()
        .map_err(|e| SunbeamError::Other(format!("locate current_exe: {e}")))?;
    let log_path = state_dir.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| SunbeamError::Other(format!("open daemon log: {e}")))?;
    let log_err = log
        .try_clone()
        .map_err(|e| SunbeamError::Other(format!("dup daemon log fd: {e}")))?;

    let mut cmd = std::process::Command::new(&exe);
    // --context is a top-level flag, must precede the subcommand.
    let cfg = crate::config::load_config();
    if !cfg.current_context.is_empty() {
        cmd.arg("--context").arg(&cfg.current_context);
    }
    cmd.arg("__vpn-daemon");
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log_err));

    // Detach from the controlling terminal so closing the parent shell
    // doesn't SIGHUP the daemon.
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            // Become a session leader so the child has no controlling TTY.
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .map_err(|e| SunbeamError::Other(format!("spawn daemon: {e}")))?;

    step(&format!(
        "VPN daemon spawned (pid {}, logs at {})",
        child.id(),
        log_path.display()
    ));

    // Poll the IPC socket until the daemon reaches Running.
    let client = sunbeam_net::IpcClient::new(&socket);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if std::time::Instant::now() > deadline {
            warn(
                "VPN daemon did not reach Running state within 30s — \
                 check the daemon log for details",
            );
            return Ok(());
        }
        if !client.socket_exists() {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            continue;
        }
        match client.status().await {
            Ok(sunbeam_net::DaemonStatus::Running {
                addresses,
                peer_count,
                ..
            }) => {
                let addrs: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
                ok(&format!(
                    "Connected ({}) — {} peers visible",
                    addrs.join(", "),
                    peer_count
                ));
                return Ok(());
            }
            Ok(sunbeam_net::DaemonStatus::Error { message }) => {
                return Err(SunbeamError::Other(format!("VPN daemon error: {message}")));
            }
            // Still starting / connecting / registering — keep polling.
            Ok(_) | Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
    }
}

/// The hidden `__vpn-daemon` subcommand entry point.
pub async fn cmd_vpn_daemon() -> Result<()> {
    run_daemon_foreground().await
}

/// Build VpnConfig from the active context, start the daemon, and block
/// until SIGINT/SIGTERM or an IPC `Stop` request brings it down.
async fn run_daemon_foreground() -> Result<()> {
    let ctx = active_context();
    let state_dir = vpn_state_dir()?;
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        SunbeamError::Other(format!("create vpn state dir {}: {e}", state_dir.display()))
    })?;

    let user = whoami::username().unwrap_or_else(|_| "unknown".to_string());
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());
    let hostname = format!("{user}@{host}");

    let config = sunbeam_net::VpnConfig {
        coordination_url: ctx.vpn_url.clone(),
        auth_key: ctx.vpn_auth_key.clone(),
        state_dir: state_dir.clone(),
        // Bind the local k8s proxy on 16579 — far enough away from common
        // conflicts (6443 = kube API) that we shouldn't collide on dev
        // machines. TODO: make this configurable.
        proxy_bind: crate::vpn_env::VPN_K8S_PROXY.parse().expect("static addr"),
        // The daemon auto-picks the first non-self peer when this is
        // None, which is correct for single-cluster deployments. Set an
        // explicit fallback here only if you have multiple peers and
        // want to override that heuristic.
        cluster_api_addr: None,
        cluster_api_port: 443,
        // If the user set vpn-cluster-host in their context config, the
        // daemon resolves it from the netmap and uses that peer's
        // tailnet IP for the proxy backend.
        cluster_api_host: if ctx.vpn_cluster_host.is_empty() {
            None
        } else {
            Some(ctx.vpn_cluster_host.clone())
        },
        control_socket: state_dir.join("daemon.sock"),
        hostname,
        server_public_key: None,
        derp_tls_insecure: ctx.vpn_tls_insecure,
        route_whitelist: sunbeam_net::config::default_route_whitelist(),
        socks_bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        socks_allow_ports: sunbeam_net::config::default_socks_allow_ports(),
        dns_server: parse_dns_server(&ctx.vpn_dns_server)?,
        dns_search_domains: parse_dns_search(&ctx.vpn_dns_search),
    };

    step(&format!("Connecting to {}", ctx.vpn_url));
    let handle = sunbeam_net::VpnDaemon::start(config)
        .await
        .map_err(|e| SunbeamError::Other(format!("daemon start: {e}")))?;

    // Wait for either Ctrl-C, SIGTERM, or the daemon stopping itself
    // (e.g. via an IPC `Stop` request).
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| SunbeamError::Other(format!("install SIGTERM handler: {e}")))?;

    loop {
        tokio::select! {
            biased;
            _ = &mut ctrl_c => {
                step("Interrupt — disconnecting...");
                break;
            }
            _ = sigterm.recv() => {
                step("SIGTERM — disconnecting...");
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if matches!(handle.current_status(), sunbeam_net::DaemonStatus::Stopped) {
                    break;
                }
            }
        }
    }

    handle
        .shutdown()
        .await
        .map_err(|e| SunbeamError::Other(format!("daemon shutdown: {e}")))?;
    ok("Disconnected.");
    Ok(())
}

/// Run `sunbeam disconnect` — signal a running daemon via its IPC socket.
pub async fn cmd_disconnect() -> Result<()> {
    let socket = vpn_state_dir()?.join("daemon.sock");
    let client = sunbeam_net::IpcClient::new(&socket);
    if !client.socket_exists() {
        return Err(SunbeamError::Other(
            "no running VPN daemon (control socket missing)".into(),
        ));
    }
    step("Asking VPN daemon to stop...");
    client
        .stop()
        .await
        .map_err(|e| SunbeamError::Other(format!("IPC stop: {e}")))?;
    ok("Daemon acknowledged shutdown.");
    Ok(())
}

/// Run `sunbeam vpn status` — query a running daemon's status via IPC.
pub async fn cmd_vpn_status() -> Result<()> {
    let socket = vpn_state_dir()?.join("daemon.sock");
    let client = sunbeam_net::IpcClient::new(&socket);
    if !client.socket_exists() {
        println!("VPN: not running");
        return Ok(());
    }
    match client.status().await {
        Ok(sunbeam_net::DaemonStatus::Running {
            addresses,
            peer_count,
            derp_home,
            socks_proxy_port,
        }) => {
            let addrs: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
            println!("VPN: running");
            println!("  addresses: {}", addrs.join(", "));
            println!("  peers: {peer_count}");
            if let Some(region) = derp_home {
                println!("  derp home: region {region}");
            }
            if let Some(port) = socks_proxy_port {
                println!("  socks proxy: 127.0.0.1:{port}");
            }
            print_routes(&client).await;
            print_recent_connections(&client).await;
        }
        Ok(other) => {
            println!("VPN: {other}");
        }
        Err(e) => {
            // Socket exists but daemon isn't actually responding — common
            // when the daemon crashed and left a stale socket file behind.
            println!("VPN: stale socket at {} ({e})", socket.display());
        }
    }
    Ok(())
}

/// Query the daemon for its subnet-router table and print one line per
/// advertised prefix. Silently skipped if the daemon doesn't answer —
/// `sunbeam vpn status` is a best-effort view and we'd rather show
/// partial output than bail.
async fn print_routes(client: &sunbeam_net::IpcClient) {
    match client.routes().await {
        Ok(routes) if !routes.is_empty() => {
            println!("  routes:");
            for r in routes {
                // Collapse long node keys to a short suffix; the full
                // 64-char base64 hides the useful information.
                let short = short_node_key(&r.node_key);
                println!("    {:<20}  via {short}", r.cidr);
            }
        }
        Ok(_) => {
            println!("  routes: (none)");
        }
        Err(e) => {
            println!("  routes: (query failed: {e})");
        }
    }
}

/// Pull the last few SOCKS/HTTP proxy audit entries and render them
/// as a compact table. Accepted and denied entries share the same
/// layout so it's easy to eyeball what the proxy is actually doing.
async fn print_recent_connections(client: &sunbeam_net::IpcClient) {
    const TAIL: usize = 10;
    match client.recent_connections(TAIL).await {
        Ok(entries) if !entries.is_empty() => {
            println!("  recent connections (newest first):");
            for e in entries {
                println!(
                    "    {:>5}  {:<20}  {}",
                    e.protocol,
                    e.outcome.label(),
                    e.destination,
                );
            }
        }
        Ok(_) => {
            println!("  recent connections: (none)");
        }
        Err(e) => {
            println!("  recent connections: (query failed: {e})");
        }
    }
}

/// Collapse a full `nodekey:...` string to a short display form so the
/// routes table stays readable. We keep the first 12 hex characters,
/// which is unique enough for operator-level identification.
fn short_node_key(key: &str) -> String {
    let body = key.strip_prefix("nodekey:").unwrap_or(key);
    let cut = body
        .char_indices()
        .nth(12)
        .map(|(i, _)| i)
        .unwrap_or(body.len());
    format!("nodekey:{}…", &body[..cut])
}

/// Run `sunbeam vpn create-key` — call Headscale's REST API to mint a
/// new pre-auth key for onboarding a new client.
///
/// Reads `vpn-url` and `vpn-api-key` from the active context. The user
/// must have generated a Headscale API key out-of-band (typically via
/// `headscale apikeys create` on the cluster) and stored it in the
/// context config.
pub async fn cmd_vpn_create_key(
    user: &str,
    reusable: bool,
    ephemeral: bool,
    expiration: &str,
) -> Result<()> {
    let ctx = active_context();
    if ctx.vpn_url.is_empty() {
        return Err(SunbeamError::Other(
            "no vpn-url configured for this context".into(),
        ));
    }
    if ctx.vpn_api_key.is_empty() {
        return Err(SunbeamError::Other(
            "no vpn-api-key configured — generate one with \
             `kubectl exec -n vpn deploy/headscale -- headscale apikeys create` \
             and add it to your context as vpn-api-key"
                .into(),
        ));
    }

    // Headscale's REST API mirrors its gRPC schema. Body fields use
    // snake_case in the JSON request.
    let body = serde_json::json!({
        "user": user,
        "reusable": reusable,
        "ephemeral": ephemeral,
        "expiration": expiration_to_rfc3339(expiration)?,
    });

    let endpoint = format!("{}/api/v1/preauthkey", ctx.vpn_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(ctx.vpn_tls_insecure)
        .build()
        .map_err(|e| SunbeamError::Other(format!("build http client: {e}")))?;

    step(&format!("Creating pre-auth key on {}", ctx.vpn_url));
    let resp = client
        .post(&endpoint)
        .bearer_auth(&ctx.vpn_api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| SunbeamError::Other(format!("POST {endpoint}: {e}")))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| SunbeamError::Other(format!("read response body: {e}")))?;

    if !status.is_success() {
        return Err(SunbeamError::Other(format!(
            "headscale returned {status}: {text}"
        )));
    }

    // Response shape: {"preAuthKey": {"key": "...", "user": "...", "reusable": ..., ...}}
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| SunbeamError::Other(format!("parse headscale response: {e}\nbody: {text}")))?;
    let key = parsed
        .get("preAuthKey")
        .and_then(|p| p.get("key"))
        .and_then(|k| k.as_str())
        .ok_or_else(|| SunbeamError::Other(format!("no preAuthKey.key in response: {text}")))?;

    ok(&format!("Pre-auth key for user '{user}':"));
    println!("{key}");
    println!();
    println!("Add it to a context with:");
    println!("  sunbeam config set --context <ctx> vpn-auth-key {key}");
    Ok(())
}

/// Convert a human-friendly duration ("30d", "1h", "2w") into the RFC3339
/// timestamp Headscale expects in pre-auth key requests.
fn expiration_to_rfc3339(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(SunbeamError::Other("empty expiration".into()));
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u64 = num
        .parse()
        .map_err(|_| SunbeamError::Other(format!("bad expiration '{s}': expected like '30d'")))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        "w" => n * 86_400 * 7,
        other => {
            return Err(SunbeamError::Other(format!(
                "bad expiration unit '{other}': expected s/m/h/d/w"
            )));
        }
    };
    let when = std::time::SystemTime::now()
        .checked_add(std::time::Duration::from_secs(secs))
        .ok_or_else(|| SunbeamError::Other("expiration overflow".into()))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| SunbeamError::Other(format!("system time: {e}")))?
        .as_secs();
    // Format as RFC3339 manually using the same proleptic Gregorian
    // approach as elsewhere in this crate. Headscale parses Go's
    // time.RFC3339, which is `YYYY-MM-DDTHH:MM:SSZ`.
    Ok(unix_to_rfc3339(when as i64))
}

/// Convert a unix timestamp (seconds since epoch) to an RFC3339 string.
fn unix_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Days since 1970-01-01 → (year, month, day). Proleptic Gregorian.
fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    let mut y: i32 = 1970;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let year_days: i64 = if leap { 366 } else { 365 };
        if days < year_days {
            break;
        }
        days -= year_days;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mdays = [
        31u32,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u32;
    let mut days_u = days as u32;
    for md in mdays {
        if days_u < md {
            break;
        }
        days_u -= md;
        mo += 1;
    }
    (y, mo, days_u + 1)
}

/// Parse the context's `vpn-dns-server` field into a `SocketAddr`.
/// An empty string disables cluster DNS (returns `None`). A value
/// without an explicit port defaults to `:53`.
fn parse_dns_server(raw: &str) -> Result<Option<std::net::SocketAddr>> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if let Ok(addr) = s.parse::<std::net::SocketAddr>() {
        return Ok(Some(addr));
    }
    if let Ok(ip) = s.parse::<std::net::IpAddr>() {
        return Ok(Some(std::net::SocketAddr::new(ip, 53)));
    }
    Err(SunbeamError::Other(format!(
        "vpn-dns-server {s:?} is not a valid IP or host:port"
    )))
}

/// Parse the context's `vpn-dns-search` field into a list of search
/// domains, defaulting to k8s cluster domains when empty.
fn parse_dns_search(raw: &str) -> Vec<String> {
    let s = raw.trim();
    if s.is_empty() {
        return vec!["svc.cluster.local".into(), "cluster.local".into()];
    }
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod dns_parse_tests {
    use super::*;

    #[test]
    fn parses_empty_as_none() {
        assert!(parse_dns_server("").unwrap().is_none());
        assert!(parse_dns_server("   ").unwrap().is_none());
    }

    #[test]
    fn parses_ip_only_as_port_53() {
        let got = parse_dns_server("10.43.0.10").unwrap().unwrap();
        assert_eq!(got, "10.43.0.10:53".parse().unwrap());
    }

    #[test]
    fn parses_full_host_port() {
        let got = parse_dns_server("10.43.0.10:9053").unwrap().unwrap();
        assert_eq!(got, "10.43.0.10:9053".parse().unwrap());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_dns_server("not-an-ip").is_err());
    }

    #[test]
    fn search_domains_default_when_empty() {
        assert_eq!(
            parse_dns_search(""),
            vec!["svc.cluster.local".to_string(), "cluster.local".to_string()]
        );
    }

    #[test]
    fn search_domains_split_csv_and_trim() {
        assert_eq!(
            parse_dns_search(" svc.cluster.local , cluster.local , ops.local "),
            vec![
                "svc.cluster.local".to_string(),
                "cluster.local".to_string(),
                "ops.local".to_string()
            ]
        );
    }
}
