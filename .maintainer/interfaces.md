---
type: Reference
title: Interfaces with other systems
description: The sbbb deploy contract, vendored kanban protos, cluster services, and sibling crates.
tags: [interfaces, cross-repo, deploy]
timestamp: 2026-07-20T00:00:00Z
---

# Interfaces

## sbbb — the deploy contract (primary)

Contexts in `~/.sunbeam/config.json` point `infra_dir` at an sbbb checkout
(auto-discovery looks for an `sbbb` sibling). `sunbeam up` / `apply`
kustomize-build those manifests with DOMAIN_SUFFIX substitution and
server-side apply. The CLI reads `sunbeam.pt/*` labels/annotations and the
`profiles/` schema from sbbb. **Contract changes need coordination with the
`sbbb` identity via agent-mail** — either side changing unilaterally breaks
the other.

## kanban

The CLI is a gRPC **client** of the kanban backend using protos vendored in
`sunbeam-sdk/proto/sunbeam/kanban/v1/`. Source of truth is the kanban repo —
**drift risk**: if kanban changes its API, this copy goes stale silently.
Sync direction is always kanban → here. Auth resolves SSO subjects to emails
via the Kratos admin API.

## Sibling crates

- `sunbeam-net` (vpn repo) — WireGuard transport dependency.
- wfe 1.10 — workflow engine, both a library dep and the CI executor
  (`wfectl/` controls remote wfe-server; CI prereqs reference sbbb's
  `base/wfe/`).
- The sibling `sdk` repo (v3.0.0) — **not a dependency of this repo**;
  kanban/nats-callout/proxy consume it for testcontainers. See the naming
  trap in [architecture.md](architecture.md).

## Cluster services operated

OpenBao (vaultrs + HTTP), Ory Kratos/Hydra/Keto, Gitea (releases +
self-update artifacts), CNPG Postgres, Headscale VPN, BuildKit, Zot,
Stalwart mail (lettre for onboarding email), plus client code for
OpenSearch/LiveKit/Matrix/Grafana/Loki/Prometheus/S3.

## Users' machines

`~/.sunbeam/` is the config/secret root (contexts, encrypted vault keystores,
workflows.db). Schema or location changes there affect every operator —
escalate.
