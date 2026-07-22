---
type: Overview
title: CLI architecture
description: The sunbeam binary, the in-tree sunbeam-sdk crate, and what each part does.
tags: [architecture, rust, cli]
timestamp: 2026-07-20T00:00:00Z
---

# Architecture

The `sunbeam` CLI manages the whole Sunbeam platform's local/prod Kubernetes
dev stack. One command (`sunbeam up`) provisions a Lima VM (k3s + Cilium +
BuildKit), bootstraps OpenBao, applies the sbbb infra manifests, builds
workspace images, and prints service URLs. Also: service ops
(status/logs/restart/exec/port-forward), OpenBao secrets, Kratos user
onboarding, Kanban board management via gRPC, per-project build/test/deploy
(`sunbeam.yaml` targets), VPN connect, self-update from Gitea artifacts.

Stack: Rust 2024, tokio, clap v4, kube-rs, wfe 1.10 (from the private
`sunbeam` registry), tonic/prost, pure rustls (aws-lc-rs), vaultrs.

## Layout

- `src/` — the binary crate. `main.rs` is thin (rustls provider, logging,
  dispatch); `cli.rs` holds the full clap tree; sibling modules
  (`kanban.rs`, `service_cmds.rs`, `secrets_cli.rs`, `project_cli.rs`,
  `operations_cli.rs`, `profiles_cli.rs`, `vcs.rs`, `workflows_cmd.rs`,
  `output.rs`) hold dispatch — **moved here from sunbeam-sdk on the current
  refactor branch** (see [state.md](state.md)).
- `sunbeam-sdk/` — the library crate: error/config/output core, kube client,
  manifests (kustomize + domain substitution), secrets/OpenBao, users/auth,
  kanban gRPC client, `workflows/` (up/down/verify WFE definitions),
  `project/`, `operations/`, `profiles/`, `wfectl/`, VPN commands. Its
  `build.rs` embeds `lima-sunbeam.yaml` and compiles vendored protos.
- `docs/` — `sunbeam-up.md` (bring-up deep dive incl. the OpenBao
  root-token/keystore lifecycle), `service-discovery-labels.md`.

## The naming trap (again, because it bites)

In-tree `sunbeam-sdk` v2.0.0-rc5 ≠ sibling repo `sdk` v3.0.0. Same lineage,
diverged. Charter hard rule 1; details in
[interfaces.md](interfaces.md).

## Secrets you maintain code around but never touch

`~/.sunbeam/config.json` (contexts, VPN keys), `~/.sunbeam/vault/<domain>.enc`
(AES-256-GCM OpenBao root token + unseal keys, Argon2id machine-bound, 0600),
`~/.sunbeam/<context>/secrets/tls.*`. Charter hard rule 3.
