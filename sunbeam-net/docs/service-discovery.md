# Service Discovery & DNS — `sunbeam.pt/*` annotations

Status: **draft** · Target: rc5 · Supersedes: external-dns

## Goals

1. The Sunbeam desktop app discovers cluster services over the VPN by a stable
   short name, independent of k8s `Service` renames.
2. Public DNS records under `*.sunbeam.pt` are published from the same
   annotations that drive internal discovery — one source of truth.
3. The DNS backend is swappable (generic provider, eventually a Scaleway
   operator matching our exact zone/record conventions).
4. Manifests are lint-able before apply: slug collisions, hostname collisions,
   malformed values all fail fast.

## Non-goals

- Replacing `Ingress` for HTTP path routing. Ingress stays native.
- Replacing cluster DNS. `*.svc.cluster.local` remains resolvable via the VPN
  as it is today.
- Service-mesh features (retries, mTLS). Out of scope.

## Annotation schema

Applied to `Service` as the canonical resource. `Ingress` may override
`hostname` only.

```yaml
# Opt-in — required
sunbeam.pt/expose: "true"

# Identity — required
sunbeam.pt/slug: "hydra"            # ^[a-z][a-z0-9-]{1,30}$
sunbeam.pt/name: "Hydra"            # display name

# Port selection — pick one
sunbeam.pt/port-name: "public"      # preferred — references ServicePort.name
sunbeam.pt/port-number: "4444"      # fallback when ports are unnamed

# Classification
sunbeam.pt/tier: "internal"         # internal | public | vpn-only
sunbeam.pt/category: "auth"         # free-form, used for UI grouping
sunbeam.pt/scheme: "https"          # http | https | grpc | tcp

# Public DNS — required when tier=public, optional elsewhere
sunbeam.pt/hostname: "hydra.sunbeam.pt,auth.sunbeam.pt"   # comma-separated
sunbeam.pt/hostname-ttl: "300"      # default 300
sunbeam.pt/hostname-target: ""      # optional override; usually auto-detected

# UI hints — optional
sunbeam.pt/description: "OAuth2/OIDC provider"
sunbeam.pt/icon: "shield-lock"
sunbeam.pt/owner-team: "auth"
sunbeam.pt/docs-url: "https://docs.sunbeam.pt/auth/hydra"

# Health — optional, desktop-app status badges
sunbeam.pt/health-path: "/health/ready"
sunbeam.pt/health-interval: "30s"
```

## Defaults

| Rule | Default |
|---|---|
| `tier: public` + no `hostname` | auto-generate `<slug>.sunbeam.pt` |
| `hostname-ttl` empty | `300` |
| `hostname-target` empty | resolve from `Service.status.loadBalancer.ingress[]`; fallback to any `Ingress` whose `backend.service.name` references this Service |
| `scheme` empty | infer from port (443/8443→https, else http) |
| `port-name`/`-number` both empty | single-port Service: use that port; multi-port: reject |

## Invariants (controller-enforced)

Manifest is rejected (admission-webhook) or the controller marks
`status.conditions[].type=Valid=False` when:

1. `slug` is absent on any `expose: true` Service.
2. `slug` does not match `^[a-z][a-z0-9-]{1,30}$`.
3. `slug` collides with another `expose: true` Service anywhere in the cluster.
4. `hostname` has values outside the configured apex (default `*.sunbeam.pt`).
5. Two Services claim the same `hostname`.
6. `tier: public` but no `hostname` resolvable AND slug-based default would
   collide with an existing hostname.
7. Neither `port-name` nor `port-number` resolves against the Service's ports.
8. `tier: public` but the controller cannot determine a `hostname-target`
   (no LB IP, no Ingress backref) — status becomes `awaiting-lb`, no record
   is published.

## Name-resolution contract

Three ways a caller can reach an exposed service:

| Access path | Source of truth | Reachability |
|---|---|---|
| `<slug>` (short name, via desktop app) | `sunbeam.pt/slug` → VPN daemon IPC | Over VPN only |
| `<service>.<ns>.svc.cluster.local` | cluster DNS | Over VPN only |
| `<host>.sunbeam.pt` | `sunbeam.pt/hostname` → dns-controller → authoritative zone | Public Internet |

Short names are exposed via the daemon IPC (`daemon.sock`) as:
```
GET /services → [{ slug, name, namespace, svc_dns, ports, scheme, tier,
                   category, hostnames, health: { path, status } }]
```

## Controller architecture

```
┌─────────────────────┐          ┌──────────────────────┐
│ k8s apiserver       │◄─────────┤ sunbeam-dns-         │
│  (Services,         │  watch   │ controller           │
│   Ingresses)        │──events─►│                      │
└─────────────────────┘          └──────────┬───────────┘
                                            │
                                            ▼
                                    ┌───────────────┐
                                    │ DnsProvider   │ ◄── trait
                                    │ (interface)   │
                                    └───────┬───────┘
                                            │
                    ┌───────────────────────┼───────────────────────┐
                    ▼                       ▼                       ▼
             ┌───────────┐          ┌───────────────┐        ┌──────────────┐
             │ PowerDNS  │          │ Scaleway      │        │ Mock         │
             │ provider  │          │ provider      │        │ (tests only) │
             │ (v1)      │          │ (future)      │        │              │
             └───────────┘          └───────────────┘        └──────────────┘
```

The `DnsProvider` trait isolates the zone/record wire protocol. The
controller owns reconcile logic, status writes, and invariant checks.
Switching providers is a compile-time flag or a runtime config value.

**Trait sketch** (final shape TBD in rc5):
```rust
pub trait DnsProvider: Send + Sync {
    async fn list_records(&self, zone: &str) -> Result<Vec<Record>>;
    async fn upsert(&self, zone: &str, rec: &Record) -> Result<()>;
    async fn delete(&self, zone: &str, name: &str, rtype: RecordType) -> Result<()>;
}
```

## Migration path from external-dns

1. Add `sunbeam.pt/hostname` to every Service currently carrying
   `external-dns.alpha.kubernetes.io/hostname`. Values are copied verbatim.
2. Deploy `sunbeam-dns-controller` in shadow mode (reads, doesn't write) —
   verify computed record set matches external-dns actual records.
3. Scale external-dns to 0.
4. Flip `sunbeam-dns-controller` to write mode.
5. Delete external-dns RBAC + Deployment.
6. Remove `external-dns.alpha.kubernetes.io/*` annotations from manifests.

## CLI surface

Add to `sunbeam` CLI (rc5):

- `sunbeam service list` — table of slug, hostnames, tier, health, owner
- `sunbeam service get <slug>` — full detail + live health probe
- `sunbeam service validate <file>|<dir>` — pre-apply lint; exits non-zero
  on any invariant violation. Runs locally, no cluster connection needed.
- `sunbeam service watch` — stream events as services come/go

## VPN daemon integration

The VPN daemon subscribes to the controller's service registry (either by
watching k8s itself, or by pulling a reconciled JSON blob the controller
publishes to a well-known ConfigMap). On change, it writes
`~/.sunbeam/vpn/services.json` and exposes it via the existing IPC socket.

Rationale: the desktop app already connects to the daemon IPC for tunnel
status — adding service discovery to the same socket avoids a second
connection and keeps the app usable offline (cached list) when the VPN is
down.

## Open questions

- **Where does the apex live?** `*.sunbeam.pt` is fine for prod. Dev
  clusters need a local override — probably a controller flag.
- **Should `tier: vpn-only` publish an internal DNS record?** E.g., a
  `*.vpn.sunbeam.internal` zone served by the VPN daemon's DNS responder.
  Tabled until we have a use case.
- **Versioning the annotation schema.** Eventually we'll need
  `sunbeam.pt/api-version: "v1"`. Not needed on day one, but reserve the
  key so adding it later is non-breaking.

## Out of scope (intentionally)

- Cert issuance — cert-manager stays the way it is; it already reads
  Ingress and does the right thing.
- Service mesh or L7 policy.
- Multi-cluster discovery. One cluster = one tailnet = one service registry.
