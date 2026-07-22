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
- Build/clippy/fmt/nextest green (680 tests, ~82.7% line coverage — ceiling
  analysis in known-issues).
- **Homebrew tap** (`../tap`, UNCOMMITTED): `Formula/sunbeam.rb` written —
  buf+rust build deps, `SUNBEAM_SSO_CLIENT_ID` baked at build time when set
  (caveats otherwise), shell completions + 168 man pages installed.
  **sha256 is a placeholder** until v3.0.0 is tagged; then `brew audit`/
  `brew test` and commit. Tap README lists the formula.
- **Logger migration still mid-flight**: `main.rs` keeps a tracing subscriber
  as fallback (`logger-design.md`).

## Blocked / waiting

- **sso-gateway mail #32** — six API gaps (public device RPCs, broken device
  poll `server_error`, identity-by-email lookup, disable/enable RPC, admin
  set-password, directory-read scope). CLI workarounds in place (poll treats
  `server_error` as pending — COE-2026-004, confirmed by sbbb); device-poll
  integration test is `#[ignore]`d until the poll bug is fixed.
- **sdk mail #33** — testing::SsoGateway recovery courier + `sso_url`/
  `sso_client_id` Context fields.
- ~~sdk mail #25~~ — resolved in sdk **v3.1.1** (adopted + validated with a
  real orchestrator boot; local kanban replica deleted).
- ~~sbbb mail #34~~ — resolved: public client provisioned on the gateway;
  **base URL is `https://auth.{domain}`** (not `sso.{domain}`); the client
  ID is baked at compile time via `SUNBEAM_SSO_CLIENT_ID` (option_env!),
  never committed — the dist/release pipeline must set it (human owns dist).
  **#37**: sso-gateway fronts Ory in prod; up-workflow Ory seeding stays
  until sbbb's human-level call to drop Ory from the manifests.
- ~~CHANGELOG.md~~ — restarted at 3.0.0 in the release-prep commit.
- ~~sdk mail #18/#19/#20~~ — resolved in sdk v3.1.0 (adopted: workarounds
  deleted, 7 direct deps dropped, kanban testing builder delivered).

## Pick up first

- Check for open mail: `agent-mail inbox` (replies may land on #32/#33).
- Docs are current again (AGENTS.md + README rewritten for v3) — keep them
  that way when the sdk tag bumps.
