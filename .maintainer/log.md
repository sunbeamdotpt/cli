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
