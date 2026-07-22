---
type: Runbook
title: Verifying a change
description: Build, test, lint commands and the WFE CI pipeline.
tags: [testing, ci, runbook]
timestamp: 2026-07-20T00:00:00Z
---

# Verifying a change

```bash
cargo build --release
cargo nextest run                    # or -p sunbeam --lib / -p sunbeam-sdk --lib
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo doc --no-deps
```

Note: the repo is **no longer a cargo workspace** on `refactor/remove-sdk` —
root and `sunbeam-sdk/` are independent packages with separate lockfiles
(`sunbeam-sdk/Cargo.lock` is gitignored). Run cargo commands in each crate
dir as needed; `--workspace` invocations in older docs are stale.

## Test conventions

nextest + wiremock + mockall + pretty_assertions. The kanban module targets
>90% line coverage (`cargo llvm-cov`). Both crates deny
`unwrap_used`/`expect_used` outside tests and undocumented unsafe;
`sunbeam-sdk` warns on missing docs. Generated gRPC stubs
(`sunbeam-sdk/src/kanban/client/generated.rs`) are gitignored and regenerated
by build.rs — never patch them by hand.

## CI reality

No GitHub Actions. `workflows.yaml` is a WFE pipeline run by wfe-server on
push: checkout → lint (fmt + clippy) → test-unit (lib tests only, both
crates) → on mainline: tag from root `Cargo.toml` version → publish
`sunbeam-sdk` to the `sunbeam` registry → Gitea release via `tea`.
Integration tests that need real services are **not run in CI**; binary
tarball distribution is out-of-band (no cross-compile in CI). Local
verification is the only gate before mainline.

## Releasing

Both crates version together (currently 2.0.0-rc5). Bump root `Cargo.toml`,
push to mainline, CI tags and publishes. Releases are the human's call —
see [charter.md](charter.md).
