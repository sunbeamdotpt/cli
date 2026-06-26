# Sunbeam CLI

Kubernetes-based local dev stack manager. Written in Rust (2024 edition) using tokio, clap, kube-rs, and the WFE workflow engine.

The binary is `sunbeam`; the thin entry-point lives in `src/main.rs`. All implementation logic lives in the `sunbeam-sdk` crate under `sunbeam-sdk/src/`.

---

## Semantic Memory Search (Optional)

If a `sunbeam-memory` MCP server is available in your environment, use it for codebase search instead of `grep` or `rg`.

1. **Initialize the repository first.** Before searching, ensure this codebase is indexed:
   - Call `add_watch_target` with the absolute path to this repository.
   - Wait for indexing to complete, then search.
2. **Prefer semantic search.** Use `search_facts` with natural-language queries about behavior, design decisions, known issues, and prior changes.
3. **Store useful findings.** If you discover something future agents should remember (a gotcha, invariant, or decision), call `store_fact` with a concise note and a source URN when possible.

`sunbeam-memory` is **optional**. If the server is not available, skip these steps and use `grep` / `rg` / `Read` as usual. Do not fail, stall, or ask the user to install it.

---

## Technology Stack

- **Language:** Rust 2024 edition
- **Async runtime:** tokio (full features)
- **CLI framework:** clap v4 with derive macros + clap_complete
- **Kubernetes:** kube-rs (client + runtime + websockets), k8s-openapi
- **Workflow engine:** wfe, wfe-core, wfe-sqlite, wfe-yaml, wfe-server-protos
- **TLS/HTTP:** rustls (aws-lc-rs crypto provider), reqwest, tokio-rustls, h2, tonic/prost (gRPC)
- **Serialization:** serde, serde_json, serde_yaml
- **Tracing/Logging:** tracing + tracing-subscriber with custom line/json/threaded layers
- **Crypto:** rsa, sha2, hmac, blake2, chacha20poly1305, hkdf, base64, rand, aes-gcm, argon2, crypto_box, x25519-dalek, rcgen
- **Email:** lettre (SMTP with tokio + rustls)
- **VCS:** gix, repo-rs-cmd, repo-rs-engine, repo-rs-model, repo-rs-git, repo-rs-manifest
- **Secrets:** vaultrs (OpenBao / Vault)
- **Networking/VPN:** boringtun, smoltcp, ipnet, zstd
- **Testing:** cargo nextest, wiremock, pretty_assertions, tokio-test

## Workspace Structure

```
.
├── Cargo.toml              # Workspace root (resolver = "3", version = "2.0.0-rc5")
├── src/main.rs             # Thin entry point: rustls crypto, tracing init, cli dispatch
├── sunbeam-sdk/
│   ├── Cargo.toml          # SDK crate manifest
│   ├── build.rs            # Embeds lima-sunbeam.yaml, sets git commit + build metadata
│   └── src/
│       ├── main.rs
│       ├── lib.rs          # Module declarations, #![warn(missing_docs)]
│       ├── cli.rs          # Clap argument tree (Verb enum) + top-level dispatch
│       ├── error.rs        # SunbeamError, Result, ResultExt, bail! macro
│       ├── output.rs       # Table/JSON/YAML rendering, step/ok/warn banners
│       ├── logging/        # Three-mode tracing subscriber (line, json, threaded)
│       ├── config.rs       # ~/.sunbeam/config.json (Context, Profile, active_context global)
│       ├── kube.rs         # kube-rs client init, server-side apply, rollout restart
│       ├── tools.rs        # Embedded binary extraction (kustomize, helm)
│       ├── manifests.rs    # Kustomize build + domain substitution + namespace filtering + apply
│       ├── manifest_params.rs # Runtime parameter discovery (--set) and override application
│       ├── registry.rs     # Service registry discovery from cluster annotations
│       ├── services.rs     # Service status/logs/restart queries
│       ├── secrets.rs      # OpenBao init/unseal/seed, VSO secret sync, port-forward
│       ├── checks.rs       # Functional health checks
│       ├── users.rs        # Kratos identity management (onboard/offboard/CRUD)
│       ├── auth.rs         # OAuth2 / SSO login flow
│       ├── openbao.rs      # OpenBao HTTP client
│       ├── vault_keystore.rs # Vault transit keystore operations
│       ├── pm.rs           # Project management ticket sync (Planka + Gitea)
│       ├── update.rs       # Self-update from Gitea CI artifacts
│       ├── doctor.rs       # Connectivity diagnostics
│       ├── discovery.rs    # Service discovery via cluster annotations
│       ├── describe.rs     # kubectl describe wrappers
│       ├── exec.rs         # Pod exec and interactive shell helpers
│       ├── port_forward.rs # Kubernetes port-forward utilities
│       ├── proxy.rs        # Local proxy helpers
│       ├── topo.rs         # Topological sort for workspace project graphs
│       ├── vcs/            # Git commands (status, log, branch, clone, commit, push, pull, ...)
│       ├── vpn_cmds.rs     # VPN connect/disconnect/status
│       ├── vpn_env.rs      # VPN daemon socket and environment detection
│       ├── workflows/      # WFE workflow definitions, primitives, and step implementations
│       │   ├── mod.rs      # StepContext (serializable workflow context)
│       │   ├── cmd.rs      # Local workflow subcommands
│       │   ├── data.rs     # Workflow data types
│       │   ├── host.rs     # Workflow host setup
│       │   ├── up/         # Cluster bring-up workflow (definition + steps)
│       │   ├── down/       # Cluster tear-down workflow
│       │   ├── verify/     # VSO + OpenBao integration test workflow
│       │   ├── primitives/ # Reusable WFE step bodies (ApplyManifest, WaitForRollout, etc.)
│       │   └── steps/      # Shared steps (k8s secrets, kratos admin, openbao init, postgres)
│       ├── kanban/         # Kanban board/project management gRPC client
│       │   ├── mod.rs      # Top-level dispatch, URL resolution, idempotency keys
│       │   ├── client.rs   # Generated gRPC stubs + channel/auth helpers
│       │   ├── auth.rs     # Kanban auth commands (whoami, logout)
│       │   ├── projects.rs # Project CRUD and membership
│       │   ├── boards.rs   # Board and column management
│       │   ├── aggregated.rs # Aggregated (meta) boards
│       │   ├── cards.rs    # Card CRUD, move, dependencies
│       │   ├── templates.rs # Board templates
│       │   ├── card_templates.rs # Card templates
│       │   ├── attachments.rs # File attachments
│       │   ├── github_links.rs # GitHub issue/PR links
│       │   ├── search.rs   # Full-text card search
│       │   ├── public_boards.rs # Unauthenticated public board reads
│       │   └── subscribe.rs # Realtime event subscriptions
│       ├── project/        # Per-project build/test/deploy (sunbeam.yaml parsing + runner)
│       ├── operations/     # Workspace-level commands (compose, stack, worktree)
│       ├── profiles/       # Manifest profile system (shortcuts, rules, validation)
│       └── wfectl/         # Remote workflow engine control (list, run, logs, cancel, etc.)
├── sunbeam.yaml            # This repo's own project config (build, test, lint, fmt targets)
├── workflows.yaml          # WFE CI pipeline definition (lint → test-unit → tag → publish → release)
└── lima-sunbeam.yaml       # Lima VM spec for local k3s + Cilium + BuildKit provisioning
```

## Build & Test

```bash
# Build the workspace
cargo build --workspace --release

# Run all tests (requires cargo-nextest)
cargo nextest run --workspace

# Run tests for a specific package
cargo nextest run -p sunbeam-sdk --lib
cargo nextest run -p sunbeam --lib

# Lint
cargo clippy --workspace --all-targets -- -D warnings

# Format
cargo fmt --all

# Generate docs
cargo doc --workspace --no-deps
```

### Test Strategy

- Unit tests live next to the code in `#[cfg(test)]` modules.
- Integration tests that touch a real Kubernetes cluster are avoided.
- Pure-function tests (parsing, catalog building, filtering, serialization roundtrips, topological sorts) are preferred.
- The `logging` module has extensive tests for output formatting using a custom `TestWriter`.
- `error.rs` tests cover exit codes, display formatting, context extensions, and the `bail!` macro.
- `workflows/` tests verify step registration and workflow definition shape without executing against a cluster.
- `kanban/` commands are tested behind `mockall::automock` service traits; the kanban module targets >90% line coverage via `cargo llvm-cov`.

## Architecture

### Entry Point

`src/main.rs` installs the rustls aws-lc-rs crypto provider, parses CLI args, initializes the tracing subscriber (with `LogMode::Line` / `Json` / `Threaded`), sets a panic hook, and dispatches to `sunbeam_sdk::cli::dispatch(cli)`. On error it prints the error chain and exits with the error's exit code.

### CLI Dispatch

`cli.rs` defines the full clap enum/struct tree (`Cli` → `Verb` → sub-enums). `dispatch()` matches on `Verb` and routes to submodule dispatchers. Sub-dispatchers are `async fn dispatch(action: SubAction) -> Result<()>`.

Top-level verbs include:
- `up` / `down` — cluster lifecycle via WFE workflows
- `service` (alias `svc`) — status, logs, restart, apply, deploy, shell, exec, port-forward, scale, top, edit, secrets, transit, delete-job
- `project` (alias `proj`) + shortcuts (`build`, `test`, `lint`, `fmt`, `package`, `deploy`, `dev`, `clean`, `doc`) — per-project targets from `sunbeam.yaml`
- `operations` (alias `ops`) + `wt` shortcut — workspace compose, stack, worktree
- `kanban` — board/project management via gRPC against the Sunbeam Kanban backend (auth, project, board, aggregate, card, template, card-template, attachment, github, search, public-board, subscribe)
- `config`, `user`, `auth`, `pm`, `vpn`, `workflow`, `workflows`, `doctor`, `update`, `version`, `completions`

### Error Handling

Every module returns `Result<T>` (alias for `std::result::Result<T, SunbeamError>`). `SunbeamError` is a `thiserror` enum with variants: `Kube`, `Config`, `Network`, `Secrets`, `Build`, `Identity`, `ExternalTool`, `Io`, `Json`, `Yaml`, `Other`.

- Use `bail!("message")` for early returns with `SunbeamError::Other`.
- Use `.ctx("context")` or `.with_ctx(|| "lazy context".into())` from `ResultExt` to add context without losing structured error variants.
- Convenience constructors: `SunbeamError::kube("...")`, `SunbeamError::config("...")`, etc.

### Global State

- **Active Context:** Set once at startup via `config::set_active_context(ctx)` and read everywhere with `config::active_context()`.
- **Kube Context:** Set similarly via `kube::set_context("...")` and read with `kube::context()`.
- **Apply Semaphore:** A global `tokio::sync::Semaphore(2)` in `kube.rs` limits concurrent manifest applications to protect single-node k3s clusters.

### Workflows

Cluster bring-up and tear-down are orchestrated through the WFE workflow engine:

- **Primitives** (`workflows/primitives/`) — atomic, reusable steps: `ApplyManifest`, `WaitForRollout`, `CreatePGRole`, `CreatePGDatabase`, `EnsureNamespace`, `CreateK8sSecret`, `EnableVaultAuth`, `SeedKVPath`, `WriteKVPath`, `CollectCredentials`, etc.
- **Up steps** (`workflows/up/steps/`) — Lima VM, Cilium, BuildKit, TLS certificates, image builds, VPN pre-auth keys.
- **Down steps** (`workflows/down/steps/`) — namespace teardown.
- **Verify steps** (`workflows/verify/steps/`) — VSO + OpenBao integration verification.
- **Shared steps** (`workflows/steps/`) — OpenBao init/unseal, Postgres wait, Kratos admin identity seed.

Workflow definitions are versioned Rust structs registered with a `WorkflowHost` at runtime.

### Configuration

User config is stored in `~/.sunbeam/config.json` (`SunbeamConfig`). It contains multiple named `Context`s, each with:
- `domain` — domain suffix for manifest substitution
- `infra_dir` — path to infrastructure manifests
- `kube_context` — kubectl context name
- `acme_email` — Let's Encrypt / cert-manager email
- `vpn_url` — optional VPN endpoint

Per-project build config is read from `sunbeam.yaml` in the current directory or workspace root. Per-workspace config is read from `sunbeam.workspace.yaml`.

### Logging

Three output modes controlled by `--log-mode`:
- `line` (default) — awk-friendly `key="value"` single-line format.
- `json` — NDJSON structured logs.
- `threaded` — grouped concurrent output with per-task scrollback (BuildKit-style); falls back to `line` when stderr is not a TTY.

Verbosity is controlled by `--verbose` (once = debug, twice = trace) and `--quiet` (warn only). `RUST_LOG` overrides everything.

The default filter silences noisy dependencies: `tonic`, `hyper`, `h2`, `tower`, `reqwest`, and kube TLS/builder noise.

## Code Style — Follow Existing Patterns Exactly

**Module docstrings:** One-line, starts with a capital letter, uses em-dash to separate topic from description:
```rust
//! Service management — status, logs, restart.
```

**Imports:** stdlib first, then crates, then `crate::` internals. Group with blank lines.

**Output/logging:** Use `output.rs` functions — never bare `println!` for structured output:
```rust
use crate::output::{step, ok, warn};

step("Applying manifests");   // section header: "==> Applying manifests"
ok("Namespace created");      // info line: "    Namespace created"
warn("Pod not ready");        // stderr: "    WARN: Pod not ready"
```

**Error flow:** `bail!("message")` or `return Err(SunbeamError::Other("msg".into()))` for fatal errors. The top-level `main.rs` prints the error chain and exits with the error's exit code.

**Tracing:** Use `tracing::info!`, `tracing::debug!`, etc. with structured fields: `tracing::info!(msg = "...", key = %value)`.

**Avoid:**
- Don't add the `log` crate — use `tracing` or `output.rs` helpers.
- Don't wrap every kube call in `match` / `if let` when `?` + `.ctx()` is sufficient.
- Don't add CLI arguments that weren't requested. The clap setup in `cli.rs` is intentionally explicit.
- Don't create utility modules or shared abstractions for one-off operations.

## CI / CD

Continuous integration is defined in `workflows.yaml` and executed by the WFE workflow engine (not GitHub Actions). The pipeline runs on every push via a wfe-server webhook.

Pipeline stages:
1. **checkout** — clone and checkout the commit
2. **lint** — `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings`
3. **test-unit** — `cargo nextest run --lib` for `sunbeam` and `sunbeam-sdk`
4. **tag** (mainline only) — read version from `Cargo.toml`, create and push a Git tag
5. **publish** (tag created only) — `cargo publish -p sunbeam-sdk --registry sunbeam`
6. **release** (tag created only) — create a Gitea release via the `tea` CLI

Integration tests that require real services are **not** run in CI. Binary distribution (cross-compiled tarballs) is handled out-of-band.

## Security Considerations

- **Never commit secrets** — no `.env` files, credentials, or keys in the repo.
- **TLS:** The project uses pure rustls (no native-tls). The aws-lc-rs crypto provider is installed at startup.
- **VPN:** When the VPN daemon is running, `kube::get_client()` rewrites the cluster URL to a loopback proxy inside the WireGuard trust boundary and disables TLS verification for that hop.
- **Secrets:** OpenBao (HashiCorp Vault fork) is used for KV secrets, database engine config, and transit keystore operations. Root tokens are short-lived and obtained via port-forward.
- **Authentication:** OAuth2/OIDC via Hydra for SSO; Gitea personal access tokens for Git operations.
- **Crypto primitives:** Modern, well-audited crates (chacha20poly1305, aes-gcm, argon2, x25519-dalek) are used for encryption, key derivation, and VPN tunneling.

## Dependencies

- **Do NOT add unnecessary dependencies.** The workspace already pulls in kube-rs, clap, tokio, wfe, serde, etc. Prefer stdlib or existing deps.
- **Do NOT refactor code you weren't asked to change.** Touch only what the task requires.
- **Do NOT over-engineer error handling.** Use existing `SunbeamError` variants and `bail!` for early returns.
- **Do NOT create new files** unless absolutely necessary. Prefer editing existing modules.

## What NOT to Do

- Don't add the `logging` crate. Use `tracing` (already configured in `main.rs`) or `output.rs` helpers.
- Don't wrap every kube call in `match` / `if let` when `?` + `.ctx()` is sufficient.
- Don't add CLI arguments that weren't requested. The clap setup in `cli.rs` is intentionally explicit.
- Don't create utility modules or shared abstractions for one-off operations.
