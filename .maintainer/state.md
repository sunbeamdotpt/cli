---
type: State
title: Current state of cli
description: What is in flight, what is blocked, what the next session should pick up first.
tags: [state]
timestamp: 2026-07-21T00:00:00Z
---

# State — 2026-07-21

## In flight

- **v3 migration COMMITTED on `refactor/remove-sdk`** (4 commits: feat!
  migration, test suites, docs, maintainer bundle; human owns merge/tag).
  The cli depends on the sibling `sdk` repo via git tag **v3.1.1**; the
  in-tree `sunbeam-sdk/` is deleted. Verbs cut: `project` (+shortcuts),
  `operations`/`wt`, `vcs`. kanban rewritten on ConnectRPC. Man pages via
  hidden `sunbeam __man <dir>` (clap_mangen, 168 pages). Release train:
  CHANGELOG restarted at 3.0.0; CI tags from Cargo.toml on mainline.
- **Runtime auth is sso-gateway-only** (2026-07-21, second entry in log.md):
  `auth` + `user` + subject↔email resolution migrated off Hydra/Kratos;
  `user disable|enable|set-password` removed. Up-workflow Ory seeding
  deliberately untouched until sbbb drops Ory (see Blocked).
- Build/clippy/fmt/nextest green (674 tests, 82.7% line coverage — ceiling
  analysis in known-issues).
- **Logger migration still mid-flight**: `main.rs` keeps a tracing subscriber
  as fallback (`logger-design.md`).

## Blocked / waiting

- **sso-gateway mail #32** — six API gaps (public device RPCs, broken device
  poll `server_error`, identity-by-email lookup, disable/enable RPC, admin
  set-password, directory-read scope). CLI workarounds in place; device-poll
  integration test is `#[ignore]`d until the poll bug is fixed.
- **sdk mail #33** — testing::SsoGateway recovery courier + `sso_url`/
  `sso_client_id` Context fields.
- ~~sdk mail #25~~ — resolved in sdk **v3.1.1** (adopted + validated with a
  real orchestrator boot; local kanban replica deleted).
- **sbbb mail #34** — seed the "Sunbeam CLI" public OAuth2 client on
  sso-gateway; `DEFAULT_CLIENT_ID` in `src/auth.rs` is still the old Hydra
  UUID until then. **#28** heads-up: when sbbb drops Ory from the manifests,
  migrate the up-workflow seeding (kratos_admin step, Ory DBs, credentials).
- **CHANGELOG.md** still stops at v1.1.2 — now three versions behind (v3.0.0).
  Human decision whether to backfill or restart fresh.
- ~~sdk mail #18/#19/#20~~ — resolved in sdk v3.1.0 (adopted: workarounds
  deleted, 7 direct deps dropped, kanban testing builder delivered).

## Pick up first

- Check for open mail: `agent-mail inbox` (sdk may reply about #18/#19/#20).
- Docs are current again (AGENTS.md + README rewritten for v3) — keep them
  that way when the sdk tag bumps.
