//! Up workflow definition — single unified cluster bring-up from k3s+Cilium
//! to first user login. Merges the old `seed` and `bootstrap` workflows.
//!
//! Version 3 changes:
//! - Monitoring moved from Phase 1 to Phase 11 (after identity is ready).
//! - Added WaitForRollout after ApplyManifest(ory) to block on DB migrations.
//! - Removed Gitea bootstrap (org creation, admin setup) — Gitea is no longer
//!   part of the core stack.
//! - Added SeedKratosAdminIdentity (was missing from v2 — no admin user
//!   existed after `sunbeam up`).
//! - Added Phase 10 rollout waits for apps that ApplyManifest doesn't block on.
//! - Bumped version 2 → 3.

use serde_json::json;
use wfe_core::builder::WorkflowBuilder;
use wfe_core::models::{ErrorBehavior, WorkflowDefinition};

use super::steps;
use crate::workflows::primitives::kv_service_configs;
use crate::workflows::primitives::{
    ApplyManifest, CollectCredentials, CreateK8sSecret, CreatePGDatabase, CreatePGRole,
    EnableVaultAuth, EnsureNamespace, EnsureOpenSearchML, InjectOpenSearchModelId, SeedKVPath,
    WaitForRollout, WriteKVPath, WriteVaultAuthConfig, WriteVaultPolicy, WriteVaultRole,
};
use crate::workflows::steps::postgres::pg_db_map;

/// Build the up workflow definition (version 3).
pub fn build() -> WorkflowDefinition {
    WorkflowBuilder::<serde_json::Value>::new()
        .default_error_behavior(ErrorBehavior::Terminate)
        // ── Phase 0: Lima VM (local dev only) ──────────────────────────
        .start_with::<steps::EnsureLimaVm>()
        .name("ensure-lima-vm")

        // ── Phase 1: Infrastructure ────────────────────────────────────
        .then::<steps::EnsureCilium>()
        .name("ensure-cilium")

        // Cert-manager MUST be applied and ready before any other namespace.
        // Every namespace filter includes cluster-scoped resources (ClusterRoles,
        // webhooks, etc.) and cert-manager's validating webhook rejects
        // ClusterIssuer / Certificate creates if its own webhook pod isn't ready.
        .then::<steps::EnsureTLSCert>()
        .name("ensure-tls-cert")
        .then::<steps::EnsureTLSSecret>()
        .name("ensure-tls-secret")
        .then::<ApplyManifest>()
        .name("apply-cert-manager")
        .config(json!({"namespace": "cert-manager", "skip": ["scaleway-dns-credentials", "vso-auth"]}))
        .then::<WaitForRollout>()
        .name("wait-cert-manager")
        .config(json!({"namespace": "cert-manager", "deployment": "cert-manager", "timeout_secs": 120}))
        .then::<steps::WaitForCertManagerWebhook>()
        .name("wait-cert-manager-webhook")

        // Longhorn must be applied and ready before data (PVCs need Longhorn
        // webhook validation) and before build (buildkitd may use Longhorn PVCs).
        .then::<ApplyManifest>()
        .name("apply-longhorn")
        .config(json!({"namespace": "longhorn-system"}))
        .then::<steps::WaitForLonghornWebhook>()
        .name("wait-longhorn-webhook")

        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-data",
                    Some(json!({"namespace": "data"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-build",
                    Some(json!({"namespace": "build"})));
            })
        )

        .then::<steps::WaitForCNPGWebhook>()
        .name("wait-cnpg-webhook")

        .then::<WaitForRollout>()
        .name("wait-buildkitd")
        .config(json!({"namespace": "build", "deployment": "buildkitd", "timeout_secs": 120}))

        // ── Phase 1b: Bootstrap critical images ───────────────────────
        // The proxy image is required by the ingress controller (pingora),
        // but image builds normally happen AFTER ingress is ready. On a
        // fresh install this creates a deadlock. Build the proxy image
        // here using host Docker and import it into k3s containerd so
        // pingora can start when its manifest is applied.
        .then::<steps::BootstrapCriticalImages>()
        .name("bootstrap-critical-images")

        // ── Phase 2: OpenBao init (sequential) ────────────────────────
        .then::<steps::FindOpenBaoPod>()
        .name("find-openbao-pod")
        .then::<steps::WaitPodRunning>()
        .name("wait-pod-running")
        .then::<steps::InitOrUnsealOpenBao>()
        .name("init-or-unseal-openbao")

        // ── Phase 3: KV seeding (parallel per-service) ────────────────
        .parallel(|p| {
            let mut p = p;
            for cfg in kv_service_configs::all_service_configs() {
                let service = cfg["service"].as_str().unwrap().to_string();
                p = p.branch(|b| {
                    let seed_id = b.add_step_typed::<SeedKVPath>(
                        &format!("seed-{service}"), Some(cfg.clone()));
                    let write_id = b.add_step_typed::<WriteKVPath>(
                        &format!("write-{service}"), Some(json!({"service": &service})));
                    b.wire_outcome(seed_id, write_id, None);
                });
            }
            // kratos-admin depends on seaweedfs (from_creds reference)
            p.branch(|b| {
                let seed_id = b.add_step_typed::<SeedKVPath>(
                    "seed-kratos-admin", Some(kv_service_configs::kratos_admin_config()));
                let write_id = b.add_step_typed::<WriteKVPath>(
                    "write-kratos-admin", Some(json!({"service": "kratos-admin"})));
                b.wire_outcome(seed_id, write_id, None);
            })
        })
        .then::<CollectCredentials>()
        .name("collect-credentials")
        // ── Phase 3b: Vault auth (4 atomic steps) ──────────────────
        .then::<EnableVaultAuth>()
        .name("enable-k8s-auth")
        .config(json!({"mount": "kubernetes", "type": "kubernetes"}))
        .then::<WriteVaultAuthConfig>()
        .name("write-k8s-auth-config")
        .config(json!({"mount": "kubernetes", "config": {
            "kubernetes_host": "https://kubernetes.default.svc.cluster.local"
        }}))
        .then::<WriteVaultPolicy>()
        .name("write-vso-policy")
        .config(json!({"name": "vso-reader", "hcl": concat!(
            "path \"secret/data/*\" { capabilities = [\"read\"] }\n",
            "path \"secret/metadata/*\" { capabilities = [\"read\", \"list\"] }\n",
            "path \"database/static-creds/*\" { capabilities = [\"read\"] }\n",
        )}))
        .then::<WriteVaultRole>()
        .name("write-vso-role")
        .config(json!({"mount": "kubernetes", "role": "vso", "config": {
            "bound_service_account_names": "default,headscale,wfe-server",
            "bound_service_account_namespaces": "ory,devtools,storage,stalwart,matrix,media,data,monitoring,cert-manager,vpn,wfe,press,oci",
            "policies": "vso-reader",
            "ttl": "1h"
        }}))

        // ── Phase 4: PostgreSQL ───────────────────────────────────────
        .then::<steps::WaitForPostgres>()
        .name("wait-for-postgres")

        .parallel(|p| {
            let db_map = pg_db_map();
            let mut p = p;
            for (user, db) in &db_map {
                p = p.branch(|b| {
                    let role_id = b.add_step_typed::<CreatePGRole>(
                        &format!("pg-role-{user}"),
                        Some(json!({"username": user})),
                    );
                    let db_id = b.add_step_typed::<CreatePGDatabase>(
                        &format!("pg-db-{db}"),
                        Some(json!({"dbname": db, "owner": user})),
                    );
                    b.wire_outcome(role_id, db_id, None);
                });
            }
            p
        })

        .then::<steps::ConfigureDatabaseEngine>()
        .name("configure-database-engine")

        // ── Phase 5: Ensure namespaces + create secrets BEFORE applying
        // manifests. Helm charts (Ory, etc.) reference these secrets at
        // pod creation time — if secrets don't exist, pods enter
        // CreateContainerConfigError and never recover.
        .parallel(|p| p
            .branch(|b| {
                let ns = b.add_step_typed::<EnsureNamespace>("ensure-ns-ory",
                    Some(json!({"namespace": "ory"})));
                let s1 = b.add_step_typed::<CreateK8sSecret>("secret-hydra",
                    Some(json!({"namespace":"ory","name":"hydra","data":{
                        "secretsSystem":"hydra-system-secret",
                        "secretsCookie":"hydra-cookie-secret",
                        "pairwise-salt":"hydra-pairwise-salt"
                    }})));
                let s2 = b.add_step_typed::<CreateK8sSecret>("secret-kratos-app",
                    Some(json!({"namespace":"ory","name":"kratos-app-secrets","data":{
                        "secretsDefault":"kratos-secrets-default",
                        "secretsCookie":"kratos-secrets-cookie"
                    }})));
                b.wire_outcome(ns, s1, None);
                b.wire_outcome(s1, s2, None);
            })
            .branch(|b| {
                let ns = b.add_step_typed::<EnsureNamespace>("ensure-ns-storage",
                    Some(json!({"namespace": "storage"})));
                let s1 = b.add_step_typed::<CreateK8sSecret>("secret-seaweedfs-s3-creds",
                    Some(json!({"namespace":"storage","name":"seaweedfs-s3-credentials","data":{
                        "S3_ACCESS_KEY":"s3-access-key",
                        "S3_SECRET_KEY":"s3-secret-key"
                    }})));
                let s2 = b.add_step_typed::<CreateK8sSecret>("secret-seaweedfs-s3-json",
                    Some(json!({"namespace":"storage","name":"seaweedfs-s3-json","data":{
                        "s3.json":"s3_json"
                    }})));
                b.wire_outcome(ns, s1, None);
                b.wire_outcome(s1, s2, None);
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-ingress",
                    Some(json!({"namespace": "ingress"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-devtools",
                    Some(json!({"namespace": "devtools"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-media",
                    Some(json!({"namespace": "media"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-stalwart",
                    Some(json!({"namespace": "stalwart"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-vpn",
                    Some(json!({"namespace": "vpn"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-oci",
                    Some(json!({"namespace": "oci"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-vso",
                    Some(json!({"namespace": "vault-secrets-operator"})));
            })
        )

        // ── Phase 6: Platform manifests ───────────────────────────────
        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-vso",
                    Some(json!({"namespace": "vault-secrets-operator"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-ingress",
                    Some(json!({"namespace": "ingress"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-ory",
                    Some(json!({"namespace": "ory", "serial_delay_secs": 30})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-devtools",
                    Some(json!({"namespace": "devtools"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-storage",
                    Some(json!({"namespace": "storage"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-media",
                    Some(json!({"namespace": "media"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-stalwart",
                    Some(json!({"namespace": "stalwart"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-vpn",
                    Some(json!({"namespace": "vpn"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-oci",
                    Some(json!({"namespace": "oci"})));
            })
        )

        // Wait for ingress controller before anything that needs external
        // connectivity (Gitea on 443, image pushes to src.* / oci.*).
        .then::<WaitForRollout>()
        .name("wait-ingress")
        .config(json!({"namespace": "ingress", "deployment": "pingora", "timeout_secs": 300}))

        // Re-apply data namespace after VSO is installed so that VaultAuth and
        // VaultStaticSecret resources (e.g. postgres-superuser-creds) that were
        // skipped during the first apply (VSO CRDs didn't exist yet) are created.
        .then::<ApplyManifest>()
        .name("reapply-data")
        .config(json!({"namespace": "data"}))

        // CRITICAL: Ory Helm charts run DB migrations as hooks. ApplyManifest
        // does NOT wait for hooks. Block here until Hydra and Kratos are
        // actually ready before anything that depends on them continues.
        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-ory-hydra",
                    Some(json!({"namespace": "ory", "deployment": "hydra", "timeout_secs": 300})));
            })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-ory-kratos",
                    Some(json!({"namespace": "ory", "deployment": "kratos", "timeout_secs": 300})));
            })
        )

        // SeaweedFS buckets must exist before zot starts (zot uses S3 backend).
        .then::<steps::EnsureSeaweedFSBuckets>()
        .name("ensure-seaweedfs-buckets")

        // Zot (OCI registry) must be ready before we build images that push
        // to oci.*.
        .then::<WaitForRollout>()
        .name("wait-zot-early")
        .config(json!({"namespace": "oci", "deployment": "zot", "timeout_secs": 120}))

        // Build and push project images via manifest ecosystem.
        // Sequential builds keep peak disk usage bounded for fresh installs.
        .then::<steps::BuildProjectImages>()
        .name("build-project-images")

        // ── Phase 7: Application manifests ────────────────────────────
        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-matrix",
                    Some(json!({"namespace": "matrix"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-wfe",
                    Some(json!({"namespace": "wfe"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-press",
                    Some(json!({"namespace": "press"})));
            })
        )

        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-matrix",
                    Some(json!({"namespace": "matrix"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-wfe",
                    Some(json!({"namespace": "wfe"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-press",
                    Some(json!({"namespace": "press"})));
            })
        )

        // ── Phase 8: Core rollouts + OpenSearch ML (parallel) ─────────
        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-valkey",
                    Some(json!({"namespace": "data", "deployment": "valkey", "timeout_secs": 120})));
            })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-kratos",
                    Some(json!({"namespace": "ory", "deployment": "kratos", "timeout_secs": 120})));
            })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-hydra",
                    Some(json!({"namespace": "ory", "deployment": "hydra", "timeout_secs": 120})));
            })
            .branch(|b| {
                // OpenSearch ML model download/deploy — can take 10+ min on first run.
                // Runs alongside rollout waits so it doesn't block the pipeline.
                b.add_step_typed::<EnsureOpenSearchML>("ensure-opensearch-ml", None);
            })
        )

        .then::<InjectOpenSearchModelId>()
        .name("inject-opensearch-model-id")

        // ── Phase 9: Kratos admin identity  (NEW in v3) ───────────────
        // This was missing from v2. Without it, no admin user exists and
        // nobody can log in.
        .then::<steps::SeedKratosAdminIdentity>()
        .name("seed-kratos-admin-identity")

        // ── Phase 10: Application rollouts ────────────────────────────
        // ApplyManifest does not wait for pods to be ready. These waits
        // ensure downstream steps (monitoring, VPN) don't race.
        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-tuwunel",
                    Some(json!({"namespace": "matrix", "deployment": "tuwunel", "timeout_secs": 300})));
            })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-wfe-server",
                    Some(json!({"namespace": "wfe", "deployment": "wfe-server", "timeout_secs": 300})));
            })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-zot",
                    Some(json!({"namespace": "oci", "deployment": "zot", "timeout_secs": 120})));
            })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-press",
                    Some(json!({"namespace": "press", "deployment": "press", "timeout_secs": 300})));
            })
            // sol is parked at 0 replicas (proto build issue) — skip wait.
            // .branch(|b| {
            //     b.add_step_typed::<WaitForRollout>("wait-sol",
            //         Some(json!({"namespace": "matrix", "deployment": "sol", "timeout_secs": 300})));
            // })
            .branch(|b| {
                b.add_step_typed::<WaitForRollout>("wait-kratos-admin",
                    Some(json!({"namespace": "ory", "deployment": "kratos-admin-ui", "timeout_secs": 300})));
            })
        )

        // ── Phase 11: Observability  (MOVED from Phase 1) ─────────────
        // Monitoring MUST come after identity (Hydra) and VSO secrets are
        // ready. Grafana uses Hydra for OIDC and needs OpenBao secrets.
        .then::<ApplyManifest>()
        .name("apply-monitoring")
        .config(json!({"namespace": "monitoring"}))

        .then::<WaitForRollout>()
        .name("wait-grafana")
        .config(json!({"namespace": "monitoring", "deployment": "kube-prometheus-stack-grafana", "timeout_secs": 300}))

        // ── Phase 12: Finalize ────────────────────────────────────────
        .then::<steps::MintVpnPreAuthKeys>()
        .name("mint-vpn-preauth-keys")

        .then::<steps::PrintURLs>()
        .name("print-urls")
        .end_workflow()
        .build("up", 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_returns_valid_definition() {
        let def = build();
        assert_eq!(def.id, "up");
        assert_eq!(def.version, 3);
        assert!(
            def.steps.len() > 20,
            "expected >20 steps, got {}",
            def.steps.len()
        );
    }

    #[test]
    fn test_first_step_is_ensure_lima_vm() {
        let def = build();
        assert_eq!(def.steps[0].name, Some("ensure-lima-vm".into()));
        assert!(def.steps[0].step_type.contains("EnsureLimaVm"));
    }

    #[test]
    fn test_second_step_is_ensure_cilium() {
        let def = build();
        assert_eq!(def.steps[1].name, Some("ensure-cilium".into()));
        assert!(def.steps[1].step_type.contains("EnsureCilium"));
    }

    #[test]
    fn test_last_step_is_print_urls() {
        let def = build();
        let last = def.steps.last().unwrap();
        assert_eq!(last.name, Some("print-urls".into()));
        assert!(last.step_type.contains("PrintURLs"));
    }

    #[test]
    fn test_apply_manifest_steps_have_config() {
        let def = build();
        let apply_steps: Vec<_> = def
            .steps
            .iter()
            .filter(|s| s.step_type.contains("ApplyManifest"))
            .collect();
        assert!(!apply_steps.is_empty(), "should have ApplyManifest steps");
        for s in &apply_steps {
            let config = s
                .step_config
                .as_ref()
                .unwrap_or_else(|| panic!("ApplyManifest step {:?} missing config", s.name));
            assert!(
                config.get("namespace").is_some(),
                "ApplyManifest step {:?} missing namespace in config",
                s.name
            );
        }
    }

    #[test]
    fn test_wait_for_rollout_steps_have_config() {
        let def = build();
        let rollout_steps: Vec<_> = def
            .steps
            .iter()
            .filter(|s| s.step_type.contains("WaitForRollout"))
            .collect();
        assert_eq!(
            rollout_steps.len(),
            15,
            "should have 15 WaitForRollout steps (cert-manager, ory-hydra, ory-kratos, buildkitd, ingress, valkey, kratos, hydra, tuwunel, wfe-server, zot, zot-early, press, kratos-admin, grafana)"
        );
        for s in &rollout_steps {
            let config = s.step_config.as_ref().unwrap();
            assert!(config.get("namespace").is_some());
            assert!(config.get("deployment").is_some());
        }
    }

    #[test]
    fn test_has_parallel_containers() {
        let def = build();
        let seq_steps: Vec<_> = def
            .steps
            .iter()
            .filter(|s| s.step_type.contains("SequenceStep"))
            .collect();
        assert!(
            seq_steps.len() >= 4,
            "expected >=4 parallel blocks, got {}",
            seq_steps.len()
        );
        for s in &seq_steps {
            assert!(
                !s.children.is_empty(),
                "parallel container should have children"
            );
        }
    }

    #[test]
    fn test_non_container_steps_have_names() {
        let def = build();
        for s in &def.steps {
            // SequenceStep containers are auto-generated by .parallel()
            if s.step_type.contains("SequenceStep") {
                continue;
            }
            assert!(
                s.name.is_some(),
                "step {} ({}) has no name",
                s.id,
                s.step_type
            );
        }
    }

    #[test]
    fn test_cert_branch_has_chained_outcomes() {
        let def = build();
        let tls_cert = def
            .steps
            .iter()
            .find(|s| s.name.as_deref() == Some("ensure-tls-cert"))
            .expect("should have ensure-tls-cert step");
        assert!(
            !tls_cert.outcomes.is_empty(),
            "ensure-tls-cert should wire to ensure-tls-secret"
        );

        let tls_secret = def
            .steps
            .iter()
            .find(|s| s.name.as_deref() == Some("ensure-tls-secret"))
            .expect("should have ensure-tls-secret step");
        assert!(
            !tls_secret.outcomes.is_empty(),
            "ensure-tls-secret should wire to apply-cert-manager"
        );
    }

    #[test]
    fn test_seed_kratos_admin_identity_present() {
        let def = build();
        let step = def
            .steps
            .iter()
            .find(|s| s.name.as_deref() == Some("seed-kratos-admin-identity"))
            .expect("should have seed-kratos-admin-identity step");
        assert!(step.step_type.contains("SeedKratosAdminIdentity"));
    }

    #[test]
    fn test_monitoring_applied_late() {
        let def = build();
        let monitoring_idx = def
            .steps
            .iter()
            .position(|s| s.name.as_deref() == Some("apply-monitoring"))
            .expect("should have apply-monitoring step");
        let ory_idx = def
            .steps
            .iter()
            .position(|s| s.name.as_deref() == Some("apply-ory"))
            .expect("should have apply-ory step");
        assert!(
            monitoring_idx > ory_idx,
            "monitoring should be applied after ory"
        );
    }

    #[test]
    fn test_no_bootstrap_gitea_step() {
        let def = build();
        let found = def
            .steps
            .iter()
            .any(|s| s.name.as_deref() == Some("bootstrap-gitea"));
        assert!(!found, "bootstrap-gitea should not exist in v3");
    }

    #[test]
    fn test_no_sync_gitea_password_step() {
        let def = build();
        let found = def
            .steps
            .iter()
            .any(|s| s.name.as_deref() == Some("sync-gitea-admin-password"));
        assert!(!found, "sync-gitea-admin-password should not exist in v3");
    }

    #[test]
    fn test_default_error_behavior_is_terminate() {
        let def = build();
        assert_eq!(
            def.default_error_behavior,
            wfe_core::models::ErrorBehavior::Terminate,
            "up workflow should terminate immediately on any step failure"
        );
    }
}
