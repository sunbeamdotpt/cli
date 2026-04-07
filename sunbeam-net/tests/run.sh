#!/usr/bin/env bash
# Integration test runner for sunbeam-net.
#
# Spins up Headscale + two Tailscale peers, creates pre-auth keys,
# runs the Rust integration tests, then tears everything down.
set -euo pipefail

cd "$(dirname "$0")"

COMPOSE="docker compose -f docker-compose.yml"
cleanup() { $COMPOSE down -v 2>/dev/null; }
trap cleanup EXIT

echo "==> Starting Headscale..."
$COMPOSE up -d headscale
$COMPOSE exec -T headscale sh -c 'until headscale health 2>/dev/null; do sleep 1; done'

echo "==> Creating pre-auth keys..."
# Helper that handles both compact and pretty-printed JSON shapes from
# headscale preauthkeys create.
extract_key() {
    grep -o '"key":[[:space:]]*"[^"]*"' | sed 's/.*"\([^"]*\)"$/\1/'
}
# Test client uses an ephemeral key so headscale auto-deletes the node when
# the streaming map connection drops, keeping the test database clean.
PEER_A_KEY=$($COMPOSE exec -T headscale headscale preauthkeys create --user test --reusable --expiration 1h -o json | extract_key)
PEER_B_KEY=$($COMPOSE exec -T headscale headscale preauthkeys create --user test --reusable --expiration 1h -o json | extract_key)
CLIENT_KEY=$($COMPOSE exec -T headscale headscale preauthkeys create --user test --reusable --ephemeral --expiration 1h -o json | extract_key)

echo "==> Starting peers..."
PEER_A_AUTH_KEY="$PEER_A_KEY" PEER_B_AUTH_KEY="$PEER_B_KEY" $COMPOSE up -d peer-a peer-b echo

echo "==> Waiting for peers to register..."
for i in $(seq 1 30); do
    NODES=$($COMPOSE exec -T headscale headscale nodes list -o json 2>/dev/null | grep -c '"id"' || true)
    if [ "$NODES" -ge 2 ]; then
        echo "    $NODES peers registered."
        break
    fi
    sleep 2
done

# In TUN mode tailscale installs a stateful firewall that DROPs incoming
# tailnet traffic by default. Disable it on both peers so the integration
# tests can actually exchange TCP through the tunnel.
echo "==> Disabling tailscale firewall on peers..."
$COMPOSE exec -T peer-a tailscale set --shields-up=false 2>/dev/null || true
$COMPOSE exec -T peer-b tailscale set --shields-up=false 2>/dev/null || true

# Get the server's Noise public key
SERVER_KEY=$($COMPOSE exec -T headscale cat /var/lib/headscale/noise_private.key 2>/dev/null | head -1 || echo "")

echo "==> Peer A tailnet IP:"
$COMPOSE exec -T peer-a tailscale ip -4 || true

echo "==> Peer B tailnet IP:"
$COMPOSE exec -T peer-b tailscale ip -4 || true

echo "==> Running integration tests..."
cd ../..
SUNBEAM_NET_TEST_AUTH_KEY="$CLIENT_KEY" \
SUNBEAM_NET_TEST_COORD_URL="http://localhost:8080" \
SUNBEAM_NET_TEST_PEER_A_IP=$($COMPOSE -f sunbeam-net/tests/docker-compose.yml exec -T peer-a tailscale ip -4 2>/dev/null | tr -d '[:space:]') \
  cargo test -p sunbeam-net --features integration --test integration -- --nocapture

echo "==> Done."
