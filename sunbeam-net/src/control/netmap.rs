use bytes::{Buf, Bytes};
use h2::RecvStream;

use crate::proto::types::{DerpMap, MapRequest, MapResponse, Node};

/// A streaming reader for map updates from the coordination server.
///
/// The map protocol uses HTTP/2 server streaming: the client sends a single
/// `MapRequest` and the server responds with a sequence of length-prefixed
/// JSON messages on the same response body.
///
/// `MapStream` takes ownership of the originating [`ControlClient`] via
/// `_client_keep_alive`. This is load-bearing: `h2` tracks active
/// `SendRequest` handles on the connection, and once the last one is
/// dropped the client issues a graceful `GoAway` and tears the
/// connection down, which closes *every* stream including this one.
/// Holding the `ControlClient` here pins the `SendRequest` (and the
/// connection driver task it owns) for the lifetime of the map stream.
pub struct MapStream {
    body: RecvStream,
    buf: bytes::BytesMut,
    /// Whether we have received the first (full) message yet.
    first: bool,
    /// Pin the h2 connection open for as long as we're reading updates.
    /// Not read directly — the field exists purely for its Drop timing.
    _client_keep_alive: super::client::ControlClient,
}

impl std::fmt::Debug for MapStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapStream")
            .field("buffered", &self.buf.len())
            .field("first", &self.first)
            .finish()
    }
}

/// Parsed map update.
#[derive(Debug)]
pub enum MapUpdate {
    /// First response: full network map.
    Full {
        self_node: Node,
        peers: Vec<Node>,
        derp_map: Option<DerpMap>,
    },
    /// Delta: peers changed.
    PeersChanged(Vec<Node>),
    /// Delta: peers removed (by node key).
    PeersRemoved(Vec<String>),
    /// Keep-alive (empty response).
    KeepAlive,
}

impl MapStream {
    /// Read the next map update from the stream.
    ///
    /// Protocol: each update is prefixed with a 4-byte little-endian `u32`
    /// length, followed by the message body. The first message is raw JSON;
    /// subsequent messages are zstd-compressed JSON.
    pub async fn next(&mut self) -> crate::Result<Option<MapUpdate>> {
        // Ensure we have at least 4 bytes for the length prefix.
        while self.buf.len() < 4 {
            match self.body.data().await {
                Some(Ok(chunk)) => {
                    self.body
                        .flow_control()
                        .release_capacity(chunk.len())
                        .map_err(|e| {
                            crate::Error::Control(format!("flow control: {e}"))
                        })?;
                    self.buf.extend_from_slice(&chunk);
                }
                Some(Err(e)) => {
                    return Err(crate::Error::Control(format!("read stream: {e}")));
                }
                None => return Ok(None), // Stream ended.
            }
        }

        // Read the 4-byte LE length prefix.
        let msg_len = u32::from_le_bytes([
            self.buf[0],
            self.buf[1],
            self.buf[2],
            self.buf[3],
        ]) as usize;
        self.buf.advance(4);

        // Read the full message body.
        while self.buf.len() < msg_len {
            match self.body.data().await {
                Some(Ok(chunk)) => {
                    self.body
                        .flow_control()
                        .release_capacity(chunk.len())
                        .map_err(|e| {
                            crate::Error::Control(format!("flow control: {e}"))
                        })?;
                    self.buf.extend_from_slice(&chunk);
                }
                Some(Err(e)) => {
                    return Err(crate::Error::Control(format!("read stream: {e}")));
                }
                None => {
                    return Err(crate::Error::Control(format!(
                        "stream ended mid-message (need {msg_len} bytes, have {})",
                        self.buf.len()
                    )));
                }
            }
        }

        let raw = self.buf.split_to(msg_len);
        let is_first = self.first;
        if self.first {
            self.first = false;
        }

        // Detect zstd compression by magic bytes (0x28 0xB5 0x2F 0xFD).
        // Headscale only zstd-compresses if the client requested it via the
        // `Compress` field in MapRequest; otherwise messages are raw JSON.
        let json_bytes = if raw.len() >= 4 && raw[0] == 0x28 && raw[1] == 0xB5 && raw[2] == 0x2F && raw[3] == 0xFD {
            zstd::stream::decode_all(raw.as_ref()).map_err(|e| {
                crate::Error::Control(format!("zstd decompress: {e}"))
            })?
        } else {
            raw.to_vec()
        };

        // Preview the whole JSON at trace level. At debug level we only
        // log sizes + which fields are populated so logs don't drown in
        // netmap bytes under normal load.
        if tracing::enabled!(tracing::Level::TRACE) {
            tracing::trace!(
                "map frame: {} bytes json, body: {}",
                json_bytes.len(),
                String::from_utf8_lossy(&json_bytes)
            );
        }

        let resp: MapResponse = serde_json::from_slice(&json_bytes)?;
        tracing::debug!(
            "map fields present: node={} peers={} peers_changed={} peers_removed={} derp_map={}",
            resp.node.is_some(),
            resp.peers.is_some(),
            resp.peers_changed.is_some(),
            resp.peers_removed.is_some(),
            resp.derp_map.is_some(),
        );
        Ok(Some(classify_response(resp, is_first)))
    }
}

/// Classify a [`MapResponse`] into a [`MapUpdate`] variant based on which
/// fields are populated.
///
/// Only the first frame in a stream is a full snapshot — Headscale always
/// includes `Node` in that frame and usually (but not always) includes
/// `Peers`. Subsequent frames are deltas: peer-change deltas carry
/// `PeersChanged`/`PeersRemoved`, and self-info deltas (LastSeen bumps,
/// tag updates, cap version changes) carry only `Node` with no peer
/// fields. Treating those self-info frames as "full snapshot with zero
/// peers" wipes the route table, so post-first `Node`-only frames are
/// mapped to `KeepAlive` here.
fn classify_response(resp: MapResponse, is_first: bool) -> MapUpdate {
    if is_first {
        if let Some(self_node) = resp.node {
            return MapUpdate::Full {
                self_node,
                peers: resp.peers.unwrap_or_default(),
                derp_map: resp.derp_map,
            };
        }
    }

    // Delta: peers changed.
    if let Some(changed) = resp.peers_changed {
        return MapUpdate::PeersChanged(changed);
    }

    // Delta: peers removed.
    if let Some(removed) = resp.peers_removed {
        return MapUpdate::PeersRemoved(removed);
    }

    // Post-first frames that only carry our own Node (LastSeen/tag/cap
    // updates) aren't a full resync — treat them as a keep-alive so the
    // daemon doesn't clobber the peer table.
    MapUpdate::KeepAlive
}

impl super::client::ControlClient {
    /// Start a map streaming session.
    ///
    /// Sends a `MapRequest` to `POST /machine/map` and returns a [`MapStream`]
    /// that yields successive [`MapUpdate`]s as the coordination server pushes
    /// network map changes.
    /// Send a non-streaming "Lite endpoint update" to Headscale so it
    /// persists our DiscoKey on the node record. Headscale's
    /// `serveLongPoll()` (the streaming map handler) doesn't update
    /// DiscoKey for capability versions ≥ 68 — only the Lite update path
    /// does, gated on `Stream: false` + `OmitPeers: true` + `ReadOnly: false`.
    /// Without this our peers see us as having a zero disco_key and never
    /// add us to their netmaps.
    pub async fn lite_update(
        &mut self,
        keys: &crate::keys::NodeKeys,
        hostname: &str,
        endpoints: Option<Vec<String>>,
        preferred_derp: Option<u32>,
    ) -> crate::Result<()> {
        let req = MapRequest {
            version: 74,
            node_key: keys.node_key_str(),
            disco_key: keys.disco_key_str(),
            stream: false,
            omit_peers: true,
            read_only: false,
            hostinfo: super::register::build_hostinfo(hostname, preferred_derp),
            endpoints,
        };
        // Lite update returns an empty body.
        self.post_json_no_response("/machine/map", &req).await
    }

    /// Start the map streaming session and return a `MapStream` that owns
    /// the `ControlClient` — dropping the client early would close the h2
    /// connection and end the stream, so the client is pinned inside the
    /// returned `MapStream` until the caller is done reading.
    pub async fn map_stream(
        mut self,
        keys: &crate::keys::NodeKeys,
        hostname: &str,
        preferred_derp: Option<u32>,
    ) -> crate::Result<MapStream> {
        let req = MapRequest {
            version: 74,
            node_key: keys.node_key_str(),
            disco_key: keys.disco_key_str(),
            stream: true,
            omit_peers: false,
            read_only: false,
            hostinfo: super::register::build_hostinfo(hostname, preferred_derp),
            endpoints: None,
        };

        let body_bytes = serde_json::to_vec(&req)?;

        let request = http::Request::builder()
            .method("POST")
            .uri("/machine/map")
            .header("content-type", "application/json")
            .body(())
            .map_err(|e| crate::Error::Control(e.to_string()))?;

        let (response_future, mut send_stream) = self
            .sender
            .send_request(request, false)
            .map_err(|e| crate::Error::Control(format!("send map request: {e}")))?;

        send_stream
            .send_data(Bytes::from(body_bytes), true)
            .map_err(|e| crate::Error::Control(format!("send map body: {e}")))?;

        let response = response_future
            .await
            .map_err(|e| crate::Error::Control(format!("map response: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(crate::Error::Control(format!(
                "/machine/map returned {status}"
            )));
        }

        let body = response.into_body();

        Ok(MapStream {
            body,
            buf: bytes::BytesMut::new(),
            first: true,
            _client_keep_alive: self,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::types::HostInfo;

    fn sample_node(id: u64, key: &str) -> Node {
        Node {
            id,
            key: key.to_string(),
            disco_key: format!("discokey:{key}"),
            addresses: vec!["100.64.0.1/32".to_string()],
            allowed_ips: vec!["100.64.0.0/10".to_string()],
            endpoints: vec![],
            derp: "127.3.3.40:1".to_string(),
            hostinfo: HostInfo {
                go_arch: "amd64".to_string(),
                go_os: "linux".to_string(),
                go_version: "sunbeam-net/0.1.0".to_string(),
                hostname: "test".to_string(),
                os: "linux".to_string(),
                os_version: String::new(),
                device_model: None,
                frontend_log_id: None,
                backend_log_id: None,
                net_info: None,
            },
            name: "test.example.com".to_string(),
            online: Some(true),
            machine_authorized: true,
        }
    }

    #[test]
    fn test_map_update_classify_full() {
        let resp = MapResponse {
            node: Some(sample_node(1, "nodekey:aa")),
            peers: Some(vec![sample_node(2, "nodekey:bb")]),
            peers_changed: None,
            peers_removed: None,
            derp_map: None,
            dns_config: None,
            packet_filter: None,
            domain: Some("example.com".to_string()),
            collection_name: None,
        };

        let update = classify_response(resp, true);
        match update {
            MapUpdate::Full {
                self_node,
                peers,
                derp_map,
            } => {
                assert_eq!(self_node.id, 1);
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0].id, 2);
                assert!(derp_map.is_none());
            }
            other => panic!("expected Full, got {other:?}"),
        }
    }

    #[test]
    fn test_map_update_classify_full_with_no_peers() {
        // Regression for prod bug #35: Headscale serves a full snapshot
        // with `Peers` omitted (null) when the tailnet has no other
        // nodes yet. We still need to treat it as Full so the daemon
        // can reach Ready state instead of reconnecting forever.
        let resp = MapResponse {
            node: Some(sample_node(1, "nodekey:aa")),
            peers: None,
            peers_changed: None,
            peers_removed: None,
            derp_map: None,
            dns_config: None,
            packet_filter: None,
            domain: None,
            collection_name: None,
        };
        match classify_response(resp, true) {
            MapUpdate::Full { peers, self_node, .. } => {
                assert_eq!(self_node.id, 1);
                assert!(peers.is_empty());
            }
            other => panic!("expected Full with empty peers, got {other:?}"),
        }
    }

    #[test]
    fn test_map_update_post_first_node_only_is_keepalive() {
        // Regression for prod bug #35: after the first full snapshot,
        // Headscale sends self-info deltas (LastSeen bumps, tag updates,
        // SrcIPs refreshes) containing only `Node` and no peer fields.
        // Treating those as Full with empty peers wipes the route table
        // and leaves the daemon convinced there are zero peers even
        // though the subnet router is still online.
        let resp = MapResponse {
            node: Some(sample_node(1, "nodekey:aa")),
            peers: None,
            peers_changed: None,
            peers_removed: None,
            derp_map: None,
            dns_config: None,
            packet_filter: None,
            domain: None,
            collection_name: None,
        };
        let update = classify_response(resp, false);
        assert!(matches!(update, MapUpdate::KeepAlive),
            "post-first node-only frame must not be reclassified as Full: got {update:?}");
    }

    #[test]
    fn test_map_update_classify_delta() {
        let resp = MapResponse {
            node: None,
            peers: None,
            peers_changed: Some(vec![sample_node(3, "nodekey:cc")]),
            peers_removed: None,
            derp_map: None,
            dns_config: None,
            packet_filter: None,
            domain: None,
            collection_name: None,
        };

        let update = classify_response(resp, false);
        match update {
            MapUpdate::PeersChanged(changed) => {
                assert_eq!(changed.len(), 1);
                assert_eq!(changed[0].id, 3);
            }
            other => panic!("expected PeersChanged, got {other:?}"),
        }
    }

    #[test]
    fn test_map_update_classify_peers_removed() {
        let resp = MapResponse {
            node: None,
            peers: None,
            peers_changed: None,
            peers_removed: Some(vec!["nodekey:dd".to_string()]),
            derp_map: None,
            dns_config: None,
            packet_filter: None,
            domain: None,
            collection_name: None,
        };

        let update = classify_response(resp, false);
        match update {
            MapUpdate::PeersRemoved(removed) => {
                assert_eq!(removed, vec!["nodekey:dd"]);
            }
            other => panic!("expected PeersRemoved, got {other:?}"),
        }
    }

    #[test]
    fn test_map_update_classify_keepalive() {
        let resp = MapResponse {
            node: None,
            peers: None,
            peers_changed: None,
            peers_removed: None,
            derp_map: None,
            dns_config: None,
            packet_filter: None,
            domain: None,
            collection_name: None,
        };

        let update = classify_response(resp, false);
        assert!(matches!(update, MapUpdate::KeepAlive));
    }
}
