---
type: State
title: Current state of cli
description: What is in flight, what is blocked, what the next session should pick up first.
tags: [state]
timestamp: 2026-07-23T00:00:00Z
---

# State — 2026-07-23

## In flight

- **v3.1.1 RELEASED 2026-07-23** (kanban UUID resolution + `project update
  --prefix` from sunbeam's #59). Tap bumped, verified (`brew upgrade` →
  3.1.1, audit/test green) and pushed. Nothing in flight — next release is
  a Cargo.toml bump + merge + 4 sha256s in the tap.
- **Release train = GitHub Actions only** (WFE/Gitea pipeline REMOVED
  2026-07-22): `ci.yml` (fmt/clippy/nextest + auto-tag from Cargo.toml on
  mainline + dispatch) and `release.yml` (native matrix builds, tarballs +
  raw binaries + checksums → GH release, tap sha256 in job summary).
  `sunbeam update` now self-updates from GH release raw-binary assets.
  Process: `docs/release.md`. Needs repo secret `SUNBEAM_SSO_CLIENT_ID`.
- **Runtime auth is sso-gateway-only** (2026-07-21, second entry in log.md):
  `auth` + `user` + subject↔email resolution migrated off Hydra/Kratos;
  `user disable|enable|set-password` removed. Up-workflow Ory seeding
  deliberately untouched until sbbb drops Ory (see Blocked).
- Build/clippy/fmt/nextest green (694 tests, 83.3% line coverage — ceiling
  analysis in known-issues).
- **Homebrew tap**: live since 3.0.0 (`sunbeamdotpt/tap`). NOTE: brew's tap
  clone under `$(brew --repository)/Library/Taps/sunbeamdotpt/homebrew-tap`
  is a SEPARATE git clone, not a symlink to `../tap` — local formula edits
  must be copied in (or pushed + `brew update`) before `brew audit/install/
  test` will see them.
- **Logger migration**: unified pipeline done (see log.md 2026-07-22);
  intentional tracing:: remainder in users/auth/secrets_cli/workflows (no
  logger in scope / wfe ctx) — acceptable end state, not debt.

## Blocked / waiting

- **sso-gateway mail #32** — six API gaps (public device RPCs, broken device
  poll `server_error`, identity-by-email lookup, disable/enable RPC, admin
  set-password, directory-read scope). CLI workarounds in place (poll treats
  `server_error` as pending — COE-2026-004, confirmed by sbbb); device-poll
  integration test is `#[ignore]`d until the poll bug is fixed.
- **sdk mail #33** — testing::SsoGateway recovery courier + `sso_url`/
  `sso_client_id` Context fields.
- **sdk mail #39** — BUG: `tools::ensure_tool` downloads kustomize/helm via
  `reqwest::blocking`; the internal runtime panics when dropped inside an
  async context (cold tool cache → any `kustomize_build` from async code
  crashes, incl. `sunbeam service apply` on a fresh machine). Workaround:
  tests pre-warm `~/.sunbeam/bin` synchronously (tests/common). When a fix
  tag lands: bump, drop the prewarm.
- **sdk mail #44** — `From<ConnectError>` collapses the structured
  ConnectRPC code into a string; cli string-matches `"unavailable:"` for
  its retry decision until a structured variant lands.
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
- **Automate the tap bump** (human suggestion, 2026-07-23): `release.yml`
  already prints the four tarball sha256s in its job summary; a final job
  could check out `sunbeamdotpt/tap` and bump the formula itself. Needs a
  cross-repo credential (PAT or GitHub App token — `GITHUB_TOKEN` can't
  push to another repo). Decide PR-vs-direct-push when implementing.
- Deferred kanban UX work from sunbeam's #59 (tracked as cards on the
  kanban dev board): template column authoring + `board create --template`,
  card-template field plumbing (title/description/labels/checklist),
  `--columns` on `board create`, `member add` raw OIDC subject + scope
  help, `aggregate create` sources, `project list` board/card counts.
  Check proto/server support before designing flags.
- Docs are current again (AGENTS.md + README rewritten for v3) — keep them
  that way when the sdk tag bumps.
