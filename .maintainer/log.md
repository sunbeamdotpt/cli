# Decision log

Append-only. Newest at the bottom. Every entry: what was decided, and *why* —
future sessions need the reasoning, not just the outcome. Never rewrite history.

## 2026-07-20 — Maintainer bundle created

Enrolled cli as the fourth repo in agent-mail. Three charter decisions worth
recording: (1) the `sunbeam-sdk` vs sibling-`sdk` naming trap is hard rule 1
because the names invite a confident agent to edit the wrong repo — the
crates share lineage but diverged (v2.0.0-rc in-tree vs v3.0.0 standalone);
(2) "maintainers edit the tool, the human operates it" — a session that can
run `sunbeam up` against a real cluster must still not, because provisioning
is an operation with blast radius, not an edit; (3) the bundle landed while
the repo sits on the mid-refactor branch `refactor/remove-sdk`, so stale
docs that describe the old layout are recorded as known issues to fix *in
coordination with* the branch landing rather than immediately — fixing them
ahead of the merge would document a layout that's about to disappear.


## 2026-07-21 — v3: cli now depends on the sibling `sdk` repo (hard rule 1 retired)

The human clarified that the sibling `sdk` repo IS the in-tree `sunbeam-sdk`:
they moved it out, shrank it, and reshaped it into a proper SDK (v3.0.0). The
charter's hard rule 1 ("never cross-edit, never reunify") was written against
the wrong premise and is amended today. Why this matters for future sessions:
the old mental model (two diverged crates, in-tree one is canonical for cli)
would lead an agent to refuse exactly the migration the human asked for.

Decisions made this session (human-directed):
- cli depends on `sdk = { git, tag = "v3.0.0" }`; imports are `sdk::…`. The
  in-tree `sunbeam-sdk/` is deleted. This is cli v3.0.0 — breaking changes
  were explicitly authorized.
- CLI surface slimmed: `project` (+shortcut verbs), `operations`/`wt`, `vcs`
  removed. The project/operations library code stays as internal modules only
  because `workflows/up` builds images with it.
- kanban rewritten on the sdk's ConnectRPC `KanbanClient` (old tonic/gRPC
  stack dropped, including the vendored protos — the proto-drift known issue
  dies with them). `card-template`/`subscribe` were briefly dropped, then
  restored when the generated clients turned out to expose those RPCs.
- Everything v3 doesn't provide was ported INTO the cli crate (auth login
  flow, users, services, checks, registry, workflows tree, wfectl command
  layer, etc.) — that code is command-layer logic, so cli is its right home.
- Test bar set by the human: >90% unit line coverage (llvm-cov) plus as many
  integration tests as feasible (sdk `testing` feature / testcontainers +
  wiremock, docker-gated skips).
- Coordination protocol with the sdk repo: when the SDK is missing something,
  mail `agent-mail send --to sdk --kind task` and work around locally in the
  meantime; bump the git tag when a new sdk release lands and drop the
  workaround. Sent: #18 (secrets visibility, lettre From impls, kube/reqwest
  re-export, wfectl error unification), #19 (correction + ConnectError From,
  prelude re-exports, client ergonomics).


## 2026-07-21 (later) — Runtime auth is sso-gateway-only; Ory dropped from the runtime surface

Human decision: Hydra/Kratos support is out, sso-gateway is the only auth
backend the CLI talks to. Why: the platform is consolidating IAM onto
sso-gateway; maintaining two auth stacks in the CLI was duplication against a
backend that's going away.

Scope call (human picked "runtime only"): `auth` (device-code login →
sso-gateway's public device endpoints), `user` (→ IdentityService), and
subject↔email resolution (→ IdentityService) migrated; the `up` workflow Ory
seeding deliberately left in place because it tracks sbbb's manifests — sbbb
was mailed (#28 heads-up, #34 seed request for the CLI's public OAuth2
client). When sbbb drops Ory from the manifests, the workflow seeding follows.

Consequences worth remembering:
- `user disable/enable` and `user set-password` are GONE (no such RPCs in
  sso-gateway — filed with sso-gateway as #32). `offboard` is now destructive
  (revoke sessions + delete identity).
- Login uses the gateway's PUBLIC /oauth2/device HTTP endpoints, not the
  ConnectRPC OAuth2DeviceService (it's admin-gated — chicken-and-egg).
- DEFAULT_CLIENT_ID in src/auth.rs is still the old Hydra UUID until sbbb
  provisions the CLI's public client (#34).
- Mails sent: #32 (sso-gateway: 6 API gaps), #33 (sdk: testing recovery
  courier + sso_url/client-id config fields), #34 (sbbb: seed request).


## 2026-07-22 — WFE/Gitea CI removed; release train is GitHub Actions only

Human decision: "remove wfe ci integration for now, esp gitea as that's long
gone." workflows.yaml deleted; `.github/workflows/ci.yml` (fmt/clippy/nextest
+ auto-tag from Cargo.toml on mainline + explicit release dispatch) and
`release.yml` (native matrix builds → GH release) replace it. Why the
explicit dispatch: GITHUB_TOKEN-pushed tags don't cascade to on:push
workflows — a first-pass workaround that avoids needing a PAT.

Self-update moved from Gitea CI artifacts (per-commit bleeding edge) to GH
tagged releases (`sunbeam-raw-<target>` asset + checksums.txt). Considered
the `self_update` crate and rejected it: it duplicates reqwest and pulls
tar/zip/indicatif for an archive-extraction model we don't need once raw
binary assets exist — the existing tested atomic_replace/checksum machinery
was kept and only the fetch layer swapped. Dead `check_update_background`
cache removed with the Gitea code.


## 2026-07-22 — v3.0.0 released; train proven end-to-end

First release through the new train. Merge to mainline → ci.yml tagged
v3.0.0 → release.yml published the GH release; tap updated and pushed.
CI hardening learned the hard way (each a real commit): runners need
protoc (wfe-buildkit-protos), gxhash needs per-target AES flags in
.cargo/config.toml (x86_64 +aes,+sse2; aarch64-linux +aes,+neon — only
apple-aarch64 gets AES by default), sdk's reqwest::blocking tool download
panics in async contexts on cold caches (sdk mail #39; test-side prewarm
workaround), a test distractor asset must never equal the host's asset
name, buf-setup-action needs github_token (rate limits), and macos-13
Intel runners queue forever (x86_64-apple now cross-builds on macos-14 +
Rosetta). Homebrew: the brew wrapper filters env to an allowlist +
HOMEBREW_*, so build-time baking uses HOMEBREW_SSO_CLIENT_ID; formula
takes protobuf+buf as build deps (superenv hides system tools).


## 2026-07-22 (later) — Opt-in logging; sbbb prod feedback shipped

Human directive after sbbb's kanban field report: logging must be opt-in
("otherwise exhausting to script around"). Decision: default level is Warn
(data output only), -v/-vv/-vvv opt into info/debug/trace — the unix
convention and what makes `cmd | jq` trustworthy. Unified the pipeline at
the same time: `Logger::new(TracingSink)` over the sdk tracing subscriber
(one stderr pipeline for Logger macros AND tracing:: calls) instead of the
dual sink-selection in main.rs. Deliberately did NOT chase full
tracing→Logger call-site migration: users/auth/secrets_cli have no logger
in scope and workflows has no logger in the wfe ctx — post-unification the
behavior is identical, so the remaining tracing:: calls are an accepted
end state, not debt.

sbbb's whoami "expired:false" bug did not reproduce — likely a stale
pre-v3 binary (we found a June cargo-install shadowing brew on our own
machine and told sbbb to check theirs). Locked correct behavior with a
regression test anyway. The rest of their feedback shipped as two commits
on mainline (3.0.1 candidate, unreleased — version bump is the human's).


## 2026-07-22 (even later) — v3.1.0 released; tap ships binaries, not source

Human hard requirement mid-upgrade: Homebrew must not require a build
toolchain (buf/protoc/rust + network + 10-min compile is unacceptable for
end users). The tap formula now installs the prebuilt release tarballs
(binary + man pages + completions per platform) instead of building from
the source archive. Consequences: no build deps in the formula, installs
are instant, and the SSO client ID reaches brew users pre-baked from the
release workflow's repo secret — the HOMEBREW_SSO_CLIENT_ID bake channel
from earlier today is now moot (kept in the formula's history only).
release.yml's job summary now prints the four tarball sha256s for the
formula update step.


## 2026-07-23 — Mail batch: sbbb verifications closed; sunbeam UX triage

sbbb confirmed all kanban fixes hold on 3.1.0 (#48 cold-start retry + whoami,
#49 positional IDs/prefix resolution, #51 full reverify clean; #50 was a
mis-addressed consent thread, no cli ask). Also resolved: the whoami
"expired" confusion was sbbb misreading +01:00 local timestamps as UTC —
the flag was never broken; our regression test stands regardless.

sunbeam's #59: UX feedback from a ~200-call org-wide kanban rollout. Two
items fixed same-day on mainline (unreleased): (1) the ID resolver was
ULID-only while template IDs are legacy UUIDs, so `template get` rejected
the exact ID `template list` prints — `looks_like_id` now recognises UUIDs
(decision: accept the printed ID rather than print something else, since
UUIDs are the server's real identifiers and pass-through costs no RPC);
(2) `project update --prefix` added — the field existed on the proto but
was never wired into the CLI's field mask. Deferred to sunbeam's dev-board
cards (feature design, not drive-bys): template column authoring +
`board create --template`, card-template field plumbing, `--columns` bulk
flag, `member add` raw-subject/help polish. Verified: 117/117 kanban tests
incl. docker end-to-end, clippy -D warnings clean.


## 2026-07-23 (later) — v3.1.1 shipped; setup-protoc rate limit

Human approved the release train; four conventional commits (fix/feat/
chore/release) pushed, ci.yml tagged v3.1.1, release dispatched. First
release run FAILED on x86_64-apple: arduino/setup-protoc hit the
unauthenticated GitHub API rate limit — same class as the buf failure we
fixed with github_token (94e08f5b). Fixed identically (repo-token:
secrets.GITHUB_TOKEN in ci.yml + release.yml, c09864eb) and re-dispatched
the release for the SAME tag per the runbook (never move tags). Lesson:
any action that resolves a tool via the GitHub API needs an explicit
token input; audit future additions for it.

Tap update surfaced a workflow wrinkle: brew's tap clone under
Library/Taps is a separate clone, not a symlink to ../tap, and this
brew's `brew audit <path>` is disabled — so the documented
audit/install/test-then-push order only works after copying the edited
formula into the tap clone (or pushing first). Recorded in state.md.
Human floated automating the tap bump from release.yml — noted under
Pick up first (needs a cross-repo credential; GITHUB_TOKEN won't push
to sunbeamdotpt/tap).


## 2026-07-23 (later still) — mail #71: device login identity scopes

sunbeam reported `kanban project member add` failing permission_denied
because `sunbeam auth login` tokens only carry OIDC scopes; the gateway
client ceiling was raised to allow identity:admin. Fixed by adding
`identity:read identity:admin` to the device-flow scope request
(src/auth.rs:295) and the registration doc comment. Chose both scopes
(admin for member mutations, read for lookups) — the ceiling allows all
six *:admin, and Hydra-style scope checks are exact-match so admin does
not imply read. Verified: cargo check, fmt, 67 auth-tagged nextest tests.
Replied + acked #71. Release (v3.1.2) is required for the fix to reach
sienna — escalated as human-owned per charter.


## 2026-07-23 (evening) — v3.1.2 shipped; two release-train wrinkles

Human approved the cut. Gates green (clippy, 695/695 nextest), three
commits on mainline (fix/auth scopes, maintainer, release), CI tagged
v3.1.2, release published all 9 assets. Then two wrinkles:

1. **homebrew-tap job failed: "appId option is required".** Reusable
   workflows receive NO secrets unless the caller passes them — the job
   added in 0b6129d2 lacked `secrets: inherit`, so the org's
   SUNBEAM_TAP_APP_ID never crossed the workflow_call boundary. Fixed in
   9b96ed59. Lesson: any `uses: <reusable>` that touches secrets needs
   `secrets: inherit` (or explicit mapping) — audit future additions.
2. **Re-dispatching the release for the same tag fails at publish**:
   org releases are IMMUTABLE — "Cannot delete asset from an immutable
   release". The runbook's re-dispatch path only works when assets are
   missing; documented the caveat + the surgical alternative (dispatch
   bump-formula.yml in tap directly) in docs/release.md.

Tap side: bump-formula opened PR #3 correctly (shas verified against
checksums.txt) but pull_request test-bot fails repo-wide ("Did not find
any formulae or commits to test!" — every PR run, #1/#2/#3), so
auto-merge can never engage. Landed the bump on tap mainline manually
(331fc79, cherry-pick of the PR commit — same pattern as sunbeam-memory
0.3.4 earlier today) and closed PR #3. Mailed tap (#75) about the
broken PR CI.


## 2026-07-24 — sdk v3.2.0 adopted; cli v3.1.3 shipped

Human pre-authorized a patch release triggered on sdk mail. A 15-min cron
watch (kimi session cron 7c8390dd) caught four sdk replies landing together:

- #87 (kanban testing bugs): sdk confirmed, closed — acked. v3.1.1 stands
  as the good orchestrator baseline.
- #88 (ensure_tool reqwest::blocking panic): confirmed real, but NOT in
  v3.2.0 — spawn_blocking wrap queued for the next cycle; async signature
  change waits for the next major. Prewarm workaround stays.
- #89 (From<ConnectError> loses code): structured variant planned for the
  release after v3.2.0. String match on "unavailable:" stays.
- #90 (recovery courier + sso_url/sso_client_id Context fields): both
  queued for the next testing-harness pass; not in v3.2.0.

Decision: v3.2.0 (auth-proto regen for sso-gateway skip_consent) carried
none of the tracked fixes, but adopting it promptly keeps the tag delta
small — bumped both sdk entries in Cargo.toml, version 3.1.2 → 3.1.3,
CHANGELOG entry, gates green (fmt, clippy, 695/695 nextest), pushed as
715fea99. Why patch not minor: no CLI surface change, dependency-only.

Also handled sso-gateway #82: item 2 of the six API gaps fixed in gateway
v2026.07.22 (oauth2 proxy relays Hydra 4xx verbatim → RFC 8628
authorization_pending). Replied (#83); COE-2026-004 workaround + the
#[ignore]d device-poll test stay until the fix reaches the testing image.


## 2026-07-24 (later) — v3.1.3 verified; boards updated; tap bump broken

Release workflow: 4 builds + publish green, 9 assets on v3.1.3 — but the
homebrew-tap job failed (exit 127, `scripts/bump-formula.sh` missing in the
tap repo's reusable workflow). Not re-dispatched (would fail identically);
recorded in state.md for the human — tap mainline is tap's publish, not
ours. sdk watch cron (7c8390dd) deleted; trigger fulfilled.

Human asked to card the tracked work: sdk dev board now has SDK-001..009
(3 sdk items, 5 sso-gateway gaps, 1 cli follow-up; priority used as the
scoring dimension — no numeric field exists). Found two kanban gaps while
doing it: `card list` returns 2 of 11 cards (KANBAN-012, high) and no
numeric score/estimate field (KANBAN-013, medium) — mailed kanban (#96).

## 2026-07-24 — agent-mail → kanban ticketing migration

Cross-repo coordination moved off agent-mail (deprecated) onto kanban cards
via `sunbeam kanban` — the same migration sbbb did earlier. The AGENTS.md
ritual, charter, state.md, and interfaces.md now describe the kanban flow;
mail-thread references in older entries and in state.md (#32/#33/#39/#44
etc.) are historical identifiers, kept so the record stays traceable.
*Why:* the human standardized cross-repo tracking on kanban so tickets are
visible to everyone, not just the two mail endpoints.


## 2026-07-24 (later) — v3.2.0 prepared: kanban UX batch (CLI-001..004/006/008)

Human asked to work the cli dev board and cut a release. Six cards in todo;
five implementable against sdk v3.2.0 + the live BSR module, CLI-008 added to
the pile by the human mid-session. CLI-005 (archive) stays blocked on
KANBAN-001 server-side; CLI-007 (labels) landed mid-session and is likewise
server-blocked (no ListLabels RPC — names can't resolve to catalog ULIDs).

Shipped in one minor release (new verbs/flags → minor per docs/release.md):
card comment list/add/edit/delete (CLI-003), card assign/unassign with
email→subject resolution via sso-gateway + assignees in list output (CLI-008),
card update --blocked (CLI-006; set-only — the server's UpdateCard SQL never
applies blocked=false, documented in the CHANGELOG), card-template field
plumbing (CLI-004), card create defaults to the left-most column (CLI-002),
board create warns when defaulting to private (CLI-001). Gates green: fmt,
clippy, 699/699 nextest.

Milestone ask ("make the cards part of the 3.2 milestone") turned out to be
impossible client-side: Card.milestone_id exists in the proto but there is no
MilestoneService anywhere — not on the BSR, not in the kanban repo. Filed
KANBAN-017 on kanban/dev. Human then pushed a BSR update; it documented
cross-board dependency edges (not milestones) — dogfooded immediately: CLI-005
now depends_on KANBAN-001 across boards, and CLI-005 is marked blocked=true
with the freshly built --blocked flag.

Board end state: six implemented cards in review (→ done when the release
publishes), CLI-005 todo+blocked with the dependency edge, CLI-007 todo.

Commit e09b1768 "release: v3.2.0" is on mainline but NOT pushed — human
chose commit-only; pushing triggers the tag + publish.

Ops note: the kanban server returned intermittent empty `unauthenticated:`
errors all session (token valid; retries succeeded). Worth watching — if it
persists, card it against kanban.


## 2026-07-24 (evening) — v3.2.0 SHIPPED: labels + milestones land mid-flight

The v3.2.0 release scope grew twice in-session. After the kanban UX batch
was committed (e09b1768, unpushed), kanban shipped v2026.07.7 — MilestoneService
+ label catalog CRUD, closing KANBAN-017/018 same-day — and sdk tagged v3.3.0
with the matching clients (labels()/milestones() accessors) plus the
structured SunbeamError::Connect variant (SDK-002). Human directed: adopt,
implement CLI-009, and push.

Adopted sdk v3.3.0; implemented label CRUD + `card label set|add|remove`
(name resolution via ListLabels, wholesale replace via BulkUpdateCardLabels —
CLI-007 covered too), milestone CRUD + `card update --milestone` (title
resolution against the card's project). The sdk's new Connect variant broke
two kanban retry tests — the cold-start matcher now uses the structured
ErrorCode::Unavailable + send-failure message instead of the old string
collapse, and pins that server-side `unavailable` is NOT retried. Gates:
fmt/clippy/713 nextest green. Conventional commits on top (chore(deps),
feat(kanban)), pushed 1ffb846d; CI tagged v3.2.0, release workflow
published all 9 assets.

Tap: the homebrew-tap job failed exit 127 AGAIN — root cause found this
time: reusable workflows inherit the CALLER's github context, so the
workflow's checkout pulled sunbeamdotpt/cli (no scripts/bump-formula.sh)
instead of the tap. Fixed tap-side (8bf6217: pin repository+ref on the
checkout), dispatched bump-formula for 3.2.0 — bot PR #4 shas verified
against checksums.txt, but the repo-wide pull_request test-bot failure
('Did not find any formulae or commits to test!') blocked auto-merge again,
so landed on tap mainline manually (180a51f, same pattern as 3.1.2's #3)
and closed the PR. brew info sunbeam → stable 3.2.0. The test-bot bug is
now the only manual step in the train; it's tap-repo work, still open.

Board end state: CLI-001..004/006/007/008/009 done, grouped under the new
'3.2' milestone on the cli project (milestone management itself dogfooded
from the fresh debug binary). CLI-005 stays todo+blocked (KANBAN-001).
Filed KANBAN-023: done-column moves don't set completed_at, so milestone
completion stats read 0/8 despite all cards done — stats vs column semantics
is a kanban-team call. SDK-002 and SDK-010 are delivered in sdk v3.3.0
(sdk board left for the sdk maintainer to move).
