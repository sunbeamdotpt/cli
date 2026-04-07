use crate::keys::NodeKeys;
use crate::proto::types::{AuthInfo, HostInfo, RegisterRequest, RegisterResponse};

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
            auth: Some(AuthInfo {
                auth_key: Some(auth_key.to_string()),
            }),
            hostinfo: build_hostinfo(hostname),
            followup: None,
            timestamp: None,
        };

        self.post_json("/machine/register", &req).await
    }
}

/// Build a [`HostInfo`] for the current platform.
pub(crate) fn build_hostinfo(hostname: &str) -> HostInfo {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_hostinfo() {
        let hi = build_hostinfo("myhost");
        assert_eq!(hi.hostname, "myhost");
        assert!(!hi.go_arch.is_empty(), "go_arch should be set");
        assert!(!hi.go_os.is_empty(), "go_os should be set");
        assert!(
            hi.go_version.starts_with("sunbeam-net/"),
            "go_version should start with sunbeam-net/"
        );
        assert!(!hi.os.is_empty(), "os should be set");
    }

    #[test]
    fn test_register_request_construction() {
        let keys = crate::keys::NodeKeys::generate();
        let req = RegisterRequest {
            version: 74,
            node_key: keys.node_key_str(),
            old_node_key: String::new(),
            auth: Some(AuthInfo {
                auth_key: Some("tskey-auth-test123".to_string()),
            }),
            hostinfo: build_hostinfo("test-host"),
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
