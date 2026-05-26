# Sunbeam CLI

Kubernetes-based local dev stack manager. Written in Rust (tokio, clap, kube-rs, wfe workflow engine). The binary is `sunbeam`; source lives in `sunbeam-sdk/src/`.

## Build & Test

```bash
cargo build --workspace --release    # build the sunbeam binary
cargo nextest run --workspace         # run all tests
cargo clippy --workspace -- -D warnings  # lint
```

Unit tests live next to the code in `#[cfg(test)]` modules. Integration tests that touch the cluster are avoided; pure-function tests (parsing, catalog building, filtering) are preferred.

## Architecture

```
platform/cli/
  src/main.rs                       → entry point: installs rustls crypto, tracing, dispatches to sunbeam_sdk::cli::dispatch
  sunbeam-sdk/src/
    cli.rs                          → clap argument definitions + top-level dispatch
    config.rs                       → ~/.sunbeam/config.json (SunbeamConfig, Context, active_context global)
    error.rs                        → SunbeamError, Result, ResultExt, bail! macro
    output.rs                       → step/ok/warn logging helpers
    kube.rs                         → kube-rs client init, server-side apply (kube_apply), rollout restart
    tools.rs                        → downloads/caches kustomize, helm, buildctl binaries
    manifests.rs                    → kustomize build + domain substitution + namespace filtering + apply engine
    manifest_params.rs              → runtime parameter discovery (--set) and override application
    registry.rs                     → service registry discovery from cluster annotations
    services.rs                     → service status/logs/restart queries
    secrets.rs                      → OpenBao init/unseal/seed, VSO secret sync, port-forward
    checks.rs                       → functional health checks
    users.rs                        → Kratos identity management
    workflows/                      → WFE workflow definitions, primitives, and step implementations
      up/                           → cluster bring-up workflow (versioned definition)
      down/                         → cluster tear-down workflow
      verify/                       → VSO + OpenBao integration test workflow
      primitives/                   → reusable WFE step bodies (ApplyManifest, WaitForRollout, etc.)
    project/                        → per-project build/test/deploy commands
    operations/                     → workspace-level commands (compose, stack, worktree)
```

## Critical Rules

- **Do NOT add unnecessary dependencies.** The workspace already pulls in kube-rs, clap, tokio, wfe, serde, etc. Prefer stdlib or existing deps.
- **Do NOT refactor code you weren't asked to change.** Touch only what the task requires.
- **Do NOT over-engineer error handling.** Use `SunbeamError` variants (`Config`, `Kube`, `Io`, `Other`) and `bail!` for early returns. Trust internal code.
- **Do NOT create new files** unless absolutely necessary. Prefer editing existing modules.
- **Never commit secrets** (.env, credentials, keys).

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

**Command dispatch:** `cli.rs` defines the clap enum/struct tree. `dispatch()` matches on `Verb` and routes to submodules. Sub-dispatchers are `async fn dispatch(action: SubAction) -> Result<()>`.

**Error flow:** `bail!("message")` or `return Err(SunbeamError::Other("msg".into()))` for fatal errors. The top-level `main.rs` prints the error chain and exits with the error's exit code.

**Global state:** The active `Context` is set once at startup via `config::set_active_context(ctx)` and read everywhere with `config::active_context()`. The kube context is set similarly via `kube::set_context("...")`.

## What NOT to Do

- Don't add the `logging` crate. Use `tracing` (already configured in `main.rs`) or `output.rs` helpers.
- Don't wrap every kube call in `match` / `if let` when `?` + `with_ctx()` is sufficient.
- Don't add CLI arguments that weren't requested. The clap setup in `cli.rs` is intentionally explicit.
- Don't create utility modules or shared abstractions for one-off operations.
