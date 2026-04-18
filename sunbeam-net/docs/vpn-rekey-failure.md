# VPN WireGuard Re-Handshake Failure

## Symptom

The VPN proxy at `127.0.0.1:16579` works for ~2-3 minutes after daemon startup, then silently stops forwarding traffic. Restarting the daemon temporarily fixes it. The cycle repeats every time.

## What works

- Initial WG handshake succeeds (type 1 → type 2 exchange).
- Data flows (type 4 TransportData packets) for the first ~2 minutes.
- DERP relay connection is established and delivers packets.
- smoltcp ↔ engine ↔ WG encap/decap pipeline is correct.
- Target resolution (10.43.0.1:443), NetworkPolicy, and Headscale ACLs are all fixed and working.

## What breaks

boringtun's WG session keys expire after ~2 minutes (standard WG `REKEY_AFTER_TIME`). When the daemon tries to re-handshake:

1. `tunnel.tick()` calls `update_timers()` on the boringtun `Tunn`, which produces a 148-byte HandshakeInitiation (type 1).
2. `dispatch_encap` sends this via both DERP and UDP.
3. The peer (Tailscale subnet router) **never responds**. No type 2 HandshakeResponse comes back.
4. The daemon retries every ~5 seconds (boringtun's internal retry timer).
5. DERP relay only delivers type 0x06 (KeepAlive) frames after the initial session — no type 0x05 (RecvPacket) carrying a HandshakeResponse.
6. `decap → response` count stays at 1 (the initial handshake) for the entire session.

## Root causes (ranked by likelihood)

### 1. `map_stream_loop` never feeds peer updates to `WgTunnel` (CRITICAL)

**File:** `sunbeam-net/src/daemon/lifecycle.rs:893-930`

The `map_stream_loop` function reads netmap updates (`Full`, `PeersChanged`, `PeersRemoved`) and updates the `RouteTable`, but it **never calls `wg_tunnel.update_peers()`**. The `WgTunnel` is only populated once at startup (line 246) and then moved into `run_wg_loop` (line 480-492) where it's inaccessible to `map_stream_loop`.

This means:
- If the peer restarts and gets a new WG public key, we're encrypting to a stale key.
- If the peer's DERP region or endpoint changes, we're sending to the wrong place.
- If Headscale rotates the peer's node key (which it does periodically), our handshake initiations are addressed to a key the peer no longer holds.

**The fix must bridge netmap peer updates from `map_stream_loop` into `run_wg_loop`.** A channel-based approach (e.g., `mpsc::Sender<Vec<Node>>` from `map_stream_loop` → `run_wg_loop`) is the natural pattern here. `run_wg_loop` already does `tokio::select!` over multiple channels, so adding one more branch is straightforward.

### 2. `handle_decap` only sends responses via DERP, not UDP (LIKELY CONTRIBUTING)

**File:** `sunbeam-net/src/daemon/lifecycle.rs:688-705`

When `decapsulate()` returns `DecapAction::Response` (i.e., a HandshakeResponse we need to send back), `handle_decap` sends it only via `derp_out_tx`. It does **not** send via `udp_out_tx`, even though the initiating packet may have arrived via UDP.

This is asymmetric: `dispatch_encap` sends over both transports, but `handle_decap` only uses DERP for responses. If the peer's re-handshake initiation arrives via UDP and we reply only via DERP, the peer may not correlate them (especially if the peer prefers direct UDP and isn't expecting a DERP reply to a UDP-initiated handshake).

**Fix:** `handle_decap` should also dispatch responses via UDP when the packet arrived over UDP. This requires either:
- Passing `udp_out_tx` into `handle_decap` and knowing the source transport.
- Or wrapping the response in an `EncapAction` and using `dispatch_encap` (which sends both).

### 3. DERP relay may lose our peer mapping (POSSIBLE)

The DERP relay tracks which clients are connected by their node key. If the relay drops our session (timeout, reconnect, etc.), it stops forwarding packets addressed to us. The `run_derp_loop` (line 793-831) has **no reconnection logic** — if `recv_packet()` returns an error, the loop exits and the DERP task dies silently. There's no heartbeat or keep-alive from our side either (we respond to `FRAME_PING` but don't initiate).

**Evidence:** After the initial session, only type 0x06 (KeepAlive) frames arrive — no type 0x05 (RecvPacket). This is consistent with the relay still being connected but no longer routing peer data to us, possibly because the peer can't address us (see root cause 1 — our `Node.DERP` field may go stale).

**Fix:** Add DERP reconnection logic in `run_derp_loop`, or propagate the error up so `run_session` restarts.

### 4. boringtun re-handshake and Tailscale disco protocol mismatch (POSSIBLE)

The Tailscale subnet router (v1.76.6) uses the "disco" protocol for endpoint discovery. Raw boringtun handshake initiations may be ignored by the Tailscale peer if it expects them to be wrapped in disco frames. However, this theory is weakened by the fact that the **initial** handshake works — if disco wrapping were required, nothing would work at all.

A subtler variant: Tailscale may accept bare WG handshakes initially but switch to disco-only mode after the first session, particularly after a DERP region change or endpoint update.

**This is the lowest-priority theory** — investigate only after fixes 1-3 are applied.

## Architecture of the data path

```
Local TCP (127.0.0.1:16579)
  → NetworkEngine (smoltcp virtual TCP/IP stack)
    → engine_to_wg_rx channel
      → run_wg_loop: tunnel.encapsulate(dst_ip, packet)
        → dispatch_encap: send via DERP and/or UDP
          → DERP relay (headscale.sunbeam.pt:8443)
            → Tailscale subnet router pod
              → kube-proxy DNAT → k8s API (10.43.0.1:443)

Return path:
  DERP relay / UDP
    → run_wg_loop: tunnel.decapsulate(peer_key, data)
      → to_engine channel
        → NetworkEngine (smoltcp reassembly)
          → local TCP socket
```

## Key files

| File | What to look at |
|---|---|
| `sunbeam-net/src/daemon/lifecycle.rs:893-930` | `map_stream_loop` — needs to forward peer updates to WgTunnel |
| `sunbeam-net/src/daemon/lifecycle.rs:605-663` | `run_wg_loop` — needs a new channel arm for peer updates |
| `sunbeam-net/src/daemon/lifecycle.rs:688-705` | `handle_decap` — responses only go via DERP, not UDP |
| `sunbeam-net/src/daemon/lifecycle.rs:793-831` | `run_derp_loop` — no reconnection on error |
| `sunbeam-net/src/wg/tunnel.rs:83-130` | `WgTunnel::update_peers` — already exists, just needs to be called |
| `sunbeam-net/src/wg/tunnel.rs:204-230` | `WgTunnel::tick` — timer-based keepalives and re-handshake |
| `sunbeam-net/src/wg/tunnel.rs:159-202` | `WgTunnel::decapsulate` — response chaining logic |

## Suggested fix order

1. **Wire peer updates into `run_wg_loop`**: Add an `mpsc::Sender<Vec<Node>>` from `map_stream_loop` to `run_wg_loop`. In `run_wg_loop`, add a `select!` arm that calls `tunnel.update_peers(&new_peers)` when received. This is the most likely fix for the re-handshake failure.

2. **Make `handle_decap` send responses via both transports**: Either pass `udp_out_tx` into `handle_decap` or route responses through `dispatch_encap`.

3. **Add DERP reconnection**: When `run_derp_loop` exits due to error, signal the parent to reconnect rather than silently dying. At minimum, log at `error!` level so the failure is visible.

4. **Add diagnostic logging**: In `run_wg_loop`'s tick arm, log when `update_timers` produces a HandshakeInitiation (type 1, 148 bytes). In `handle_decap`, count and log HandshakeResponse events. This will confirm whether the re-handshake is being attempted and whether responses arrive.

## How to verify the fix

```bash
# Start the daemon
sunbeam vpn connect

# In another terminal, run a long-lived kubectl through the proxy
HTTPS_PROXY=http://127.0.0.1:16579 kubectl get pods -w

# Wait 5+ minutes. If the watch stays alive through at least one
# WG rekey cycle (~2 min), the fix works.

# Check logs for successful rekey:
# "decap → response (92 bytes, type=2)" should appear every ~2 min
```
