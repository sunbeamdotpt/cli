//! Seed workflow definition — linear sequence of all seed steps.

use serde_json::json;
use wfe_core::builder::WorkflowBuilder;
use wfe_core::models::WorkflowDefinition;

use super::steps;
use crate::workflows::primitives::kv_service_configs;
use crate::workflows::primitives::{
    CollectCredentials, CreateK8sSecret, CreatePGDatabase, CreatePGRole, EnableVaultAuth,
    EnsureNamespace, SeedKVPath, WriteKVPath, WriteVaultAuthConfig, WriteVaultPolicy,
    WriteVaultRole,
};
use steps::postgres::pg_db_map;

/// Build the seed workflow definition.
pub fn build() -> WorkflowDefinition {
    WorkflowBuilder::<serde_json::Value>::new()
        .start_with::<steps::FindOpenBaoPod>()
        .name("find-openbao-pod")
        .then::<steps::WaitPodRunning>()
        .name("wait-pod-running")
        .then::<steps::InitOrUnsealOpenBao>()
        .name("init-or-unseal-openbao")
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
            "bound_service_account_names": "default",
            "bound_service_account_namespaces": "ory,devtools,storage,stalwart,matrix,media,data,monitoring,cert-manager,vpn,wfe",
            "policies": "vso-reader",
            "ttl": "1h"
        }}))
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
        .then::<steps::SeedKratosAdminIdentity>()
        .name("seed-kratos-admin-identity")
        .then::<steps::PrintSeedOutputs>()
        .name("print-seed-outputs")
        .end_workflow()
        .build("seed", 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_returns_valid_definition() {
        let def = build();
        assert_eq!(def.id, "seed");
        assert_eq!(def.version, 2);
        // More steps now due to parallel PG branches
        assert!(
            def.steps.len() > 13,
            "expected >13 steps, got {}",
            def.steps.len()
        );
    }

    #[test]
    fn test_has_pg_role_and_db_steps() {
        let def = build();
        let role_steps: Vec<_> = def
            .steps
            .iter()
            .filter(|s| s.step_type.contains("CreatePGRole"))
            .collect();
        let db_steps: Vec<_> = def
            .steps
            .iter()
            .filter(|s| s.step_type.contains("CreatePGDatabase"))
            .collect();
        assert_eq!(role_steps.len(), 8, "should have 8 CreatePGRole steps");
        assert_eq!(db_steps.len(), 8, "should have 8 CreatePGDatabase steps");
    }

    #[test]
    fn test_pg_steps_have_config() {
        let def = build();
        for s in &def.steps {
            if s.step_type.contains("CreatePGRole") {
                let config = s.step_config.as_ref().expect("CreatePGRole missing config");
                assert!(config.get("username").is_some());
            }
            if s.step_type.contains("CreatePGDatabase") {
                let config = s
                    .step_config
                    .as_ref()
                    .expect("CreatePGDatabase missing config");
                assert!(config.get("dbname").is_some());
                assert!(config.get("owner").is_some());
            }
        }
    }

    #[test]
    fn test_first_and_last_steps() {
        let def = build();
        assert_eq!(def.steps[0].name, Some("find-openbao-pod".into()));
        let last = def.steps.last().unwrap();
        assert_eq!(last.name, Some("print-seed-outputs".into()));
    }
}
