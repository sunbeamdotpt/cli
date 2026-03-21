# Sunbeam CLI

**Sunbeam CLI** is a local development stack manager for Kubernetes-based applications. It simplifies cluster management, service operations, secret handling, and manifest deployment.

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)

## Quick Start

```bash
# Install from source
cargo install --path sunbeam

# Start your local cluster
sunbeam up

# Apply manifests
sunbeam apply

# Check status
sunbeam status
```

## Features

- **Cluster Management**: Bring up local Kubernetes clusters with cert-manager, Linkerd, TLS
- **Service Operations**: Status, logs, restart, health checks across namespaces
- **Secret Management**: OpenBao KV seeding, DB engine config, VSO verification
- **Manifest Management**: Kustomize + Helm builds with domain/email substitution
- **User Management**: Kratos identity CRUD, onboarding/offboarding with mailbox and project provisioning
- **Image Building**: Buildkit-based builds with registry push and rollout deploy
- **Project Management**: Unified ticket management across Planka and Gitea
- **Self-Update**: Binary update from the latest mainline commit
- **Tool Bundling**: kustomize and helm binaries embedded at compile time

## Installation

### Prerequisites

- Rust (2024 edition)
- Docker
- Lima (for local VM management)
- A running Kubernetes cluster (kubectl context `sunbeam` for local dev)

### Install from Source

```bash
git clone https://src.sunbeam.pt/studio/cli.git
cd cli
cargo install --path sunbeam
sunbeam --help
```

### Self-Update

Once installed, sunbeam can update itself:

```bash
sunbeam update
```

## Workspace Layout

```
cli/
  Cargo.toml                    # [workspace] — sunbeam-sdk + sunbeam
  sunbeam-sdk/                  # Library crate — all logic
    src/
      lib.rs
      error.rs, config.rs, output.rs, constants.rs
      kube/       # client, apply, exec, secrets, kustomize_build, tools
      openbao/    # BaoClient HTTP API
      auth/       # OAuth2 PKCE, token cache
      services/   # status, logs, get, restart
      images/     # build, mirror, per-service builders
      secrets/    # seed, verify, KV seeding, DB engine
      users/      # identity CRUD, provisioning (mailbox, projects, email)
      checks/     # functional health probes, S3 auth
      pm/         # Planka + Gitea ticket management
      cluster/    # cert-manager, Linkerd, TLS
      manifests/  # kustomize apply, namespace filtering
      gitea/      # bootstrap (orgs, repos, OIDC)
      update/     # self-update, version
  sunbeam/                      # Binary crate — thin CLI wrapper
    src/
      main.rs                   # tokio, rustls, tracing init
      cli.rs                    # Clap structs + dispatch
```

## Usage

### Basic Commands

```bash
sunbeam up                      # Full cluster bring-up
sunbeam status                  # Pod health across all namespaces
sunbeam status ory              # Scoped to namespace
sunbeam apply                   # Build + apply all manifests
sunbeam apply lasuite           # Apply single namespace
sunbeam logs ory/kratos         # Stream logs
sunbeam logs ory/kratos -f      # Follow mode
sunbeam restart                 # Rolling restart all services
sunbeam restart ory/kratos      # Restart specific deployment
```

### Configuration

```bash
sunbeam config set --domain sunbeam.pt --host user@server.example.com
sunbeam config get
sunbeam config use-context production
```

### Building and Deploying

```bash
sunbeam build proxy             # Build image
sunbeam build proxy --push      # Build + push to registry
sunbeam build proxy --deploy    # Build + push + apply + restart
sunbeam build proxy --no-cache  # Disable buildkit cache
sunbeam mirror                  # Mirror amd64-only images
```

### User Management

```bash
sunbeam user list
sunbeam user create user@example.com --name "User Name"
sunbeam user set-password user@example.com
sunbeam user onboard new@example.com --name "New User" --department Engineering
sunbeam user offboard departed@example.com
sunbeam user recover user@example.com
```

### Secret Management

```bash
sunbeam seed                    # Generate + store all credentials in OpenBao
sunbeam verify                  # E2E VSO + OpenBao integration test
```

### Project Management

```bash
sunbeam pm list                 # List tickets (Planka + Gitea)
sunbeam pm show p:42            # Show Planka card
sunbeam pm show g:studio/cli#7  # Show Gitea issue
sunbeam pm create "Title" --source gitea --target studio/cli
sunbeam pm assign p:42 user@example.com
sunbeam pm close g:studio/cli#7
```

### Health Checks

```bash
sunbeam check                   # Run all functional probes
sunbeam check devtools          # Scoped to namespace
```

### Passthrough

```bash
sunbeam k8s get pods -A         # kubectl passthrough
sunbeam bao status              # bao CLI inside OpenBao pod
```

### Production

```bash
sunbeam config set --domain sunbeam.pt --host user@62.210.145.138
sunbeam config use-context production
sunbeam apply                   # Opens SSH tunnel automatically
```

## Running Tests

```bash
cargo nextest run --workspace   # 232 tests
cargo test --workspace          # Alternative
```

## Python CLI (Legacy)

The original Python implementation is in the `sunbeam/` package and remains functional:

```bash
pip install -e .
python -m sunbeam --help
```

## License

MIT — see [LICENSE](LICENSE).
