---
type: Runbook
title: Verifying a change
description: Build, test, lint commands and the GitHub Actions CI setup.
tags: [testing, ci, runbook]
timestamp: 2026-07-22T00:00:00Z
---

# Verifying a change

```bash
cargo build --release
cargo nextest run                    # nextest, NOT cargo test (env-redirect
                                     # tests conflict under the threaded runner)
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo llvm-cov nextest               # coverage; needs LLVM_COV/LLVM_PROFDATA
                                     # env pointing at the nightly llvm-tools
```

Single bin crate `sunbeam` (no workspace). The external `sdk` crate is a
git-tag dependency — builds require `buf` on PATH and network access to
buf.build (sdk ConnectRPC codegen).

## Test conventions

nextest + wiremock + tempfile. Docker-gated integration suites under `tests/`
(sdk `testing` feature / testcontainers: OpenBao, SsoGateway, Headscale,
kanban full stack) skip cleanly without docker. Coverage target >90% lines
(current ~82%, ceiling analysis in known-issues.md); `cargo llvm-cov nextest`.

## CI reality

GitHub Actions only (the WFE pipeline `workflows.yaml` was removed in the v3
release train; Gitea is gone):

- `.github/workflows/ci.yml` — on push/PR: fmt + clippy + nextest. On
  mainline: tags `vX.Y.Z` from `Cargo.toml` (if absent) and dispatches the
  release workflow (GITHUB_TOKEN-pushed tags don't cascade, hence the
  explicit dispatch).
- `.github/workflows/release.yml` — tag-triggered (or workflow_dispatch):
  native matrix builds (aarch64/x86_64 × macOS/Linux) with
  `SUNBEAM_SSO_CLIENT_ID` baked from a repo secret, tarballs + raw binaries
  + checksums → GitHub release. Job summary prints the Homebrew tap sha256.

## Releasing

See `docs/release.md` (the release train). Bump `Cargo.toml`, merge to
mainline, everything else is automated. Releases are the human's call —
see [charter.md](charter.md).
