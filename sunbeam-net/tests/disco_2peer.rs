//! 2-peer disco direct-path integration test.
//!
//! Exercises the Ping/Pong handshake end-to-end over an in-memory UDP
//! simulation: peer A starts with no known direct endpoint for peer B,
//! emits a sealed disco Ping toward a candidate address, peer B decrypts
//! it and replies with a sealed Pong, and peer A's `EndpointTracker`
//! transitions `best_addr` from `None` to `Some(candidate)`.
//!
//! No real sockets — packets flow through `tokio::sync::mpsc` channels.
//!
//! ## Note on production-code access
//!
//! The `daemon::endpoint` module and its `EndpointTracker` struct were
//! `pub(crate)` at the time this test was written. They've been promoted
//! to `#[doc(hidden)] pub` (the minimum needed for this integration test
//! target to reach them). No behavioural changes.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use crypto_box::aead::OsRng;
use crypto_box::{PublicKey, SalsaBox, SecretKey};
use tokio::sync::mpsc;
use tokio::time::timeout;

use sunbeam_net::daemon::endpoint::EndpointTracker;
use sunbeam_net::disco;

/// A simulated peer participating in the disco handshake.
struct Peer {
    name: &'static str,
    disco_pub: [u8; 32],
    /// sender_disco_pub -> precomputed SalsaBox
    shared: HashMap<[u8; 32], SalsaBox>,
    tracker: EndpointTracker,
    /// "Bound" address for this peer — the src address its outgoing packets
    /// appear to come from, as seen by the dispatcher / peer on the other end.
    bound_addr: SocketAddr,
    /// Outbound queue: (dst_addr, bytes). The dispatcher drains this.
    out_tx: mpsc::Sender<(SocketAddr, Vec<u8>)>,
    out_rx: mpsc::Receiver<(SocketAddr, Vec<u8>)>,
    /// Inbound queue: (src_addr, bytes). The dispatcher pushes to this.
    in_tx: mpsc::Sender<(SocketAddr, Vec<u8>)>,
    in_rx: mpsc::Receiver<(SocketAddr, Vec<u8>)>,
}

impl Peer {
    fn new(name: &'static str, disco_pub: [u8; 32], bound_addr: SocketAddr) -> Self {
        let (out_tx, out_rx) = mpsc::channel(32);
        let (in_tx, in_rx) = mpsc::channel(32);
        Self {
            name,
            disco_pub,
            shared: HashMap::new(),
            tracker: EndpointTracker::new(),
            bound_addr,
            out_tx,
            out_rx,
            in_tx,
            in_rx,
        }
    }

    /// Seal `msg` for the given recipient and push onto our outbound queue.
    async fn send_disco(&self, peer_pub: &[u8; 32], dst: SocketAddr, msg: disco::Message) {
        let shared = self
            .shared
            .get(peer_pub)
            .expect("missing shared key for peer");
        let sealed = disco::seal(&msg, &self.disco_pub, shared);
        self.out_tx
            .send((dst, sealed))
            .await
            .expect("outbound channel closed");
    }

    /// Decrypt an inbound packet using our shared-keys map.
    fn open(&self, pkt: &[u8]) -> Option<(disco::Message, [u8; 32])> {
        disco::open(pkt, &self.shared)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disco_two_peer_ping_pong_populates_best_addr() {
    let run = async {
        // ---- Keypairs ----
        let sk_a = SecretKey::generate(&mut OsRng);
        let pk_a: PublicKey = sk_a.public_key();
        let sk_b = SecretKey::generate(&mut OsRng);
        let pk_b: PublicKey = sk_b.public_key();
        let a_pub: [u8; 32] = *pk_a.as_bytes();
        let b_pub: [u8; 32] = *pk_b.as_bytes();

        // Addresses each peer appears to send from. The "candidate" A probes
        // for B is B's bound address. In a real run this comes from
        // STUN/netmap; here we hand it in directly.
        let a_bound: SocketAddr = "127.0.0.1:41641".parse().expect("parse A bound");
        let b_bound: SocketAddr = "127.0.0.1:9999".parse().expect("parse B bound");

        let mut peer_a = Peer::new("A", a_pub, a_bound);
        let mut peer_b = Peer::new("B", b_pub, b_bound);

        // Precompute NaCl shared keys for each direction, keyed by the
        // sender's disco public key (this mirrors `rebuild_disco_shared`
        // in daemon::lifecycle).
        peer_a.shared.insert(b_pub, SalsaBox::new(&pk_b, &sk_a));
        peer_b.shared.insert(a_pub, SalsaBox::new(&pk_a, &sk_b));

        // ---- Precondition: best_addr is None on both sides ----
        assert_eq!(
            peer_a.tracker.best_addr(&b_pub),
            None,
            "A must start with no known direct addr for B"
        );
        assert_eq!(
            peer_b.tracker.best_addr(&a_pub),
            None,
            "B must start with no known direct addr for A"
        );

        // ---- A starts probing B via a single candidate ----
        let candidate = b_bound;
        let pings = peer_a.tracker.start_probing(&b_pub, vec![candidate]);
        assert_eq!(pings.len(), 1, "expected exactly one ping for one candidate");
        let (ping_dst, ping) = pings
            .into_iter()
            .next()
            .expect("start_probing returned empty despite len check");
        assert_eq!(ping_dst, candidate);

        // A seals and queues the Ping for B.
        peer_a
            .send_disco(&b_pub, ping_dst, disco::Message::Ping(ping))
            .await;

        // ---- Dispatcher: drain A.out -> B.in ----
        let (a_dst, a_pkt) = peer_a
            .out_rx
            .recv()
            .await
            .expect("A did not emit a packet");
        assert_eq!(a_dst, b_bound, "A's ping should target B's bound addr");
        peer_b
            .in_tx
            .send((a_bound, a_pkt))
            .await
            .expect("B inbound channel closed");

        // ---- B receives, decrypts, responds ----
        let (src_from_a, pkt_from_a) = peer_b
            .in_rx
            .recv()
            .await
            .expect("B did not receive a packet");
        assert_eq!(src_from_a, a_bound, "B should see the ping as coming from A's addr");

        let (msg, sender_pub) = peer_b
            .open(&pkt_from_a)
            .expect("B failed to open sealed disco packet from A");
        assert_eq!(sender_pub, a_pub, "sealed packet must identify A as sender");

        let ping = match msg {
            disco::Message::Ping(p) => p,
            other => panic!("expected Ping, got {other:?}"),
        };

        // B echoes the src it observed, mirroring daemon::lifecycle::handle_disco.
        let pong = disco::Message::Pong(disco::Pong {
            tx_id: ping.tx_id,
            src: src_from_a,
        });
        peer_b.send_disco(&a_pub, src_from_a, pong).await;

        // ---- Dispatcher: drain B.out -> A.in ----
        let (b_dst, b_pkt) = peer_b
            .out_rx
            .recv()
            .await
            .expect("B did not emit a pong");
        assert_eq!(b_dst, a_bound, "B's pong should target A's bound addr");
        peer_a
            .in_tx
            .send((b_bound, b_pkt))
            .await
            .expect("A inbound channel closed");

        // ---- A receives, decrypts, hands to tracker ----
        let (src_from_b, pkt_from_b) = peer_a
            .in_rx
            .recv()
            .await
            .expect("A did not receive a packet");
        assert_eq!(src_from_b, b_bound);

        let (msg, sender_pub) = peer_a
            .open(&pkt_from_b)
            .expect("A failed to open sealed disco packet from B");
        assert_eq!(sender_pub, b_pub, "sealed packet must identify B as sender");

        let pong = match msg {
            disco::Message::Pong(p) => p,
            other => panic!("expected Pong, got {other:?}"),
        };

        let best = peer_a
            .tracker
            .handle_pong(&b_pub, &pong.tx_id, pong.src)
            .expect("handle_pong should return the newly-selected best addr");

        // ---- Postcondition: best_addr transitioned None -> Some(candidate) ----
        assert_eq!(
            best, candidate,
            "handle_pong should pin the candidate A originally probed"
        );
        assert_eq!(
            peer_a.tracker.best_addr(&b_pub),
            Some(candidate),
            "A's tracker must now report the candidate as the best direct addr for B"
        );

        // Silence unused-field lints without leaking them to warnings=deny builds.
        let _ = (peer_a.name, peer_b.name, peer_a.bound_addr);
    };

    timeout(Duration::from_secs(5), run)
        .await
        .expect("2-peer disco handshake timed out (5s budget)");
}
