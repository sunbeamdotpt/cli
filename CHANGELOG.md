# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

> **Note:** v2.x history was not maintained in this file. The changelog
> restarts at v3.0.0; consult the git tags (`git tag`, `git log v1.1.2..v3.0.0`)
> for v2.x archeology.

## [3.1.1] - 2026-07-23

### Fixed

- Kanban ID resolution accepts legacy UUIDs: `template get` (and every other
  resolver) rejected the exact identifiers `template list` prints because
  template IDs are UUIDs while the resolver only recognised ULIDs and
  prefixed IDs.

### Added

- `kanban project update --prefix` — change a project's short key after
  creation (the server's `UpdateProject` already supported it via the field
  mask; the CLI never exposed it).

## [3.1.0] - 2026-07-22

### Changed

- **Logging is opt-in:** the default level is now `warn` — commands emit only
  data output plus warnings/errors unless `-v` (info), `-vv` (debug), or
  `-vvv` (trace) is passed; `--quiet` = error. Logging unifies on a single
  stderr pipeline. Piping (`sunbeam kanban ... | jq`) is clean by default.
- **Breaking (kanban):** entity IDs are positional across the kanban command
  tree; modifiers stay flags (`board create <PROJECT>`, `card list <BOARD>`, …).
- Kanban identifiers resolve everywhere: exact ULID, ULID prefix,
  case-insensitive name, and project key prefixes (e.g. `TRI`).
- Kanban errors are actionable: ambiguity lists candidates, not-found lists
  available names, and column resolution fails client-side with the board's
  valid columns.
- Data previously logged at info level (`auth status`, `user create/onboard`
  IDs, `secrets init`/`unseal` results) now prints to stdout as data.

### Added

- One-shot retry on cold-start kanban transport failures (client rebuilt per
  attempt; mutations are safe via idempotency keys).
- `card create` defaults to the board's sole column when `--column` is omitted.
- Regression test locking `kanban auth whoami` expiry reporting.

## [3.0.0] - 2026-07-22

### Changed

- **Breaking:** the in-tree `sunbeam-sdk` crate is replaced by the external
  [`sdk`](https://github.com/sunbeamdotpt/sdk) crate, pinned as a git-tag
  dependency (v3.1.1).
- **Breaking:** the kanban client moved to ConnectRPC; some request and type
  names changed.
- **Breaking:** `user offboard` is now destructive — it revokes all sessions
  and deletes the identity (the sso-gateway has no disable/state API).
- **Breaking:** `auth login` uses the sso-gateway OAuth2 device flow only.

### Removed

- `project`, `operations`/`wt`, and `vcs` verbs, plus the project shortcut
  verbs (`build`, `test`, `lint`, `fmt`, `package`, `deploy`, `dev`, `clean`,
  `doc`). The project/operations library code remains as internal modules used
  by the `up` workflow.
- `user disable`, `user enable`, and `user set-password` (the sso-gateway IAM
  exposes no identity-state or admin credential-set RPC).
- The in-tree SDK crate and the cargo workspace — the CLI is now a single bin
  crate depending on the external `sdk`.

### Added

- Man page generation: hidden `sunbeam __man <dir>` renders the full command
  tree (root + every subcommand, recursively) as section-1 man pages.
- `kanban auth whoami` and `kanban auth logout`.
- Integration test suites for OpenBao, SsoGateway, Headscale, the kanban full
  stack, and profiles (testcontainers via the sdk `testing` feature; they skip
  cleanly without Docker).
- In-tree `wfectl` command layer under `sunbeam workflow` (local + remote WFE
  instance control).
- sso-gateway SSO for runtime authentication and identity management.

### Fixed

- Kanban `--visibility` short-flag collision with the global `--verbose` flag.

## v1.1.2

- 30dc4f9 fix(opensearch): make ML model registration idempotent
- 3d2d16d feat(secrets): add xchacha20-poly1305 cipher key seeding for Kratos
- 80ab6d6 feat: enable Meet external API, fix SDK path
- b08a80d refactor: nest infra commands under `sunbeam platform`

## v1.1.1

- cd80a57 fix: DynamicBearer auth, retry on 500/429, upload resilience
- de5c807 fix: progress bar tracks files not bytes, retry on 502, dedup folders
- 2ab2fd5 fix: polish Drive upload progress UI
- 27536b4 feat: parallel Drive upload with indicatif progress UI

## v1.1.0

- 477006e chore: bump to v1.1.0, update package description
- ca0748b feat: encrypted vault keystore, JWT auth, Drive upload
- 13e3f5d fix opensearch pod resolution + sol-agent vault policy
- faf5255 feat: async SunbeamClient factory with unified auth resolution

## v1.0.1

- 34647e6 feat: seed Sol agent vault policy + gitea creds, bump v1.0.1

## v1.0.0

- 051e17d chore: bump to v1.0.0, drop native-tls for pure rustls
- 7ebf900 feat: wire 15 service subcommands into CLI, remove old user command
- f867805 feat: CLI modules for all 25+ service clients
- 3d7a2d5 feat: OutputFormat enum + render/render_list/read_json_input helpers
- 756fbc5 chore: update Cargo.lock
- 97976e0 fix: include build module (was gitignored)
- f06a167 feat: BuildKit client + integration test suite (651 tests)
- b60e22e feat: La Suite clients — 7 DRF services (75 endpoints)
- 915f0b2 feat: monitoring clients — Prometheus, Loki, Grafana (57 endpoints)
- 21f9e18 feat: LiveKitClient — real-time media API (15 endpoints + JWT)
- a33697c feat: S3Client — object storage API (21 endpoints)
- 329c18b feat: OpenSearchClient — search and analytics API (60 endpoints)
- 2888d59 feat: MatrixClient — chat and collaboration API (80 endpoints)
- 890d7b8 feat: GiteaClient — unified git forge API (50+ endpoints)
- c597234 feat: HydraClient — OAuth2/OIDC admin API (35 endpoints)
- f0bc363 feat: KratosClient — identity management (30 endpoints)
- 6823772 feat: ServiceClient trait, HttpTransport, and SunbeamClient factory
- 31fde1a fix: forge URL derivation for bare IP hosts, add Cargo registry config
- 46d2133 docs: update README for Rust workspace layout
- 3ef3fc0 feat: Python upstream — Sol bot registration TODO
- e0961cc refactor: binary crate — thin main.rs + cli.rs dispatch
- 8e5d295 refactor: SDK small command modules — services, cluster, manifests, gitea, update, auth
- 6c7e1cd refactor: SDK users, pm, and checks modules with submodule splits
- bc65b91 refactor: SDK images and secrets modules with submodule splits
- 8e51e0b refactor: SDK kube, openbao, and tools modules
- b92700d refactor: SDK core modules — error, config, output, constants
- 2ffedb9 refactor: workspace scaffolding — sunbeam-sdk + sunbeam binary crate
- b6daf60 chore: suppress dead_code warning on exit code constants
- b92c6ad feat: Python upstream — onboard/offboard, mailbox, Projects, --no-cache
- 8d6e815 feat: --no-cache build flag and Sol build target
- f75f61f feat: user provisioning — mailbox, Projects, welcome email
- c6aa1bd feat: complete pm subcommands with board discovery and user resolution
- ffc0fe9 feat: split auth into sso/git, Planka token exchange, board discovery
- ded0ab4 refactor: remove --env flag, use --context like kubectl
- 88b02ac feat: kubectl-style contexts with per-domain auth tokens
- 3a5e1c6 fix: use predictable client_id via pre-seeded K8s secret
- 1029ff0 fix: auth login UX — timeout, Ctrl+C, suppress K8s error, center HTML
- 43b5a4e fix: URL-encode scope parameter with %20 instead of +
- 7fab2a7 fix: auth login domain resolution with --domain flag
- 184ad85 fix: install rustls ring crypto provider at startup
- 5bdb789 feat: unified project management across Planka and Gitea
- d4421d3 feat: OAuth2 CLI authentication with PKCE and token caching
- aad469e fix: stdin password, port-forward retry, seed advisory lock
- dff4588 fix: employee ID pagination, add async tests
- 019c73e fix: S3 auth signature tested against AWS reference vector
- e95ee4f fix: rewrite users.rs to fully async (was blocking tokio runtime)
- 24e98b4 fix: CNPG readiness, DKIM SPKI format, kv_patch, container name
- 6ec0666 fix: SSH tunnel leak, cmd_bao injection, discovery cache, DNS async
- bcfb443 refactor: deduplicate constants, fix secret key mismatch, add VSS pruning
- 503e407 feat: implement OpenSearch ML setup and model_id injection
- bc5eeaa feat: implement secrets.rs with OpenBao HTTP API
- 7fd8874 refactor: migrate all modules from anyhow to SunbeamError
- cc0b6a8 refactor: add thiserror error tree and tracing logging
- ec23568 feat: Phase 2 feature modules + comprehensive test suite (142 tests)
- 42c2a74 feat: Phase 1 foundations — kube-rs client, OpenBao HTTP client, self-update
- 80c67d3 feat: Rust rewrite scaffolding with embedded kustomize+helm
- d5b9632 refactor: cross-platform tool downloads, configurable infra dir and ACME email
- c82f15b feat: add tuwunel/matrix support with OpenSearch ML post-apply hooks
- 928323e fix(cli): unify proxy build path, fix Gitea password sync
- 956a883 chore: added AGENTS.md file for various models.
- 507b4d3 feat(config): add production host and infrastructure directory configuration
- cbf5c12 docs: update repository URLs to use HTTPS remotes for src.sunbeam.pt
- 133fc98 docs: add comprehensive README with professional documentation
- 33d7774 chore: added license
- 1a97781 docs: add comprehensive documentation for sunbeam CLI
- 28c266e feat(cli): partial apply with namespace filter
- 2569978 feat(cli): meet build/seed support, production kube tunnel, gitea OIDC bootstrap
- c759f2c feat(users): add disable/enable lockout commands; fix table output
- cb5a290 feat: auto-restart deployments on ConfigMap change after sunbeam apply
- 1a3df1f feat: add sunbeam build integration target
- de12847 feat: add impress image mirroring and docs secret seeding
- 14dd685 feat: add kratos-admin-ui build target and user management commands
- b917aa3 fix: specify -c openbao container in cmd_bao kubectl exec
- 352f0b6 feat: add sunbeam k8s kubectl passthrough; fix kube_exec container arg
- fb3fd93 fix: sunbeam apply and bootstrap reliability
- 0acbf66 check: rewrite seaweedfs probe with S3 SigV4 auth
- 6bd59ab sunbeam check: parallel execution, 5s timeout, external S3 check
- 39a2f70 Fix sunbeam check: group by namespace, never crash on network errors
- 1573faa Add sunbeam check verb with service-level health probes
