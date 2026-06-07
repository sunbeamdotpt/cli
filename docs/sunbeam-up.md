# `sunbeam up` — Complete Cluster Bring-Up

> **One command to go from zero to a working local Kubernetes development stack.**

```bash
sunbeam up
```

That single command provisions a VM (if needed), installs a Kubernetes cluster, bootstraps a secrets engine, generates TLS certificates, deploys a full platform stack (ingress, identity, storage, registry, databases), builds your workspace container images, and prints a list of URLs where everything is reachable.

This document explains what happens internally, how long it takes, where state lives, and how the OpenBao root token is generated, stored, and recovered.

---

## Table of Contents

1. [Quick Start](#quick-start)
2. [What `sunbeam up` Actually Does](#what-sunbeam-up-actually-does)
3. [The Twelve Phases of Bring-Up](#the-twelve-phases-of-bring-up)
4. [OpenBao, Root Tokens, and the Local Keystore](#openbao-root-tokens-and-the-local-keystore)
5. [Configuration and State Files](#configuration-and-state-files)
6. [Common Usage Patterns](#common-usage-patterns)
7. [Troubleshooting](#troubleshooting)
8. [CLI Reference](#cli-reference)

---

## Quick Start

If this is your first time:

```bash
# 1. Configure your context
sunbeam config set \
  --domain sunbeam.pt \
  --infra-dir ~/code/infra \
  --kube-context lima-sunbeam \
  --acme-email ops@sunbeam.pt

# 2. Run the full bring-up (local Lima VM mode)
sunbeam up --use-lima

# 3. Wait 10–20 minutes on a fresh machine
```

On completion you will see:

```
==> URLs
    https://auth.sunbeam.pt        (Kratos — identity management)
    https://git.sunbeam.pt         (Gitea)
    https://builds.sunbeam.pt      (WFE CI)
    https://grafana.sunbeam.pt     (Monitoring)
    https://oci.sunbeam.pt         (Zot container registry)
```

---

## What `sunbeam up` Actually Does

`sunbeam up` is not a shell script. It is a **workflow-orchestrated** cluster bring-up powered by the WFE (Workflow Engine) embedded inside the CLI. The workflow definition is versioned Rust code (`workflows/up/definition.rs`, version 3). When you run `sunbeam up`, the CLI:

1. Resolves your active **context** (`~/.sunbeam/config.json`).
2. Optionally loads a **profile** (`infra/profiles/<name>.yaml`) that can skip namespaces, enable serial mode, or inject manifest overrides.
3. Parses CLI overrides like `--set kind/namespace/name/field/path=value` or `--disable`.
4. Creates a local **WFE host** backed by SQLite (`~/.sunbeam/<context>/workflows.db`).
5. Registers every step primitive (ApplyManifest, WaitForRollout, SeedKVPath, …) and the up-specific steps (EnsureLimaVm, BootstrapCriticalImages, …).
6. Runs the workflow **synchronously** with a 1-hour timeout.
7. Prints a summary of every step and the final URLs.

If any step fails, the workflow **terminates immediately** (`ErrorBehavior::Terminate`). You can inspect the failure with:

```bash
sunbeam workflow status <id>
```

Or simply re-run `sunbeam up` — most steps are idempotent.

### Why a Workflow Engine?

Because a modern stack has dozens of moving parts with complex ordering constraints:

- cert-manager must be ready before any `Certificate` or `ClusterIssuer` is applied.
- Longhorn must be ready before any `PersistentVolumeClaim` is created.
- CNPG (CloudNativePG) webhooks must be online before Postgres clusters can be created.
- OpenBao must be initialized and unsealed before KV secrets can be written.
- Those KV secrets must exist before Helm charts reference them in `envFrom`.
- The ingress controller must be ready before anything pushing to `oci.*` or `src.*`.
- Ory (Hydra/Kratos) runs database migrations as Helm hooks; `kubectl apply` does **not** wait for hooks, so explicit `WaitForRollout` steps are required.

WFE models these as a DAG with parallel branches and sequential dependencies. The result is reliable, observable, and retryable.

---

## The Twelve Phases of Bring-Up

Here is the exact order of execution, with narrative explanations of why each phase exists and what you would see in the logs.

---

### Phase 0 — Lima VM (`ensure-lima-vm`)

**Only runs when `--profile lima` or `--use-lima` is set.**

If you are developing locally on macOS or Linux, Sunbeam can provision a Lima VM that runs k3s, Cilium, BuildKit, and containerd. The VM spec is embedded at build time from `lima-sunbeam.yaml`.

What happens:

1. `limactl` is verified to exist.
2. If the `sunbeam` VM does not exist, it is created from the embedded YAML.
3. If it exists but is stopped, it is started.
4. The step polls `limactl list sunbeam --format '{{.Status}}'` for up to **5 minutes**.
5. Once the VM is `Running`, it waits for the k3s kubeconfig at `~/.lima/sunbeam/copied-from-guest/kubeconfig.yaml`.
6. It probes the k3s API by listing nodes with the Rust kube client.
7. It **merges** the Lima kubeconfig into `~/.kube/config`, renaming the context to `lima-sunbeam` so it never clashes with your existing contexts.
8. It sets `current-context: lima-sunbeam`.

**Example output:**

```
==> Ensuring Lima VM 'sunbeam'...
    Lima VM 'sunbeam' is already running.
    k3s API is reachable.
    Host kubeconfig updated.
```

**Lima VM resources:** 6 CPUs, 16 GiB RAM, 60 GiB disk, Ubuntu 26.04 LTS.

---

### Phase 1 — Infrastructure

This phase lays the foundation every other namespace depends on.

#### 1a. Cilium (`ensure-cilium`)

Lists pods with label `k8s-app=cilium` in `kube-system` / `cilium-system`. On a fresh Lima install, Cilium is installed by a provision script inside the VM with `kubeProxyReplacement=true`. This step waits up to **5 minutes** for Cilium to be ready.

It also resolves the cluster domain dynamically from the live cluster (using the Lima VM's InternalIP) and injects it into workflow data so later steps use the correct domain.

#### 1b. TLS Certificates (`ensure-tls-cert`, `ensure-tls-secret`)

Before anything serving HTTPS can start, TLS material must exist.

- `EnsureTLSCert` generates a **self-signed wildcard certificate** for `*.<domain>` using `rcgen` if one does not already exist at `~/.sunbeam/<context>/secrets/tls.crt`.
- It also updates Docker's `daemon.json` to add `src.<domain>` and `oci.<domain>` to `insecure-registries`.
- `EnsureTLSSecret` base64-encodes the cert and key and patches two Kubernetes secrets via server-side apply:
  - `ingress/pingora-tls` (type `kubernetes.io/tls`)
  - `vpn/headscale-tls`

**Example:**

```bash
# After the first run, your certs live here:
ls ~/.sunbeam/production/secrets/
# tls.crt  tls.key
```

#### 1c. cert-manager (`apply-cert-manager`, `wait-cert-manager`, `wait-cert-manager-webhook`)

cert-manager is applied first because its validating webhook rejects `Certificate` and `ClusterIssuer` creates if it is not ready. The apply skips `scaleway-dns-credentials` and `vso-auth` (those are created later). After apply, the step waits for the cert-manager deployment and then polls the `APIService v1.cert-manager.io` for `Available=True`.

#### 1d. Longhorn (`apply-longhorn`, `wait-longhorn-webhook`)

Longhorn provides block storage. It must be ready before any PVCs are created. The step applies the `longhorn-system` namespace and waits for the `longhorn-manager` DaemonSet to report readiness.

#### 1e. Data + Build namespaces (parallel)

The `data` namespace (Postgres, OpenBao, OpenSearch, Valkey) and the `build` namespace (BuildKit DaemonSet) are applied in parallel.

#### 1f. CNPG Webhook (`wait-cnpg-webhook`)

CloudNativePG uses a validating webhook. The step polls the `cloudnative-pg` deployment and then checks that `cnpg-webhook-service` endpoints are non-empty. Timeout: **3 minutes**.

#### 1g. BuildKit (`wait-buildkitd`)

Waits for the `buildkitd` deployment in the `build` namespace. This is the in-cluster BuildKit that later phases use for image builds.

---

### Phase 1b — Bootstrap Critical Images (`bootstrap-critical-images`)

This is a **chicken-and-egg breaker** specific to local Lima deployments.

The ingress controller (`pingora`) uses an image pulled from `src.<domain>`. But `src.<domain>` is served by the ingress controller itself. On a fresh install, the proxy image does not exist in the cluster's containerd, so `pingora` can never start, so `src.<domain>` is never reachable.

This step:

1. Builds the `proxy` image locally using host Docker / BuildKit over TCP `127.0.0.1:1234`.
2. Exports the image as a tar.
3. Imports it into k3s containerd via an ephemeral `ctr` pod that mounts the host containerd socket.

After this, `pingora` can start because its image is already present.

---

### Phase 2 — OpenBao Init (`find-openbao-pod`, `wait-pod-running`, `init-or-unseal-openbao`)

This is one of the most important phases. It finds the OpenBao pod, waits for it to be `Running`, then initializes or unseals it.

See [OpenBao, Root Tokens, and the Local Keystore](#openbao-root-tokens-and-the-local-keystore) for the full deep-dive.

**High-level flow:**

1. Find the pod by label `app.kubernetes.io/name=openbao,component=server` in namespace `data`.
2. Wait up to 5 minutes for it to be `Running`.
3. Open a port-forward to the pod on port 8200 (retried 10 times).
4. Poll the seal status API up to 30 times.
5. If **not initialized**: call `bao.init(1, 1)` (1 key share, 1 threshold), store the unseal key and root token in the K8s secret `data/openbao-keys`, and save them to the **local encrypted keystore**.
6. If **initialized but sealed**: retrieve the unseal key from the K8s secret (or local keystore as fallback), then unseal.
7. If **initialized but root token is lost**: reset storage by deleting the PVC and pod, wait for restart, then re-initialize.
8. Enable the KV v2 secrets engine at path `secret`.

---

### Phase 3 — KV Seeding (parallel per-service)

Every service in the stack needs credentials: database passwords, cookie secrets, OAuth2 client secrets, S3 keys, etc. Rather than scattering these in Git or env vars, Sunbeam generates them centrally in OpenBao and then uses the **Vault Secrets Operator (VSO)** to sync them into Kubernetes Secrets at runtime.

This phase loops over `kv_service_configs::all_service_configs()` and, in parallel for each service:

1. `SeedKVPath` — generates random values for every field defined in the service config.
2. `WriteKVPath` — writes those values into OpenBao at `secret/data/<service>`.

There is an extra branch for `kratos-admin` (which depends on SeaweedFS credentials).

After all branches complete, `CollectCredentials` aggregates the generated secrets into a single JSON blob for later use.

**Example:**

```bash
# After up completes, you can inspect what was seeded:
sunbeam secrets kv get hydra
# KEY              VALUE
# secretsSystem    hvs.CAESIPx...
# secretsCookie    hvs.CAESIKy...
# pairwise-salt    hvs.CAESILz...
```

---

### Phase 3b — Vault Auth (`enable-k8s-auth`, `write-k8s-auth-config`, `write-vso-policy`, `write-vso-role`)

For VSO to sync secrets, it needs to authenticate to OpenBao using Kubernetes service-account JWTs. This phase sets up the Kubernetes auth method:

1. **EnableVaultAuth** — mounts the `kubernetes` auth method.
2. **WriteVaultAuthConfig** — configures `kubernetes_host` to `https://kubernetes.default.svc.cluster.local`.
3. **WriteVaultPolicy** — creates `vso-reader` policy with read access to `secret/data/*`, `secret/metadata/*`, and `database/static-creds/*`.
4. **WriteVaultRole** — creates the `vso` role bound to service accounts across 13 namespaces, with a 1-hour TTL.

---

### Phase 4 — PostgreSQL

The stack uses CloudNativePG (CNPG) for Postgres. This phase:

1. `WaitForPostgres` — polls the CNPG primary in `data` namespace until it accepts connections.
2. In parallel branches: `CreatePGRole` → `CreatePGDatabase` for every entry in `pg_db_map()` (kratos, hydra, keto, penpot, stalwart, headscale, wfe, press).
3. `ConfigureDatabaseEngine` — enables the OpenBao `database` secrets engine, creates a `vault` PG user with `CREATEROLE` and `ADMIN OPTION`, writes the DB config `cnpg-postgres`, and creates static roles with a 24-hour rotation period.

This means applications never hardcode database passwords. They receive short-lived credentials from OpenBao via VSO.

---

### Phase 5 — Ensure Namespaces + Create Kubernetes Secrets

Many Helm charts reference secrets at pod creation time via `envFrom`. If the secret does not exist, the pod enters `CreateContainerConfigError` and never recovers. This phase creates those secrets **before** the manifests that reference them are applied.

Parallel branches:

- **ory**: creates `hydra` and `kratos-app-secrets` with placeholder values (real values come from VSO later, but placeholders prevent CrashLoopBackOff).
- **storage**: creates `seaweedfs-s3-credentials` and `seaweedfs-s3-json`.
- **ingress, devtools, media, stalwart, vpn, oci, vault-secrets-operator**: ensures namespaces exist.

---

### Phase 6 — Platform Manifests

Now the main platform is applied in parallel:

- `vault-secrets-operator` (VSO CRDs and controllers)
- `ingress` (Pingora ingress controller)
- `ory` (Hydra + Kratos identity stack)
- `devtools` (Gitea, Registry, Builder)
- `storage` (SeaweedFS)
- `media` (Media server)
- `stalwart` (Mail server)
- `vpn` (Headscale)
- `oci` (Zot container registry)

Then:

1. **Wait for ingress** (`wait-ingress`) — Pingora must be ready before anything needs external connectivity.
2. **Re-apply data namespace** (`reapply-data`) — the first apply of `data` skipped `VaultAuth` and `VaultStaticSecret` resources because VSO CRDs did not exist yet. Now they do, so they are created.
3. **Wait for Ory migrations** (`wait-ory-hydra`, `wait-ory-kratos`) — Helm hooks run DB migrations. These waits block until Hydra and Kratos deployments are actually ready (up to 5 minutes each).
4. **Ensure SeaweedFS buckets** — creates the `zot` S3 bucket that Zot will use as backend storage.
5. **Wait for Zot** (`wait-zot-early`) — the registry must be ready before image builds push to it.
6. **Build project images** (`build-project-images`) — discovers all projects in your workspace from `sunbeam.workspace.yaml`, topologically sorts them by `deps.projects`, and runs `sunbeam project package` sequentially. This keeps peak disk usage bounded on fresh installs.

---

### Phase 7 — Application Manifests

The application layer is applied in parallel:

- `matrix` (Tuwunel, Element)
- `wfe` (Workflow engine server)
- `press` (CMS / publishing)

---

### Phase 8 — Core Rollouts + OpenSearch ML

Parallel waits for:

- `valkey` (caching layer in `data`)
- `kratos` (identity in `ory`)
- `hydra` (OAuth2 in `ory`)
- `EnsureOpenSearchML` — downloads and deploys the ML model for OpenSearch semantic search. This can take 10+ minutes on first run, so it runs in parallel with the rollout waits.

Then `InjectOpenSearchModelId` updates the OpenSearch configuration with the deployed model ID.

---

### Phase 9 — Kratos Admin Identity (`seed-kratos-admin-identity`)

**New in workflow v3.** This was missing in v2, which meant no admin user existed after `sunbeam up` and nobody could log in.

This step creates the first admin identity in Kratos so the stack is login-ready immediately.

---

### Phase 10 — Application Rollouts

More parallel waits to ensure downstream steps don't race:

- `tuwunel` (Matrix homeserver)
- `wfe-server` (CI server)
- `zot` (OCI registry)
- `press` (CMS)
- `kratos-admin-ui` (Admin dashboard)

---

### Phase 11 — Observability (`apply-monitoring`, `wait-grafana`)

Monitoring is applied **late** (moved from Phase 1 in v3) because Grafana uses Hydra for OIDC login and needs OpenBao secrets synced by VSO. Both of those prerequisites are only guaranteed after Phase 8.

The step applies the `monitoring` namespace and waits for `kube-prometheus-stack-grafana`.

---

### Phase 12 — Finalize (`mint-vpn-preauth-keys`, `print-urls`)

1. **Mint VPN pre-auth keys** — enters the Headscale pod and mints two pre-auth keys:
   - A **router key** (`tag:router`) → stored in K8s Secret `vpn/subnet-router-authkey`.
   - A **user key** (`tag:user`) → stored in your context's `vpn-auth-key` field in `~/.sunbeam/config.json`.

2. **Print URLs** — prints the final list of reachable endpoints.

**Example final output:**

```
==> URLs
    https://auth.sunbeam.pt
    https://git.sunbeam.pt
    https://builds.sunbeam.pt
    https://grafana.sunbeam.pt
    https://oci.sunbeam.pt
```

---

## OpenBao, Root Tokens, and the Local Keystore

OpenBao (a HashiCorp Vault fork) is the secrets backbone of the stack. It stores:

- KV v2 secrets for every service (database passwords, cookie salts, API keys).
- Database static-role configurations (credentials rotated every 24 hours).
- Transit keys for encryption-at-rest.

Because OpenBao is **seal-wrapper based**, it starts sealed after any pod restart. It must be unsealed with an unseal key, and administrative operations require a **root token**.

This section is long because losing the root token is painful. Understanding exactly where it lives, how it is protected, and how to recover it will save you hours.

---

### Where the Root Token Lives

Sunbeam stores OpenBao credentials in **two places** for redundancy:

#### 1. Kubernetes Secret (`data/openbao-keys`)

A standard K8s secret in the `data` namespace with two keys:

- `key` — the unseal key (base64-encoded).
- `root-token` — the root token.

This is convenient for the cluster: VSO reads it to authenticate, and the `sunbeam secrets` command resolves it automatically by reading this secret. But if the secret is accidentally deleted, overwritten, or the cluster is recreated from scratch, the keys are lost.

**View the secret:**

```bash
kubectl get secret openbao-keys -n data -o yaml
```

**Extract the root token:**

```bash
kubectl get secret openbao-keys -n data -o jsonpath='{.data.root-token}' | base64 -d
```

**Extract the unseal key:**

```bash
kubectl get secret openbao-keys -n data -o jsonpath='{.data.key}' | base64 -d
```

#### 2. Local Encrypted Keystore (`~/.sunbeam/vault/<domain>.enc`)

This is your **disaster-recovery copy**. It is encrypted with **AES-256-GCM** and bound to your machine. Even if the entire Kubernetes cluster is deleted and recreated, this file lets you restore the unseal key and root token.

---

### How the Keystore Is Encrypted

The encryption is designed so that the keystore file is **useless if stolen** without also compromising your machine and knowing the domain.

**Algorithm:** AES-256-GCM  
**Key derivation:** Argon2id  
**Machine binding:** A 32-byte machine-specific salt stored at `~/.sunbeam/.machine-salt` (permissions `0600`)

**Key derivation input:**

```
key = Argon2id(machine_salt + "sunbeam-vault-keystore:" + domain, argon2_salt)
```

The `machine_salt` is generated once per machine and never leaves it. The `domain` is your context's domain suffix (e.g. `sunbeam.pt`). This means:

- The keystore **cannot be decrypted on a different machine** (different machine salt).
- The keystore is **domain-bound** — you cannot rename the file and use it for another domain.
- Even if someone gains access to the file, they must also know the domain and have the machine salt.

**Ciphertext format on disk:**

```
[nonce (12 bytes)][argon2_salt (16 bytes)][ciphertext + AES-GCM tag]
```

**File permissions:** `0600` (owner read/write only).

---

### Keystore Structure (Plaintext)

When decrypted, the keystore is a JSON file with this structure:

```json
{
  "version": 1,
  "domain": "sunbeam.pt",
  "created_at": "2026-01-15T10:23:00Z",
  "updated_at": "2026-01-15T10:23:00Z",
  "root_token": "hvs.CAESIPx4j7mK3N2Z...",
  "unseal_keys_b64": ["dGVzdC11bnNlYWwta2V5"],
  "key_shares": 1,
  "key_threshold": 1
}
```

- `version` — keystore format version (currently 1).
- `domain` — the domain this keystore belongs to. Must match the decryption domain or decryption fails.
- `root_token` — the OpenBao root token. This is the "master key" that can do anything.
- `unseal_keys_b64` — base64-encoded unseal keys. With the default `1, 1` init policy, there is exactly one.
- `key_shares` / `key_threshold` — Shamir sharing parameters. For local dev this is `1, 1`.

---

### How `sunbeam up` Handles Token Lifecycle

The `init-or-unseal-openbao` step in the workflow is responsible for the entire token lifecycle. Here is exactly what happens in every scenario.

---

#### Scenario A: First Install (Fresh OpenBao)

**What you see:**

```
init-or-unseal-openbao:
  OpenBao is fresh (uninitialized).
  Initializing OpenBao...
  Initialized -- keys stored in secret/openbao-keys.
  Keys saved to local keystore.
  Enabling KV engine...
```

**What happens internally:**

1. The step opens a port-forward to the OpenBao pod on port 8200 (retried 10 times).
2. It polls the seal status API up to 30 times until it responds.
3. OpenBao reports `initialized: false, sealed: true`.
4. It calls `bao.init(1, 1)` (1 key share, 1 threshold — enough for local dev). Retried 5 times with backoff.
5. The returned unseal key and root token are written to the K8s secret `data/openbao-keys`.
6. The same credentials are encrypted and saved to `~/.sunbeam/vault/<domain>.enc`.
7. The KV v2 engine is enabled at path `secret/`.
8. The workflow data is updated with `root_token` so later steps can use it.

**Result:** Both the cluster and your laptop have a copy of the credentials.

---

#### Scenario B: Re-run `sunbeam up` (OpenBao Already Initialized)

**What you see:**

```
init-or-unseal-openbao:
  Already initialized.
  Unsealing...
```

**What happens internally:**

1. The seal status API reports `initialized: true, sealed: true`.
2. The unseal key is read from the K8s secret `data/openbao-keys`.
3. `bao.unseal(key)` is called.
4. OpenBao transitions to `sealed: false`.
5. The root token is read from the K8s secret and passed to later steps.

**If the local keystore is missing but the cluster secret exists:** the keystore is **backfilled** from the cluster secret. This ensures your disaster-recovery copy stays in sync.

---

#### Scenario C: Cluster Secret Deleted, Keystore Exists

**What you see:**

```
init-or-unseal-openbao:
  Already initialized.
  Cluster secret missing keys — restoring from local keystore...
  Cluster secret restored from local keystore.
  Unsealing...
```

**What happens internally:**

1. The seal status API reports `initialized: true`.
2. The K8s secret `data/openbao-keys` is checked — it exists but the `key` or `root-token` field is empty or `placeholder`.
3. The local keystore is loaded and decrypted.
4. The missing fields are restored into the K8s secret.
5. Unseal proceeds normally.

**This is the primary disaster-recovery flow.** As long as you have `~/.sunbeam/vault/<domain>.enc`, you can recover from a deleted or corrupted cluster secret.

---

#### Scenario D: Token Completely Lost (Neither Cluster Nor Keystore Has It)

**What you see:**

```
init-or-unseal-openbao:
  Vault is initialized but root token is missing -- resetting storage...
  Waiting for OpenBao pod to restart...
  OpenBao is fresh (uninitialized).
  Initializing OpenBao...
```

**What happens internally:**

1. The seal status API reports `initialized: true`.
2. The K8s secret is checked — empty or missing.
3. The local keystore is checked — missing or empty.
4. **Nuclear option:** The PVC `data-openbao-0` is deleted (wipes Raft storage).
5. The OpenBao pod is deleted.
6. A new pod is created with empty storage.
7. OpenBao reports `initialized: false`.
8. The step falls through to the initialization block (Scenario A).
9. **New keys are generated.** The old root token is gone forever.

> ⚠️ **This resets all secrets stored in OpenBao.** The stack will re-seed KV values during the same `sunbeam up` run, but any manually created secrets, transit keys, or database credentials will need to be regenerated. Your applications will receive new passwords from VSO, but you may need to restart pods to pick them up.

---

### How the CLI Automatically Resolves the Root Token

You almost never type the root token manually. Every `sunbeam secrets` subcommand resolves it automatically through this chain:

**Resolution order (first match wins):**

1. **CLI override:** `--token <token>` passed on the command line.
2. **K8s secret:** Read from `data/openbao-keys` field `root-token`.
3. **Local keystore:** Load `~/.sunbeam/vault/<domain>.enc`, decrypt with machine salt + domain, return `root_token` field.
4. **Fail:** `SunbeamError::Config("No OpenBao token found. Run sunbeam up to initialize, or pass --token")`

This is implemented in `sunbeam-sdk/src/secrets_cli.rs` in the `read_token()` function:

```rust
async fn read_token() -> Result<String> {
    // 1. Try K8s secret
    match crate::kube::kube_get_secret_field("data", "openbao-keys", "root-token").await {
        Ok(token) if !token.is_empty() => return Ok(token),
        _ => {}
    }

    // 2. Try local keystore
    let domain = crate::config::domain();
    if !domain.is_empty() {
        if let Ok(ks) = crate::vault_keystore::load_keystore(&domain) {
            if !ks.root_token.is_empty() {
                return Ok(ks.root_token);
            }
        }
    }

    Err(SunbeamError::Config(
        "No OpenBao token found. Run `sunbeam up` to initialize, or pass `--token`".into(),
    ))
}
```

**What this means for you:**

```bash
# This works automatically — no token needed
sunbeam secrets kv get hydra

# This also works automatically
sunbeam secrets status

# Override for a remote instance
sunbeam secrets --addr https://vault.other.com --token hvs.xxx status
```

---

### How to Access the Root Token Manually

There are four ways to get the raw token, depending on your goal.

#### Method 1 — From the Kubernetes Secret (Cluster Access)

```bash
kubectl get secret openbao-keys -n data -o jsonpath='{.data.root-token}' | base64 -d
```

**When to use:** You have kubectl access and just need the token for a one-off API call.

#### Method 2 — From the Local Keystore (Offline Access)

There is no dedicated CLI command to print the plaintext token (by design — it reduces accidental exposure in shell history). But you can export it:

```bash
# Write a tiny Rust program or use cargo-script
cat > /tmp/export_token.rs << 'EOF'
fn main() {
    let domain = std::env::args().nth(1).expect("usage: export_token <domain>");
    // This uses the internal API; run from the sunbeam-sdk crate directory
    let ks = sunbeam_sdk::vault_keystore::load_keystore(&domain).unwrap();
    println!("{}", ks.root_token);
}
EOF
```

Or more practically, inspect the file indirectly:

```bash
# Confirm the keystore exists
ls ~/.sunbeam/vault/

# Check which domain it belongs to (derived from filename)
ls ~/.sunbeam/vault/*.enc
```

#### Method 3 — Export for Migration

The `vault_keystore::export_plaintext(domain)` function exists for exactly this purpose. It decrypts the keystore and outputs pretty-printed JSON that you can transfer to another machine.

**Example using a test harness:**

```bash
cd /path/to/sunbeam-sdk
cat > /tmp/export.rs << 'EOF'
#[tokio::main]
async fn main() {
    let domain = "sunbeam.pt";
    let json = sunbeam_sdk::vault_keystore::export_plaintext(domain).unwrap();
    println!("{}", json);
}
EOF
```

> ⚠️ **Treat the exported JSON as a secret.** It contains the root token in plaintext. Delete the file immediately after use.

#### Method 4 — Programmatic Access in Rust

```rust
use sunbeam_sdk::vault_keystore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let domain = "sunbeam.pt";
    let ks = vault_keystore::load_keystore(domain)?;

    println!("Root token: {}", ks.root_token);
    println!("Unseal key: {}", ks.unseal_keys_b64[0]);
    println!("Created:    {}", ks.created_at);
    println!("Updated:    {}", ks.updated_at);

    Ok(())
}
```

---

### How to Back Up the Keystore

The keystore file is already encrypted, so you can back it up safely as long as you also back up the machine salt.

**Minimal backup (same machine restore):**

```bash
# Just the keystore file
cp ~/.sunbeam/vault/sunbeam_pt.enc ~/backups/
```

**Full backup (cross-machine migration):**

```bash
# The keystore file AND the machine salt
cp ~/.sunbeam/vault/sunbeam_pt.enc ~/backups/
cp ~/.sunbeam/.machine-salt ~/backups/
```

> ⚠️ **The machine salt is required for decryption.** If you only back up the `.enc` file and lose the machine salt, the keystore is permanently unreadable.

---

### How to Migrate the Keystore to a New Machine

Because the keystore is bound to the machine salt, you cannot simply copy the `.enc` file to a new laptop. You have two options:

#### Option A: Export and Re-import

1. **On the old machine**, export the plaintext keystore:

   ```rust
   let json = vault_keystore::export_plaintext("sunbeam.pt")?;
   // Save json to a secure channel (1Password, encrypted USB, etc.)
   ```

2. **On the new machine**, after running `sunbeam up` once (which creates a new keystore), manually patch the token:

   ```bash
   # Use the secrets CLI with the old token to write the new cluster secret
   sunbeam secrets --token <old-token> kv put openbao-temp root_token=<old-token>
   ```

   Or more directly, edit the K8s secret:

   ```bash
   kubectl patch secret openbao-keys -n data --type merge \
     -p '{"data":{"root-token":"'$(echo -n <old-token> | base64)'","key":"'$(echo -n <old-unseal-key> | base64)'"}}'
   ```

3. Run `sunbeam up` again. The step will detect the cluster secret and backfill the new machine's local keystore.

#### Option B: Copy Both Files

If you have access to both the `.enc` file and the `.machine-salt` file from the old machine:

```bash
# On new machine
mkdir -p ~/.sunbeam/vault
cp /mnt/old-machine/.machine-salt ~/.sunbeam/
cp /mnt/old-machine/vault/sunbeam_pt.enc ~/.sunbeam/vault/
```

This works because the key derivation uses the exact same salt. The new machine now has the same decryption key as the old one.

> ⚠️ **Security note:** Copying the machine salt means both machines can decrypt the keystore. If the old machine is compromised or sold, rotate the OpenBao root token afterwards.

---

### How to Rotate the Root Token

There is no automated rotation command yet. Here is the manual process:

1. **Generate a new root token** via the OpenBao API:

   ```bash
   # Get the current token first
   OLD_TOKEN=$(kubectl get secret openbao-keys -n data -o jsonpath='{.data.root-token}' | base64 -d)

   # Create a new token with root policy
   kubectl exec -n data deployment/openbao -- bao token create -policy=root -orphan
   ```

2. **Update the K8s secret** with the new token:

   ```bash
   NEW_TOKEN="hvs.CAESI..."
   kubectl patch secret openbao-keys -n data --type merge \
     -p '{"data":{"root-token":"'$(echo -n "$NEW_TOKEN" | base64)'"}}'
   ```

3. **Update the local keystore** by loading, modifying, and saving:

   ```rust
   let mut ks = vault_keystore::load_keystore("sunbeam.pt")?;
   ks.root_token = new_token;
   ks.updated_at = Utc::now();
   vault_keystore::save_keystore(&ks)?;
   ```

4. **Revoke the old token**:

   ```bash
   kubectl exec -n data deployment/openbao -- bao token revoke "$OLD_TOKEN"
   ```

5. **Restart VSO pods** so they pick up the new token:

   ```bash
   kubectl rollout restart deployment/vault-secrets-operator -n vault-secrets-operator
   ```

---

### How to Verify Keystore Integrity

If you suspect the keystore is corrupted, you can verify it:

```rust
use sunbeam_sdk::vault_keystore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ks = vault_keystore::verify_vault_keys("sunbeam.pt")?;
    println!("Keystore is valid. Token length: {}", ks.root_token.len());
    println!("Unseal keys: {}", ks.unseal_keys_b64.len());
    Ok(())
}
```

`verify_vault_keys` checks:
- The file decrypts successfully
- `root_token` is non-empty
- `unseal_keys_b64` is non-empty
- `key_shares` > 0
- `key_threshold` > 0 and ≤ `key_shares`

---

### What Happens When You Delete `~/.sunbeam/vault/`

If you delete the entire `vault/` directory:

- **Nothing breaks immediately.** The cluster still has the K8s secret.
- The next `sunbeam up` will **backfill** the keystore from the cluster secret.
- If the cluster secret is also deleted before the backfill, you enter **Scenario D** (nuclear reset).

**Safe cleanup:**

```bash
# If you want to start completely fresh on the same domain:
sunbeam down --yes --keep-data
rm ~/.sunbeam/vault/sunbeam_pt.enc
sunbeam up
# This will generate NEW keys and overwrite the cluster secret.
```

---

### What Happens When You Switch Contexts

Each context has its own domain, and each domain has its own keystore file:

```bash
# Context A: production
sunbeam config use-context production
ls ~/.sunbeam/vault/
# sunbeam_pt.enc

# Context B: staging (different domain)
sunbeam config use-context staging
ls ~/.sunbeam/vault/
# sunbeam_pt.enc  staging_sunbeam_pt.enc
```

The `sunbeam secrets` command always uses the **active context's domain** to pick the correct keystore. There is no risk of cross-context token leakage.

---

### Token Security Checklist

- [ ] `~/.sunbeam/.machine-salt` has permissions `0600`
- [ ] `~/.sunbeam/vault/*.enc` has permissions `0600`
- [ ] The keystore is backed up along with the machine salt
- [ ] The K8s secret `data/openbao-keys` exists and has non-placeholder values
- [ ] You know how to extract the token from both the cluster secret and local keystore
- [ ] You have tested disaster recovery at least once (delete the cluster secret and run `sunbeam up`)

---

### Quick Reference: Root Token Commands

| Task | Command |
|------|---------|
| View cluster secret | `kubectl get secret openbao-keys -n data -o yaml` |
| Extract token | `kubectl get secret openbao-keys -n data -o jsonpath='{.data.root-token}' \| base64 -d` |
| Check keystore exists | `ls ~/.sunbeam/vault/` |
| Check seal status | `sunbeam secrets status` |
| List KV secrets | `sunbeam secrets kv list` |
| Read a secret | `sunbeam secrets kv get hydra` |
| Unseal manually | `sunbeam secrets unseal <key>` |
| Override token | `sunbeam secrets --token <t> ...` |

---

## Configuration and State Files

| Path | Purpose |
|------|---------|
| `~/.sunbeam/config.json` | Main config — contexts, profiles, workflow targets, VPN keys. |
| `~/.sunbeam/<context>/` | Per-context directory. |
| `~/.sunbeam/<context>/secrets/tls.crt` | Self-signed wildcard TLS certificate. |
| `~/.sunbeam/<context>/secrets/tls.key` | Private key for the wildcard cert. |
| `~/.sunbeam/<context>/workflows.db` | WFE SQLite database (workflow instances, execution pointers). |
| `~/.sunbeam/vault/` | Encrypted vault keystores (one per domain). |
| `~/.sunbeam/.machine-salt` | 32-byte salt for keystore encryption. |
| `~/.kube/config` | Merged kubectl config (includes `lima-sunbeam` context). |

### Context Structure

```json
{
  "current-context": "production",
  "contexts": {
    "production": {
      "domain": "sunbeam.pt",
      "kube-context": "lima-sunbeam",
      "infra-dir": "/Users/sienna/code/infra",
      "acme-email": "ops@sunbeam.pt",
      "profile": { "name": "lima" },
      "vpn-url": "",
      "vpn-auth-key": "",
      "vpn-cluster-host": "",
      "vpn-api-key": "",
      "vpn-tls-insecure": false,
      "vpn-dns-server": "",
      "vpn-dns-search": ""
    }
  }
}
```

---

## Common Usage Patterns

### Fresh Local Install

```bash
sunbeam up --use-lima
```

### Re-run After Changing Manifests

```bash
# Most steps are idempotent; only changed manifests are re-applied.
sunbeam up
```

### Skip a Namespace

```bash
sunbeam up --disable matrix --disable press
```

### Override a Manifest Field

```bash
sunbeam up --set deployment/ory/kratos/spec/replicas=3
```

### Use a Profile

Profiles live in `infra/profiles/<name>.yaml`. They can define shortcuts, skip lists, and serial mode.

```bash
sunbeam up --profile minimal
```

### Debug with Graphviz

```bash
sunbeam up --graph > up.dot
dot -Tpng up.dot -o up.png
```

### Run in Serial Mode

For tiny single-node clusters where concurrent applies overload the API server:

```bash
sunbeam up --serial
```

### Watch Progress

In another terminal:

```bash
sunbeam workflow list
sunbeam workflow status <id>
```

---

## Troubleshooting

### "OpenBao init failed after 5 attempts"

Check the pod:

```bash
kubectl logs -n data deployment/openbao
kubectl describe pod -n data -l app.kubernetes.io/name=openbao
```

Common causes: PVC not bound (Longhorn not ready), resource limits, or corrupted storage.

### "Timed out waiting for Lima VM"

```bash
limactl list sunbeam
limactl shell sunbeam -- sudo systemctl status k3s
```

### "Port-forward to OpenBao failed"

The pod may not be ready yet, or there may be a network partition. Try:

```bash
kubectl get pod -n data -l app.kubernetes.io/name=openbao
kubectl port-forward -n data pod/openbao-0 8200:8200
# In another terminal:
curl http://127.0.0.1:8200/v1/sys/seal-status
```

### "Cluster secret missing keys — restoring from local keystore"

This is a **warning**, not an error. The K8s secret was deleted but your local keystore had a backup. Everything should continue normally.

### Workflow Stuck

```bash
# List instances
sunbeam workflow list

# Check step status
sunbeam workflow status <instance-id>

# Cancel and retry
sunbeam workflow cancel <instance-id>
sunbeam up
```

### Reset Everything (Nuclear Option)

```bash
sunbeam down --yes --infra
rm -rf ~/.sunbeam/<context>/workflows.db
sunbeam up --use-lima
```

---

## CLI Reference

```
sunbeam up [OPTIONS]

Options:
      --set <SET>            Override a manifest field (kind/namespace/name/field/path=value)
      --disable <DISABLE>    Disable a resource or pattern (kind/namespace/name or glob)
      --enable <ENABLE>      Re-enable a resource or pattern
      --skip-cilium          Skip the Cilium CNI check
      --graph                Output a Graphviz DOT graph of the workflow and exit
      --use-lima             Use Lima VM for local k3s (shorthand for --profile lima)
      --profile <PROFILE>    Profile to load (from infra/profiles/<name>.yaml)
      --serial               Run in serial mode: longer delays between namespace applies
  -h, --help                 Print help
```

---

*Document version: matches `sunbeam up` workflow definition v3.*
