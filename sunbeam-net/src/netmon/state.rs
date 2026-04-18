use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};

/// Snapshot of all network interfaces and their state.
#[derive(Debug, Clone, Default)]
pub struct InterfaceState {
    pub interfaces: HashMap<String, InterfaceInfo>,
    pub default_route_interface: Option<String>,
    pub have_v4: bool,
    pub have_v6: bool,
}

/// Information about a single network interface.
#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub name: String,
    pub up: bool,
    pub addrs: Vec<IpAddr>,
}

/// Describes what changed between two interface snapshots.
#[derive(Debug, Clone)]
pub struct ChangeDelta {
    pub default_interface_changed: bool,
    pub interface_ips_changed: bool,
    /// Aggregate: true if any change suggests sockets should be rebound.
    pub rebind_likely_required: bool,
}

/// Interface name prefixes we never care about.
///
/// Covers macOS (utun, awdl, llw, ipsec, gif, XHC, anpi, bridge, ap) and Linux
/// (lo, docker, veth, br-, tun, tap) uninteresting interfaces.
const SKIP_PREFIXES: &[&str] = &[
    "lo", "utun", "awdl", "llw", "ipsec", "gif", "XHC", "anpi", "bridge", "ap", "docker", "veth",
    "br-", "tun", "tap",
];

fn is_interesting(name: &str) -> bool {
    !SKIP_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// True if the address is routable (not link-local, not loopback).
fn is_routable(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local(),
        IpAddr::V6(v6) => !v6.is_loopback() && !is_link_local_v6(v6),
    }
}

fn is_link_local_v6(addr: &Ipv6Addr) -> bool {
    // fe80::/10
    let seg = addr.segments();
    (seg[0] & 0xffc0) == 0xfe80
}

impl InterfaceState {
    /// Snapshot current system interfaces via `getifaddrs(3)`.
    pub fn snapshot() -> std::io::Result<Self> {
        let mut map: HashMap<String, InterfaceInfo> = HashMap::new();

        // SAFETY: getifaddrs is a standard POSIX call. We free the list when done.
        unsafe {
            let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
            if libc::getifaddrs(&mut addrs) != 0 {
                return Err(std::io::Error::last_os_error());
            }

            let mut cur = addrs;
            while !cur.is_null() {
                let ifa = &*cur;
                let name = std::ffi::CStr::from_ptr(ifa.ifa_name)
                    .to_string_lossy()
                    .into_owned();

                if is_interesting(&name) {
                    let up = (ifa.ifa_flags as i32 & libc::IFF_UP) != 0;
                    let entry = map.entry(name.clone()).or_insert_with(|| InterfaceInfo {
                        name: name.clone(),
                        up,
                        addrs: Vec::new(),
                    });
                    // Update up flag — any record saying up wins.
                    if up {
                        entry.up = true;
                    }

                    if !ifa.ifa_addr.is_null() {
                        let sa_family = (*ifa.ifa_addr).sa_family as i32;
                        if sa_family == libc::AF_INET {
                            let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                            let ip = IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(
                                sin.sin_addr.s_addr,
                            )));
                            entry.addrs.push(ip);
                        } else if sa_family == libc::AF_INET6 {
                            let sin6 = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                            let ip = IpAddr::V6(std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr));
                            entry.addrs.push(ip);
                        }
                    }
                }

                cur = ifa.ifa_next;
            }

            libc::freeifaddrs(addrs);
        }

        let have_v4 = map
            .values()
            .any(|i| i.up && i.addrs.iter().any(|a| a.is_ipv4() && is_routable(a)));
        let have_v6 = map
            .values()
            .any(|i| i.up && i.addrs.iter().any(|a| a.is_ipv6() && is_routable(a)));

        let default_route_interface = detect_default_route();

        Ok(Self {
            interfaces: map,
            default_route_interface,
            have_v4,
            have_v6,
        })
    }

    /// Compute what changed between `self` (old) and `new`.
    pub fn diff(&self, new: &InterfaceState) -> ChangeDelta {
        let default_interface_changed = self.default_route_interface != new.default_route_interface;

        let interface_ips_changed = self.ips_differ(new);

        let rebind_likely_required = default_interface_changed || interface_ips_changed;

        ChangeDelta {
            default_interface_changed,
            interface_ips_changed,
            rebind_likely_required,
        }
    }

    fn ips_differ(&self, new: &InterfaceState) -> bool {
        if self.interfaces.len() != new.interfaces.len() {
            return true;
        }
        for (name, old_info) in &self.interfaces {
            let Some(new_info) = new.interfaces.get(name) else {
                return true;
            };
            if old_info.up != new_info.up {
                return true;
            }
            let mut old_addrs = old_info.addrs.clone();
            let mut new_addrs = new_info.addrs.clone();
            old_addrs.sort_by_key(|a| a.to_string());
            new_addrs.sort_by_key(|a| a.to_string());
            if old_addrs != new_addrs {
                return true;
            }
        }
        false
    }
}

/// Detect the default-route interface.
///
/// Platform-specific: BSDs (incl. macOS) use `route -n get default`; Linux
/// reads `/proc/net/route`. Other platforms return `None`.
#[cfg(target_os = "macos")]
fn detect_default_route() -> Option<String> {
    let output = std::process::Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(iface) = trimmed.strip_prefix("interface:") {
            return Some(iface.trim().to_owned());
        }
    }
    None
}

/// On Linux, scan `/proc/net/route` for the first row whose destination and
/// mask are both 0.0.0.0 — that's the default route. Falls back to `None` on
/// read error.
#[cfg(target_os = "linux")]
fn detect_default_route() -> Option<String> {
    let contents = std::fs::read_to_string("/proc/net/route").ok()?;
    // Header: Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT
    for line in contents.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let iface = fields.next()?;
        let dest = fields.next()?;
        let _gateway = fields.next()?;
        let _flags = fields.next()?;
        let _refcnt = fields.next()?;
        let _use = fields.next()?;
        let _metric = fields.next()?;
        let mask = fields.next()?;
        if dest == "00000000" && mask == "00000000" {
            return Some(iface.to_owned());
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn detect_default_route() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_returns_interfaces() {
        let state = InterfaceState::snapshot().expect("snapshot should succeed");
        assert!(
            !state.interfaces.is_empty(),
            "expected at least one network interface"
        );
    }

    #[test]
    fn test_diff_identical() {
        let state = InterfaceState::snapshot().expect("snapshot should succeed");
        let delta = state.diff(&state);
        assert!(!delta.default_interface_changed);
        assert!(!delta.interface_ips_changed);
        assert!(!delta.rebind_likely_required);
    }

    #[test]
    fn test_diff_ip_added() {
        let state = InterfaceState::snapshot().expect("snapshot should succeed");
        let mut new_state = state.clone();
        // Add a fake IP to the first interface.
        if let Some(info) = new_state.interfaces.values_mut().next() {
            info.addrs
                .push(IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, 1)));
        }
        let delta = state.diff(&new_state);
        assert!(delta.interface_ips_changed);
        assert!(delta.rebind_likely_required);
    }

    #[test]
    fn test_diff_default_route_changed() {
        let state = InterfaceState::snapshot().expect("snapshot should succeed");
        let mut new_state = state.clone();
        new_state.default_route_interface = Some("fake99".to_owned());
        let delta = state.diff(&new_state);
        assert!(delta.default_interface_changed);
        assert!(delta.rebind_likely_required);
    }

    #[test]
    fn test_uninteresting_filter() {
        let state = InterfaceState::snapshot().expect("snapshot should succeed");
        for name in state.interfaces.keys() {
            assert!(
                is_interesting(name),
                "interface {name} should have been filtered"
            );
        }
    }
}
