---
type: KnownIssue
title: Known issues and doc drift
description: Stale docs, drift risks, and small debt. Remove entries as fixed.
tags: [known-issue, docs, tech-debt]
timestamp: 2026-07-21T00:00:00Z
---

# Known issues

Active discrepancies and debt. When you fix one, delete the entry here and
note it in [log.md](log.md). Verify against the repo before trusting an entry.

## Stale docs

- `CHANGELOG.md` — stops at v1.1.2; the crate is at 3.0.0. Three major
  versions of history missing; human's call whether to backfill or restart.
- `main.rs` keeps a tracing subscriber "as a fallback until the migration is
  fully complete" — the `Logger`/`Sink` migration (`logger-design.md`) is
  mid-flight.

## Drift risks (standing, not one-off)

- **Release asset names** — `sunbeam update` selects the
  `sunbeam-raw-<target>` asset by exact name from the latest GH release;
  renaming assets in `release.yml` breaks self-update for every installed
  binary. Keep the names in `release.yml`, `update.rs`, and
  `docs/release.md` in sync.
- **Test runner** — env-redirect tests conflict under plain `cargo test`
  (threaded); nextest is the supported runner. If that ever changes, those
  suites need a serialization mutex or relocation to integration binaries.
- `vpn create-key` hardcoded value lives in the sdk repo's `vpn/cmds.rs` —
  transferred; fix belongs there.

## Small TODOs

- **Coverage: 82.69% lines** (`cargo llvm-cov nextest`), target was >90%. The
  gap is structural, not laziness: ~1,050 lines `workflows/up/steps` (Lima/
  cluster-bound, accepted), ~900 lines port-forward/pod-exec command bodies,
  ~600 lines HTTPS probe/dispatch glue. Closing it needs production refactors
  (injectable seams) or a live-cluster test rig — human's call whether either
  is worth it.
- Several stale remote branches exist — cleanup is the human's call.
- **sdk `tools::ensure_tool` is async-unsafe** (mail #39): cold-cache tool
  downloads panic inside async contexts (production-reachable via
  `sunbeam service apply` on fresh machines). Track the sdk fix; remove
  the `tests/common::prewarm_tool_cache` workaround when it lands.

## Resolved 2026-07-21 (kept for the record, remove on next pass)

- AGENTS.md / README.md staleness — both rewritten for the v3 layout.
- `src/secrets_cli.rs` kv_list — uses `BaoClient::list` (sdk v3) now.
- kanban proto vendoring drift — vendored protos deleted with the in-tree
  crate; stubs are generated from buf.build by the sdk.
