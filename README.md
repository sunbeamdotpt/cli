# Sunbeam CLI

**Sunbeam CLI** (`sunbeam`) provisions and operates the Sunbeam platform: local
Kubernetes cluster lifecycle, service operations, secrets, identity management,
kanban project management, VPN access, and WFE workflow control.

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE.md)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)

The CLI is a thin command layer over the
[`sdk`](https://github.com/sunbeamdotpt/sdk) crate (v3, git-tag dependency),
which provides Kubernetes, OpenBao, secrets, VPN, kanban, and wfectl clients.

## Quick Start

```bash
# Install from source
cargo install --path .

# Start your local cluster
sunbeam up

# Apply manifests and check status
sunbeam service apply
sunbeam service status
```

## Features

- **Cluster lifecycle**: `sunbeam up` / `sunbeam down` — WFE-workflow-driven
  bring-up/teardown (Lima VM, Cilium, cert-manager, OpenBao, image builds).
- **Service operations**: status, logs, restart, apply, deploy, get, describe,
  exec, shell, port-forward, scale, top, edit, check, seed, transit, verify.
- **Secrets**: OpenBao KV get/put/patch/delete/list, `bao` passthrough.
- **Identity**: sso-gateway user CRUD, onboard/offboard, recovery links.
- **Kanban**: full board/card/project management over ConnectRPC, plus
  realtime `kanban subscribe`.
- **VPN**: Headscale/WireGuard tunnel (`sunbeam vpn connect|status|disconnect`).
- **Workflows**: local + remote WFE instance control (`sunbeam workflow ...`).
- **Auth**: OAuth2 device-code SSO login (`sunbeam auth login`).
- **Self-update**: `sunbeam update` pulls the latest tagged release from GitHub.

## Installation

### Prerequisites

- Rust (2024 edition)
- [`buf`](https://buf.build) on `PATH` (the `sdk` dependency generates its
  ConnectRPC stubs at build time)
- Docker + Lima (for `sunbeam up`)
- A Kubernetes context for service operations

### Install from Source

```bash
git clone https://src.sunbeam.pt/studio/cli.git
cd cli
cargo install --path .
sunbeam --help
```

### Self-Update

```bash
sunbeam update
```

## Usage

### Cluster lifecycle

```bash
sunbeam up                      # Full cluster bring-up (WFE workflow)
sunbeam down                    # Tear down app namespaces
sunbeam down --infra            # Also tear down infra namespaces
```

### Services

```bash
sunbeam service status          # Pod health across all namespaces
sunbeam service status ory      # Scoped to namespace
sunbeam service apply           # Build + apply all manifests
sunbeam service apply ory       # Apply single namespace
sunbeam service logs ory/kratos -f
sunbeam service restart ory/kratos
sunbeam service check           # Functional health probes
sunbeam service seed            # Generate + store credentials in OpenBao
sunbeam service verify          # E2E VSO + OpenBao integration test
```

### Configuration

```bash
sunbeam config set --domain sunbeam.pt
sunbeam config get
sunbeam config use-context production
```

### Identity

```bash
sunbeam user list
sunbeam user create user@example.com --name "User Name"
sunbeam user onboard new@example.com --name "New User"
sunbeam user offboard departed@example.com
sunbeam user recover user@example.com
```

### Kanban

```bash
sunbeam kanban project list
sunbeam kanban board list --project my-project
sunbeam kanban card create --board my-board --title "Fix the thing"
sunbeam kanban subscribe board my-board
```

### Secrets

```bash
sunbeam secrets status              # Seal/init status
sunbeam secrets kv get <path>       # KV v2 read
sunbeam secrets kv put <path> key=value
sunbeam secrets exec secrets list   # Raw bao CLI inside the pod
```

### VPN

```bash
sunbeam vpn connect             # Background daemon
sunbeam vpn connect --foreground
sunbeam vpn status
sunbeam vpn disconnect
```

### Workflows

```bash
sunbeam workflow list           # Local WFE instances
sunbeam workflow status <id>
sunbeam workflow retry <id>
sunbeam workflow -t builds list # Remote wfe-server target
```

## Man pages

Generate man pages for the full command tree (root + every subcommand,
recursively) with the hidden `__man` verb:

```bash
sunbeam __man ./man
man ./man/sunbeam.1
```

This produces `sunbeam.1`, `sunbeam-up.1`, `sunbeam-service-apply.1`, etc.
To install them into your manpath, copy the pages into a section-1 directory:

```bash
sudo cp man/*.1 /usr/local/share/man/man1/
man sunbeam service apply
```

## Running Tests

```bash
cargo nextest run               # Unit + integration tests
cargo llvm-cov                  # Coverage (target: >90% lines)
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Integration tests under `tests/` use testcontainers and skip cleanly when
Docker is unavailable.

## License

MIT — see [LICENSE.md](LICENSE.md).
