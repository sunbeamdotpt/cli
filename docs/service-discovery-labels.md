# Sunbeam Service Discovery Labels

Migration guide: replacing hardcoded service definitions in `sunbeam-sdk/src/registry/services.rs` with Kubernetes label-based discovery.

---

## 1. Label Convention

Every Sunbeam-managed service must have labels and annotations on its **primary Kubernetes resource** (Deployment, StatefulSet, DaemonSet, or CNPG Cluster).

### Labels (required)

| Label | Value | Purpose |
|---|---|---|
| `sunbeam.pt/service` | Service name (e.g. `hydra`) | Primary lookup key for the CLI |
| `sunbeam.pt/category` | Category slug (e.g. `auth`) | Grouping for `sunbeam status`, `sunbeam restart <category>` |

### Annotations (conditional)

| Annotation | Value | When to include |
|---|---|---|
| `sunbeam.pt/display-name` | Human-readable name (e.g. `"Hydra (OAuth2/OIDC)"`) | Always |
| `sunbeam.pt/kv-path` | OpenBao KV v2 secret path (e.g. `hydra`) | Only if the service has secrets in OpenBao |
| `sunbeam.pt/db-user` | PostgreSQL username (e.g. `hydra`) | Only if the service has a CNPG database |
| `sunbeam.pt/db-name` | PostgreSQL database name (e.g. `hydra_db`) | Only if the service has a CNPG database |
| `sunbeam.pt/build-target` | Build target name (e.g. `people`) | Only if the service is built from source |
| `sunbeam.pt/depends-on` | Comma-separated service names (e.g. `postgres,openbao`) | Only if the service has dependencies |
| `sunbeam.pt/health-check` | One of: `pod-ready`, `cnpg`, `seal-status`, or an HTTP path like `/healthz` | Only if not the default (`pod-ready`). Omit for services with `HealthCheck::None` |

### Virtual services

Services with **no workload pods** (e.g. `scaleway-s3`, `longhorn`) should be represented by a ConfigMap carrying the labels and an additional marker:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: sunbeam-svc-scaleway-s3
  namespace: external
  labels:
    sunbeam.pt/service: scaleway-s3
    sunbeam.pt/category: infra
    sunbeam.pt/virtual: "true"
  annotations:
    sunbeam.pt/display-name: "Scaleway S3"
    sunbeam.pt/kv-path: scaleway-s3
data: {}
```

### Multi-deployment services

When a service spans multiple Deployments (e.g. `messages` has `messages-backend`, `messages-mta-in`, `messages-mta-out`), **all** Deployments get the **same** `sunbeam.pt/service` label. The CLI groups them automatically. Put the full annotations on just one of them (conventionally the primary/backend Deployment) and minimal labels on the rest.

---

## 2. Complete Service Table

33 services organized by category. The "K8s Resource" column indicates which object to label.

### Auth (namespace: `ory`)

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `hydra` | Deployment `hydra` in `ory` | `service: hydra`, `category: auth` | `display-name: "Hydra (OAuth2/OIDC)"`, `kv-path: hydra`, `db-user: hydra`, `db-name: hydra_db`, `depends-on: postgres,openbao` | |
| `kratos` | Deployment `kratos` in `ory` | `service: kratos`, `category: auth` | `display-name: "Kratos (Identity)"`, `kv-path: kratos`, `db-user: kratos`, `db-name: kratos_db`, `depends-on: postgres,openbao` | |
| `login-ui` | Deployment `login-ui` in `ory` | `service: login-ui`, `category: auth` | `display-name: "Login UI"`, `kv-path: login-ui`, `depends-on: kratos` | |

### Data (namespace: `data`)

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `postgres` | CNPG Cluster `postgres` in `data` | `service: postgres`, `category: data` | `display-name: "PostgreSQL (CNPG)"`, `health-check: cnpg` | Health check is `cnpg`, not `pod-ready` |
| `openbao` | StatefulSet `openbao` in `data` | `service: openbao`, `category: data` | `display-name: "OpenBao (Secrets)"`, `health-check: seal-status` | Health check is `seal-status` |
| `valkey` | Deployment `valkey` in `data` | `service: valkey`, `category: data` | `display-name: "Valkey (Cache)"` | |
| `opensearch` | Deployment `opensearch` in `data` | `service: opensearch`, `category: data` | `display-name: "OpenSearch"` | |

### DevTools (namespace: `devtools`)

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `gitea` | Deployment `gitea` in `devtools` | `service: gitea`, `category: devtools` | `display-name: "Gitea (Git Forge)"`, `kv-path: gitea`, `db-user: gitea`, `db-name: gitea_db`, `depends-on: postgres,openbao` | |

### Messaging

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `stalwart` | Deployment `stalwart` in `stalwart` | `service: stalwart`, `category: messaging` | `display-name: "Stalwart (Mail)"`, `kv-path: stalwart`, `db-user: stalwart`, `db-name: stalwart_db`, `depends-on: postgres,openbao` | |
| `tuwunel` | Deployment `tuwunel` in `matrix` | `service: tuwunel`, `category: messaging` | `display-name: "Tuwunel (Matrix)"`, `kv-path: tuwunel`, `build-target: tuwunel`, `depends-on: openbao` | |

### Media (namespace: `media`)

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `livekit` | Deployment `livekit-server` in `media` | `service: livekit`, `category: media` | `display-name: "LiveKit (WebRTC)"`, `kv-path: livekit`, `depends-on: openbao` | |

### Storage (namespace: `storage`)

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `seaweedfs` | Deployment `seaweedfs-filer` in `storage` | `service: seaweedfs`, `category: storage` | `display-name: "SeaweedFS (S3)"`, `kv-path: seaweedfs`, `depends-on: openbao` | |

### Monitoring (namespace: `monitoring`)

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `grafana` | Deployment `grafana` in `monitoring` | `service: grafana`, `category: monitoring` | `display-name: "Grafana"`, `kv-path: grafana`, `depends-on: openbao` | |
| `prometheus` | Deployment `prometheus` in `monitoring` | `service: prometheus`, `category: monitoring` | `display-name: "Prometheus"` | No KV, no DB, no deps |
| `loki` | Deployment `loki` in `monitoring` | `service: loki`, `category: monitoring` | `display-name: "Loki"` | No KV, no DB, no deps |

### Infra

| Service | K8s Resource | Labels | Annotations | Notes |
|---|---|---|---|---|
| `cilium` | Deployment `cilium-operator` in `kube-system` | `service: cilium`, `category: infra` | `display-name: "Cilium (CNI)"` | No health check (HealthCheck::None) |
| `longhorn` | ConfigMap `sunbeam-svc-longhorn` in `longhorn-system` | `service: longhorn`, `category: infra`, `virtual: "true"` | `display-name: "Longhorn (Storage)"` | Virtual: no deployments listed |
| `cert-manager` | Deployment `cert-manager` in `cert-manager` | `service: cert-manager`, `category: infra` | `display-name: "cert-manager (TLS)"` | No health check |
| `ingress` | Deployment `pingora` in `ingress` | `service: ingress`, `category: infra` | `display-name: "Ingress (Proxy)"`, `depends-on: cert-manager` | No health check |
| `vso` | Deployment `vault-secrets-operator` in `vault-secrets-operator` | `service: vso`, `category: infra` | `display-name: "Vault Secrets Operator"`, `depends-on: openbao` | No health check |
| `headscale` | Deployment `headscale` in `vpn` | `service: headscale`, `category: infra` | `display-name: "Headscale (VPN)"`, `db-user: headscale`, `db-name: headscale_db`, `depends-on: postgres` | No health check, no KV, but has a DB |
| `scaleway-s3` | ConfigMap `sunbeam-svc-scaleway-s3` in `external` | `service: scaleway-s3`, `category: infra`, `virtual: "true"` | `display-name: "Scaleway S3"`, `kv-path: scaleway-s3` | Virtual: external service, no pods |

---

## 3. Example Patches

### Example 1: Standard Deployment (hydra)

```yaml
# Deployment in namespace: ory
apiVersion: apps/v1
kind: Deployment
metadata:
  name: hydra
  namespace: ory
  labels:
    sunbeam.pt/service: hydra
    sunbeam.pt/category: auth
  annotations:
    sunbeam.pt/display-name: "Hydra (OAuth2/OIDC)"
    sunbeam.pt/kv-path: hydra
    sunbeam.pt/db-user: hydra
    sunbeam.pt/db-name: hydra_db
    sunbeam.pt/depends-on: postgres,openbao
```

### Example 2: StatefulSet (openbao)

```yaml
# StatefulSet in namespace: data
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: openbao
  namespace: data
  labels:
    sunbeam.pt/service: openbao
    sunbeam.pt/category: data
  annotations:
    sunbeam.pt/display-name: "OpenBao (Secrets)"
    sunbeam.pt/health-check: seal-status
```

### Example 3: CNPG Cluster (postgres)

```yaml
# CNPG Cluster in namespace: data
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: postgres
  namespace: data
  labels:
    sunbeam.pt/service: postgres
    sunbeam.pt/category: data
  annotations:
    sunbeam.pt/display-name: "PostgreSQL (CNPG)"
    sunbeam.pt/health-check: cnpg
```

### Example 4: External service ConfigMap (scaleway-s3)

```yaml
# No pods exist for this service; use a ConfigMap placeholder
apiVersion: v1
kind: ConfigMap
metadata:
  name: sunbeam-svc-scaleway-s3
  namespace: external
  labels:
    sunbeam.pt/service: scaleway-s3
    sunbeam.pt/category: infra
    sunbeam.pt/virtual: "true"
  annotations:
    sunbeam.pt/display-name: "Scaleway S3"
    sunbeam.pt/kv-path: scaleway-s3
data: {}
```

### Example 5: Multi-deployment service

When a logical service is backed by multiple Deployments, all of them carry the same `sunbeam.pt/service` label. Full annotations go on the primary Deployment; the others only need the labels.

```yaml
# Primary Deployment - carries all annotations
apiVersion: apps/v1
kind: Deployment
metadata:
  name: stalwart
  namespace: stalwart
  labels:
    sunbeam.pt/service: stalwart
    sunbeam.pt/category: messaging
  annotations:
    sunbeam.pt/display-name: "Stalwart (Mail)"
    sunbeam.pt/kv-path: stalwart
    sunbeam.pt/db-user: stalwart
    sunbeam.pt/db-name: stalwart_db
    sunbeam.pt/depends-on: postgres,openbao
---
# Secondary Deployment - labels only
apiVersion: apps/v1
kind: Deployment
metadata:
  name: stalwart-worker
  namespace: stalwart
  labels:
    sunbeam.pt/service: stalwart
    sunbeam.pt/category: messaging
```

---

## 4. Verification

### List all sunbeam-managed services

```bash
kubectl get deploy,sts,cm -A -l sunbeam.pt/service
```

### Check a specific service

```bash
kubectl get deploy -n ory -l sunbeam.pt/service=hydra \
  -o jsonpath='{.items[0].metadata.annotations}'
```

### List all services in a category

```bash
kubectl get deploy,sts,cm -A -l sunbeam.pt/category=platform
```

### List virtual services only

```bash
kubectl get cm -A -l sunbeam.pt/virtual=true
```

### Verify all 33 services are discoverable

```bash
kubectl get deploy,sts,cm -A -l sunbeam.pt/service \
  -o jsonpath='{range .items[*]}{.metadata.labels.sunbeam\.pt/service}{"\n"}{end}' \
  | sort -u | wc -l
# Expected: 33
```

### Test with the CLI after labeling

```bash
sunbeam status          # Should list all services
sunbeam logs hydra      # Should find hydra pods via label selector
sunbeam restart auth    # Should restart all services in the auth category
```

---

## Notes

- The `sunbeam.pt/health-check` annotation defaults to `pod-ready` when omitted. Only set it explicitly for `cnpg`, `seal-status`, or custom HTTP paths.
- Services with `HealthCheck::None` in the registry (all Infra services) should **not** have a `health-check` annotation. The CLI treats missing health-check on infra services as "no active health monitoring."
- The `sunbeam.pt/virtual` label is only needed on ConfigMap placeholders for services with no workload resources. The two virtual services are `longhorn` and `scaleway-s3`.
- Namespace is not encoded as a label/annotation because it is intrinsic to the Kubernetes resource itself.
