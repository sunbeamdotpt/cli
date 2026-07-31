---
type: State
title: Current state of cli
description: What is in flight, what is blocked, what the next session should pick up first.
tags: [state]
timestamp: 2026-07-31T00:00:00Z
---

# State — 2026-07-31

## In flight

- **CLI-023 DONE 2026-07-31 (late)** — card label add/set/remove sent the
  card id as `x-sunbeam-object-id` on BulkUpdateCardLabels (gated
  KanbanBoard+edit → 403 for everyone, root cause of KANBAN-039); now
  sends the card's board_id. Live-verified, committed 4d259e3a. sdk
  bumped to v3.3.2 (d422b004): subscribe renders CardTransferred and
  prefers proto `Assignee.email` over the identity lookup (fallback for
  pre-v2026.07.12 servers). Both unreleased — sit in [Unreleased].
- **KANBAN-054 filed (high)** — attaching a GLOBAL label
  (`label create` without `--project`) to a card makes GetCard for that
  card and ListCardsByBoard for its board 502 for every caller; the
  502ing write still commits. Recover by deleting the label (cascade).
  Hit live on CLI-029 during the CLI-023 verify — see log.md.
- **v3.3.0 RELEASED 2026-07-31** (sdk v3.3.0 → v3.3.1). Eight cards under
  the cli '3.3' milestone: CLI-018 (assign subject validation),
  CLI-019 (card list timestamps), CLI-014+015 (`card update --unblocked`,
  `board column add/update --is-done`, `is_done` in board output),
  CLI-010 (OIDC discovery retry/backoff/cache + transport-vs-auth error
  split — both bugs were cli-side in src/auth.rs), CLI-011 (sdk v3.3.x
  fixes adopted: test prewarm dropped, structural ConnectError matching,
  device-poll test un-ignored and passing against gateway v2026.07.22),
  CLI-020 (`card move --board` cross-board recreate-and-close; deletes
  source instead of done-stamping), CLI-017 (closed no-op — sso-gateway
  stubs regenerate at build time from unpinned BSR). Gates green
  (fmt/clippy/738 nextest). GH release: 9 assets. sdk v3.3.1's tag was
  missing after its release commit — filed SDK-013, tag pushed same day.
  Filed KANBAN-033 (template Done-column is_done needs
  TemplateColumn.is_done) and KANBAN-034 (first-class TransferCard
  endpoint) on the kanban dev board.
- **Tap renamed + test-bot FIXED 2026-07-31** — the repo-wide
  pull_request failure ("Did not find any formulae or commits to test!")
  root-caused: the repo was named `sunbeamdotpt/tap`, but brew tooling
  keys off the `homebrew-*` convention — `Tap#full_name` resolves to
  `sunbeamdotpt/homebrew-tap`, never matching `GITHUB_REPOSITORY`, so
  test-bot skipped PR diff detection; push runs passed vacuously
  (`--only-formulae` is PR-only). Renamed to
  `sunbeamdotpt/homebrew-tap` (GH redirects old URLs; updated refs in
  tap bump-formula.yml/README + cli release.yml a0d619fb). First honest
  runs then surfaced real, previously-invisible issues: shellcheck/shfmt
  offenses in scripts/bump-formula.sh (fixed via `brew style --fix`) and
  a shellcheck 0.11-vs-0.10 gap (SC2312). PR #7 (3.3.0 bump) auto-merged
  hands-off — squash at 14:15 UTC. `brew info sunbeam` → 3.3.0.
  **The release train is now hands-off end to end.** NOTE: closing a PR
  cancels auto-merge; `gh pr merge --auto` must be re-armed after
  close/reopen. Also newer Homebrew requires `brew trust sunbeamdotpt/tap`
  on each machine.
- **v3.2.1 RELEASED 2026-07-27** (patch: CLI-013 card-template name
  resolution now covers project-scoped templates; `-p/--project` on
  get/update/delete). Gates green (fmt/clippy/716 nextest). GH release: 9
  assets. Tap formula landed manually on mainline (38df8b1) — test-bot
  still broken; PR #5 closed with the reason. `brew info sunbeam` → 3.2.1.
  CLI-013 done under the '3.2' milestone. Side finding: seeded global card
  templates 404 on GetCardTemplate — filed KANBAN-026 (kanban dev board).
- **v3.2.0 RELEASED 2026-07-24** (sdk v3.2.0 → v3.3.0). Kanban batch:
  label CRUD + `card label set|add|remove` (CLI-007/009), milestone CRUD +
  `card update --milestone`, comments (CLI-003), assign/unassign (CLI-008),
  `--blocked` (CLI-006), card-template plumbing (CLI-004), left-most-column
  default (CLI-002), private-visibility warning (CLI-001). Gates green
  (fmt/clippy/713 nextest). GH release: 9 assets. Tap: bump-formula
  root-caused + fixed (caller-context checkout bug, 8bf6217); formula
  landed manually (180a51f) because the tap PR test-bot is repo-wide broken.
  `brew info sunbeam` → 3.2.0. All 8 cards done under the cli '3.2'
  milestone. Nothing in flight — next release is a Cargo.toml bump + push.
- **v3.1.3 RELEASED 2026-07-24** (sdk v3.1.1 → v3.2.0: auth-proto regen for
  the sso-gateway `skip_consent` update; no CLI surface changes). Gates green
  (fmt/clippy/695 nextest), pushed as `release: v3.1.3` (715fea99).
- **v3.1.2 RELEASED 2026-07-23** (mail #71: device login requests
  `identity:read identity:admin`; unblocks `kanban project member add` —
  re-login required). GH release published (9 assets); formula landed on
  tap mainline (331fc79) via manual fallback. sienna: `brew upgrade` +
  `sunbeam auth login` to unblock member grants.
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

- ~~tap: pull_request test-bot repo-wide broken~~ FIXED 2026-07-31 —
  root cause was the repo name (see In flight): renamed to
  `sunbeamdotpt/homebrew-tap`; PR #7 auto-merged hands-off.
  ~~bump-formula exit 127~~ FIXED 2026-07-24 (8bf6217): reusable workflows
  inherit the caller's github context, so the checkout pulled the source
  repo, not the tap — checkout now pins `repository:
  sunbeamdotpt/homebrew-tap`.
  The 3.1.3 formula bump was never landed (tap went 3.1.2 → 3.2.0 direct).
- **Kanban tracking** (2026-07-24): sdk dev board carries the tracked
  sdk/sso items as SDK-001..009 (priority as scoring — no numeric field
  exists, carded as KANBAN-013). Kanban list bug carded as KANBAN-012
  (cards_count=11, `card list` returns 2). Mailed kanban (#96).
- **Kanban server** (2026-07-24): v2026.07.7 shipped MilestoneService +
  label catalog CRUD, closing KANBAN-017/018 same-day; cli v3.2.0 consumes
  both. Filed KANBAN-023 (done-column moves don't set completed_at →
  milestone stats read 0). SDK-002 (structured ConnectError) and SDK-010
  (label/milestone clients) are delivered in sdk v3.3.0 — adopted; sdk
  board left for the sdk maintainer. Intermittent empty `unauthenticated:`
  responses with a valid token observed all session — retries succeeded;
  card it if it persists.
- **sso-gateway mail #32** — six API gaps. Item 2 (device poll
  `server_error`) FIXED gateway-side in v2026.07.22 (mail #82): the oauth2
  proxy now relays Hydra 4xx bodies verbatim → proper RFC 8628
  `authorization_pending`. CLI workaround (poll treats `server_error` as
  pending — COE-2026-004) stays until the fix is confirmed in the testing
  image; then un-ignore the device-poll integration test. Items 1, 3-6
  (public device RPCs, identity-by-email, disable/enable RPC, admin
  set-password, directory-read scope) triaged separately by sso-gateway;
  thread open.
- **sdk mail #33** — testing::SsoGateway recovery courier + `sso_url`/
  `sso_client_id` Context fields. sdk reply (#90, 2026-07-24): both
  reasonable, queued for the next testing-harness pass; NOT in v3.2.0.
- **sdk mail #39** — BUG: `tools::ensure_tool` downloads kustomize/helm via
  `reqwest::blocking`; the internal runtime panics when dropped inside an
  async context (cold tool cache → any `kustomize_build` from async code
  crashes, incl. `sunbeam service apply` on a fresh machine). sdk reply
  (#88, 2026-07-24): confirmed real; non-breaking fix (spawn_blocking wrap)
  queued for the NEXT cycle (not v3.2.0); making ensure_tool async waits
  for the next major (breaks `kustomize_build` signature). Workaround:
  tests pre-warm `~/.sunbeam/bin` synchronously (tests/common). When a fix
  tag lands: bump, drop the prewarm.
- **sdk mail #44** — `From<ConnectError>` collapses the structured
  ConnectRPC code into a string. sdk reply (#89, 2026-07-24): dedicated
  variant/structured code field planned, targeted for the release AFTER
  v3.2.0. cli string-matches `"unavailable:"` for its retry decision until
  then.
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

- **CLI-027** (high, todo) — clippy `*_or_default` ban, deferred mid-work
  by the human 2026-07-31. Log.md has the recon: ~121 sites in 42 files,
  two of the card's clippy.toml paths need `allow-invalid`, build.rs is
  linted too. Also open: CLI-021 (auth silent-refresh staleness, high-ish),
  CLI-012/022/024/025/026 (medium), CLI-028/029 (low); CLI-005 still
  blocked on server KANBAN-001.
- The old agent-mail threads (#32/#33 etc.) are historical; cross-repo
  tracking is kanban-only now.
- Deferred kanban UX work from sunbeam's #59 (tracked as cards on the
  kanban dev board): template column authoring + `board create --template`,
  `--columns` on `board create`, `member add` raw OIDC subject + scope
  help, `aggregate create` sources, `project list` board/card counts.
  Check proto/server support before designing flags. (card-template field
  plumbing shipped in v3.2.0.)
- Docs are current again (AGENTS.md + README rewritten for v3) — keep them
  that way when the sdk tag bumps.
