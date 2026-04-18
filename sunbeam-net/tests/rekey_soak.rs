//! Rekey soak test — fast-forward WireGuard rekey ~100 times, verify
//! data keeps flowing across every rekey.
//!
//! # What this test catches
//!
//! A 24-hour real-time soak would exercise ~720 rekeys (one every ~2 min).
//! That's impractical for CI. This test drives 100 forced rekeys in a tight
//! in-memory loop between two `boringtun::noise::Tunn` instances, asserting
//! that:
//!
//! * every rekey completes (handshake init → response → keepalive → session
//!   ready) without decap failures on either side
//! * a burst of data packets encrypts and decrypts cleanly between rekeys
//! * payloads arrive byte-identical, proving the session keys on both sides
//!   agree after each rotation (regression guard for the key-desync bug
//!   this RC series chased)
//! * no `TunnResult::Err` leaks out — boringtun reports every packet as
//!   `WriteToTunnelV4` / `WriteToNetwork` / `Done`
//!
//! # How rekey is forced
//!
//! Boringtun's real rekey trigger is `REKEY_AFTER_TIME = 120s` (see
//! <https://github.com/cloudflare/boringtun/blob/master/boringtun/src/noise/timers.rs>).
//! Boringtun uses `std::time::Instant` directly (it only uses `mock_instant`
//! when compiled with its `mock-instant` cargo feature, which we don't
//! enable), so `tokio::time::pause()` does nothing to it.
//!
//! Instead we use boringtun's public Rust API
//! `Tunn::format_handshake_initiation(dst, force_resend = true)` which
//! generates a fresh type-1 handshake initiation regardless of timers. This
//! is the same code path `update_timers` takes when `REKEY_AFTER_TIME`
//! expires — we're just calling it directly.
//!
//! TAI64N replay protection in the responder requires strictly increasing
//! timestamps. `Instant::now()` advances by at least one nanosecond between
//! calls on every platform we support, so 100 iterations in a tight loop
//! are fine.
//!
//! # Running
//!
//! This test is `#[ignore]`d by default — it takes a few seconds and isn't
//! worth running on every `cargo test` invocation. Run it explicitly:
//!
//! ```sh
//! RUSTC_WRAPPER= cargo test -p sunbeam-net --test rekey_soak -- --ignored
//! ```
//!
//! To see progress logs:
//!
//! ```sh
//! RUSTC_WRAPPER= cargo test -p sunbeam-net --test rekey_soak -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use boringtun::noise::rate_limiter::RateLimiter;
use boringtun::noise::{Tunn, TunnResult};
use x25519_dalek::{PublicKey, StaticSecret};

/// Scratch buffer size — matches what the production wrapper uses.
const BUF_SIZE: usize = 65536;

/// Number of forced rekeys to perform.
const REKEY_ITERATIONS: usize = 100;

/// Number of data packets exchanged in each direction between rekeys.
const PACKETS_PER_ROUND: usize = 10;

/// A minimal IPv4 packet (20-byte header, no payload). boringtun inspects
/// the IP version nibble on inbound decap to decide between
/// `WriteToTunnelV4` and `WriteToTunnelV6` — sending garbage bytes makes it
/// bail out. This is the smallest well-formed IPv4 packet.
fn ipv4_packet(seed: u8) -> Vec<u8> {
    let mut p = vec![0u8; 20];
    p[0] = 0x45; // version = 4, IHL = 5 (20 bytes)
    p[1] = 0x00; // DSCP/ECN
    p[2] = 0x00;
    p[3] = 20; // total length = 20
    p[4] = seed; // identification byte (varies per packet so we can verify)
    p[8] = 64; // TTL
    p[9] = 0xfd; // protocol = unassigned / experimental (won't confuse anything)
    // src = 10.0.0.<seed>, dst = 10.0.0.<seed+1>
    p[12] = 10;
    p[13] = 0;
    p[14] = 0;
    p[15] = seed;
    p[16] = 10;
    p[17] = 0;
    p[18] = 0;
    p[19] = seed.wrapping_add(1);
    p
}

/// Make a fresh boringtun tunnel. `index` is the local session index that
/// boringtun uses to correlate inbound packets.
///
/// We pass a custom `RateLimiter` with a practically-infinite limit. In
/// production we pass `None`, which gives us boringtun's default limit of
/// `PEER_HANDSHAKE_RATE_LIMIT = 10` handshakes/sec — fine for real traffic
/// where rekeys are ~120s apart, but this test deliberately forces 100
/// rekeys as fast as possible and would trip the default limiter (seen as
/// `TunnResult::Err(UnderLoad)` on decap). Disabling it is a test-only
/// concern; the production wrapper stays unchanged.
fn make_tunn(
    my_secret: StaticSecret,
    my_public: PublicKey,
    peer_public: PublicKey,
    index: u32,
) -> Tunn {
    let rate_limiter = Arc::new(RateLimiter::new(&my_public, u64::MAX));
    Tunn::new(
        my_secret,
        peer_public,
        None, // no preshared key
        None, // no persistent keepalive — we drive traffic manually
        index,
        Some(rate_limiter),
    )
}

/// Deliver a single WG packet to `dst`, returning any chained responses
/// `dst` wants to send back. Drains chained WriteToNetwork via empty-input
/// decap calls, matching the production wrapper's behaviour.
///
/// Panics with iteration context on any decap error. Decrypted IPv4/IPv6
/// payloads are collected and returned so the caller can assert on them.
fn deliver(
    dst: &mut Tunn,
    packet: &[u8],
    ctx: &str,
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut responses = Vec::new();
    let mut decrypted = Vec::new();
    let mut buf = vec![0u8; BUF_SIZE];
    match dst.decapsulate(None, packet, &mut buf) {
        TunnResult::WriteToNetwork(data) => {
            responses.push(data.to_vec());
            let mut chain_buf = vec![0u8; BUF_SIZE];
            loop {
                match dst.decapsulate(None, &[], &mut chain_buf) {
                    TunnResult::WriteToNetwork(more) => {
                        responses.push(more.to_vec());
                    }
                    TunnResult::Done => break,
                    TunnResult::Err(e) => {
                        panic!("{ctx}: chained decap error: {e:?}");
                    }
                    TunnResult::WriteToTunnelV4(data, _) | TunnResult::WriteToTunnelV6(data, _) => {
                        decrypted.push(data.to_vec());
                    }
                }
            }
        }
        TunnResult::WriteToTunnelV4(data, _) => decrypted.push(data.to_vec()),
        TunnResult::WriteToTunnelV6(data, _) => decrypted.push(data.to_vec()),
        TunnResult::Done => {}
        TunnResult::Err(e) => {
            panic!("{ctx}: decap error: {e:?}");
        }
    }
    (responses, decrypted)
}

/// Drive a full handshake: initiator sends type-1, responder sends type-2,
/// initiator sends a keepalive (type-4, empty payload), responder consumes
/// it. After this returns, both sides have an active session.
fn handshake(a: &mut Tunn, b: &mut Tunn, ctx: &str) {
    // A: generate handshake initiation.
    let mut buf = vec![0u8; BUF_SIZE];
    let init = match a.format_handshake_initiation(&mut buf, true) {
        TunnResult::WriteToNetwork(d) => d.to_vec(),
        other => panic!("{ctx}: expected handshake init, got {other:?}"),
    };

    // B: receive init → should emit handshake response.
    let (responses, _dec) = deliver(b, &init, &format!("{ctx}: B<-A init"));
    assert_eq!(
        responses.len(),
        1,
        "{ctx}: expected 1 handshake response from B, got {}",
        responses.len()
    );
    let resp = &responses[0];

    // A: receive response → should emit a keepalive (type-4, empty data).
    let (keepalives, _dec) = deliver(a, resp, &format!("{ctx}: A<-B resp"));
    assert_eq!(
        keepalives.len(),
        1,
        "{ctx}: expected 1 keepalive from A after handshake response, got {}",
        keepalives.len()
    );
    let ka = &keepalives[0];

    // B: receive keepalive → should be Done.
    let (more, _dec) = deliver(b, ka, &format!("{ctx}: B<-A keepalive"));
    assert!(
        more.is_empty(),
        "{ctx}: unexpected output after keepalive: {} responses",
        more.len()
    );
}

/// Encapsulate `payload` on `src`, expecting a data packet (not a handshake).
/// Returns the encrypted bytes.
fn encap_data(src: &mut Tunn, payload: &[u8], ctx: &str) -> Vec<u8> {
    let mut buf = vec![0u8; BUF_SIZE];
    match src.encapsulate(payload, &mut buf) {
        TunnResult::WriteToNetwork(data) => data.to_vec(),
        other => panic!("{ctx}: expected data packet from encap, got {other:?}"),
    }
}

#[test]
#[ignore = "soak test — run with `cargo test --test rekey_soak -- --ignored`"]
fn rekey_soak_100_iterations() {
    // Real keys, real crypto. No mocks.
    let a_secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let b_secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let a_public = PublicKey::from(&a_secret);
    let b_public = PublicKey::from(&b_secret);

    let mut a = make_tunn(a_secret, a_public, b_public, 1);
    let mut b = make_tunn(b_secret, b_public, a_public, 2);

    let start = Instant::now();

    // Initial handshake.
    handshake(&mut a, &mut b, "initial handshake");

    // For each rekey iteration:
    //   1. Exchange PACKETS_PER_ROUND data packets in each direction.
    //   2. Alternate the initiator (A on even iterations, B on odd) so we
    //      exercise both roles.
    //   3. Force a fresh handshake.
    //   4. After rekey, exchange a couple more packets to prove the new
    //      session works.
    for i in 0..REKEY_ITERATIONS {
        // Data flow A → B.
        for j in 0..PACKETS_PER_ROUND {
            let seed = ((i * PACKETS_PER_ROUND + j) & 0xff) as u8;
            let plaintext = ipv4_packet(seed);
            let ciphertext = encap_data(&mut a, &plaintext, &format!("iter {i} pkt {j} A->B encap"));
            let (resp, dec) = deliver(&mut b, &ciphertext, &format!("iter {i} pkt {j} A->B decap"));
            assert!(
                resp.is_empty(),
                "iter {i} pkt {j} A->B: unexpected {} network response(s) from data decap",
                resp.len()
            );
            assert_eq!(
                dec.len(),
                1,
                "iter {i} pkt {j} A->B: expected 1 decrypted packet, got {}",
                dec.len()
            );
            assert_eq!(
                dec[0], plaintext,
                "iter {i} pkt {j} A->B: payload mismatch after decap (session key desync?)"
            );
        }

        // Data flow B → A.
        for j in 0..PACKETS_PER_ROUND {
            let seed = (((i * PACKETS_PER_ROUND + j) ^ 0x80) & 0xff) as u8;
            let plaintext = ipv4_packet(seed);
            let ciphertext = encap_data(&mut b, &plaintext, &format!("iter {i} pkt {j} B->A encap"));
            let (resp, dec) = deliver(&mut a, &ciphertext, &format!("iter {i} pkt {j} B->A decap"));
            assert!(
                resp.is_empty(),
                "iter {i} pkt {j} B->A: unexpected {} network response(s) from data decap",
                resp.len()
            );
            assert_eq!(
                dec.len(),
                1,
                "iter {i} pkt {j} B->A: expected 1 decrypted packet, got {}",
                dec.len()
            );
            assert_eq!(
                dec[0], plaintext,
                "iter {i} pkt {j} B->A: payload mismatch after decap (session key desync?)"
            );
        }

        // Force a rekey. Alternate which side initiates so we exercise both
        // the original-initiator and role-reversal code paths.
        let ctx = format!("iter {i} rekey");
        if i % 2 == 0 {
            handshake(&mut a, &mut b, &ctx);
        } else {
            handshake(&mut b, &mut a, &ctx);
        }

        // Smoke-test the fresh session with a single packet in each
        // direction — this is the specific regression path for key-desync
        // (peer rekeys but its partner doesn't install the new keys).
        let probe = ipv4_packet(0xab);
        let ct = encap_data(&mut a, &probe, &format!("iter {i} post-rekey A->B encap"));
        let (_r, dec) = deliver(&mut b, &ct, &format!("iter {i} post-rekey A->B decap"));
        assert_eq!(
            dec.len(),
            1,
            "iter {i}: post-rekey A->B lost packet (new session not installed on B?)"
        );
        assert_eq!(
            dec[0], probe,
            "iter {i}: post-rekey A->B payload mismatch"
        );

        let probe = ipv4_packet(0xcd);
        let ct = encap_data(&mut b, &probe, &format!("iter {i} post-rekey B->A encap"));
        let (_r, dec) = deliver(&mut a, &ct, &format!("iter {i} post-rekey B->A decap"));
        assert_eq!(
            dec.len(),
            1,
            "iter {i}: post-rekey B->A lost packet (new session not installed on A?)"
        );
        assert_eq!(
            dec[0], probe,
            "iter {i}: post-rekey B->A payload mismatch"
        );
    }

    let elapsed = start.elapsed();
    println!(
        "rekey soak: {REKEY_ITERATIONS} rekeys, {} packets total, {:?} elapsed ({:.0} rekeys/sec)",
        REKEY_ITERATIONS * (PACKETS_PER_ROUND * 2 + 2),
        elapsed,
        REKEY_ITERATIONS as f64 / elapsed.as_secs_f64(),
    );
}
