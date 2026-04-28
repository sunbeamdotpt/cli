//! Up workflow definition — phased deployment with parallel branches.

use serde_json::json;
use wfe_core::builder::WorkflowBuilder;
use wfe_core::models::WorkflowDefinition;

use super::steps;
use crate::workflows::primitives::kv_service_configs;
use crate::workflows::primitives::{
    ApplyManifest, CollectCredentials, CreateK8sSecret, CreatePGDatabase, CreatePGRole,
    EnableVaultAuth, EnsureNamespace, EnsureOpenSearchML, InjectOpenSearchModelId, SeedKVPath,
    WaitForRollout, WriteKVPath, WriteVaultAuthConfig, WriteVaultPolicy, WriteVaultRole,
};
use crate::workflows::seed::steps::postgres::pg_db_map;

/// Build the up workflow definition.
pub fn build() -> WorkflowDefinition {
    WorkflowBuilder::<serde_json::Value>::new()
        // ── Phase 1: Infrastructure ────────────────────────────────────
        .start_with::<steps::EnsureCilium>()
        .name("ensure-cilium")

        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-longhorn",
                    Some(json!({"namespace": "longhorn-system"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-monitoring",
                    Some(json!({"namespace": "monitoring"})));
            })
        )

        .then::<ApplyManifest>()
        .name("apply-data")
        .config(json!({"namespace": "data"}))

        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<steps::EnsureBuildKit>("ensure-buildkit", None);
            })
            .branch(|b| {
                let id0 = b.add_step_typed::<steps::EnsureTLSCert>("ensure-tls-cert", None);
                let id1 = b.add_step_typed::<steps::EnsureTLSSecret>("ensure-tls-secret", None);
                let id2 = b.add_step_typed::<ApplyManifest>("apply-cert-manager",
                    Some(json!({"namespace": "cert-manager"})));
                b.wire_outcome(id0, id1, None);
                b.wire_outcome(id1, id2, None);
            })
        )

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
            "bound_service_account_names": "default,gitserv",
            "bound_service_account_namespaces": "ory,devtools,storage,stalwart,matrix,media,data,monitoring,cert-manager,vpn,wfe,gitserv,oci",
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

        // ── Phase 5: Platform manifests ───────────────────────────────
        .then::<ApplyManifest>()
        .name("apply-vso")
        .config(json!({"namespace": "vault-secrets-operator"}))

        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-ingress",
                    Some(json!({"namespace": "ingress"})));
            })
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-ory",
                    Some(json!({"namespace": "ory"})));
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
                // VPN bootstrap path — headscale needs cert-manager + the
                // headscale_db postgres role from Phase 4. Both are done by
                // the time this branch fires.
                b.add_step_typed::<ApplyManifest>("apply-vpn",
                    Some(json!({"namespace": "vpn"})));
            })
        )

        // ── Phase 6: K8s secrets (parallel by namespace) ──────────────
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
                let ns = b.add_step_typed::<EnsureNamespace>("ensure-ns-devtools",
                    Some(json!({"namespace": "devtools"})));
                let s1 = b.add_step_typed::<CreateK8sSecret>("secret-gitea-s3",
                    Some(json!({"namespace":"devtools","name":"gitea-s3-credentials","data":{
                        "access-key":"s3-access-key",
                        "secret-key":"s3-secret-key"
                    }})));
                let s2 = b.add_step_typed::<CreateK8sSecret>("secret-gitea-admin",
                    Some(json!({"namespace":"devtools","name":"gitea-admin-credentials","data":{
                        "username":"literal:gitea_admin",
                        "password":"gitea-admin-password"
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
                b.add_step_typed::<EnsureNamespace>("ensure-ns-matrix",
                    Some(json!({"namespace": "matrix"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-media",
                    Some(json!({"namespace": "media"})));
            })
            .branch(|b| {
                b.add_step_typed::<EnsureNamespace>("ensure-ns-monitoring",
                    Some(json!({"namespace": "monitoring"})));
            })
        )

        .then::<steps::SyncGiteaAdminPassword>()
        .name("sync-gitea-admin-password")
        .then::<steps::BootstrapGitea>()
        .name("bootstrap-gitea")

        // ── Phase 7: Application manifests ────────────────────────────
        .parallel(|p| p
            .branch(|b| {
                b.add_step_typed::<ApplyManifest>("apply-matrix",
                    Some(json!({"namespace": "matrix"})));
            })
            .branch(|b| {
                // wfe-server pulls its image from src.DOMAIN_SUFFIX (built
                // locally), so it has to come *after* gitea bootstrap.
                b.add_step_typed::<ApplyManifest>("apply-wfe",
                    Some(json!({"namespace": "wfe"})));
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

        // ── Phase 9: VPN pre-auth keys (mint or skip) ─────────────────
        // Runs after headscale has been applied (phase 5) and the ACL
        // ConfigMap has been loaded. Idempotent: no-op if both the
        // router Secret and the user config already have a key.
        .then::<steps::MintVpnPreAuthKeys>()
        .name("mint-vpn-preauth-keys")

        .then::<steps::PrintURLs>()
        .name("print-urls")
        .end_workflow()
        .build("up", 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_returns_valid_definition() {
        let def = build();
        assert_eq!(def.id, "up");
        assert_eq!(def.version, 2);
        assert!(
            def.steps.len() > 20,
            "expected >20 steps, got {}",
            def.steps.len()
        );
    }

    #[test]
    fn test_first_step_is_ensure_cilium() {
        let def = build();
        assert_eq!(def.steps[0].name, Some("ensure-cilium".into()));
        assert!(def.steps[0].step_type.contains("EnsureCilium"));
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
        assert_eq!(rollout_steps.len(), 3, "should have 3 WaitForRollout steps");
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
}
