use crate::keys::NodeKeys;
use crate::proto::types::{AuthInfo, HostInfo, NetInfo, RegisterRequest, RegisterResponse};

impl super::client::ControlClient {
    /// Register this node with the coordination server using a pre-auth key.
    ///
    /// Sends a `RegisterRequest` to `POST /machine/register` and returns the
    /// server's `RegisterResponse`, which includes the assigned addresses,
    /// user info, and machine authorization status.
    pub async fn register(
        &mut self,
        auth_key: &str,
        hostname: &str,
        keys: &NodeKeys,
    ) -> crate::Result<RegisterResponse> {
        let req = RegisterRequest {
            version: 74, // capability version
            node_key: keys.node_key_str(),
            old_node_key: format!("nodekey:{}", "0".repeat(64)),
            disco_key: keys.disco_key_str(),
            auth: Some(AuthInfo {
                auth_key: Some(auth_key.to_string()),
            }),
            hostinfo: build_hostinfo(hostname, None),
            followup: None,
            timestamp: None,
        };

        self.post_json("/machine/register", &req).await
    }
}

/// Build a [`HostInfo`] for the current platform.
///
/// `preferred_derp` populates `NetInfo.PreferredDERP`, which Headscale uses
/// to set our `Node.DERP` field and which peers consult when deciding which
/// DERP region to address relay traffic to. Pass `None` on the initial
/// register (we don't know the region yet) and `Some(region)` on later
/// lite-updates once the netmap has told us which DERP region to prefer.
pub(crate) fn build_hostinfo(hostname: &str, preferred_derp: Option<u32>) -> HostInfo {
    HostInfo {
        go_arch: std::env::consts::ARCH.to_string(),
        go_os: std::env::consts::OS.to_string(),
        go_version: format!("sunbeam-net/{}", env!("CARGO_PKG_VERSION")),
        hostname: hostname.to_string(),
        os: std::env::consts::OS.to_string(),
        os_version: String::new(),
        device_model: None,
        frontend_log_id: None,
        backend_log_id: None,
        net_info: preferred_derp.map(|region| NetInfo {
            preferred_derp: region,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_hostinfo() {
        let hi = build_hostinfo("myhost", None);
        assert_eq!(hi.hostname, "myhost");
        assert!(!hi.go_arch.is_empty(), "go_arch should be set");
        assert!(!hi.go_os.is_empty(), "go_os should be set");
        assert!(
            hi.go_version.starts_with("sunbeam-net/"),
            "go_version should start with sunbeam-net/"
        );
        assert!(!hi.os.is_empty(), "os should be set");
        assert!(hi.net_info.is_none(), "no NetInfo without preferred_derp");

        let hi2 = build_hostinfo("myhost", Some(999));
        assert_eq!(hi2.net_info.as_ref().unwrap().preferred_derp, 999);
    }

    #[test]
    fn test_register_request_construction() {
        let keys = crate::keys::NodeKeys::generate();
        let req = RegisterRequest {
            version: 74,
            node_key: keys.node_key_str(),
            old_node_key: String::new(),
            disco_key: keys.disco_key_str(),
            auth: Some(AuthInfo {
                auth_key: Some("tskey-auth-test123".to_string()),
            }),
            hostinfo: build_hostinfo("test-host", None),
            followup: None,
            timestamp: None,
        };

        let json = serde_json::to_string(&req).unwrap();

        // Verify key fields are present with expected values.
        assert!(json.contains("\"Version\":74"));
        assert!(json.contains(&format!("\"NodeKey\":\"{}\"", keys.node_key_str())));
        assert!(json.contains("\"AuthKey\":\"tskey-auth-test123\""));
        assert!(json.contains("\"Hostname\":\"test-host\""));

        // Round-trip.
        let parsed: RegisterRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 74);
        assert_eq!(parsed.node_key, keys.node_key_str());
    }
}
