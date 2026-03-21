//! OpenBao database secrets engine configuration.

use std::collections::HashMap;

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};

use crate::error::{Result, ResultExt};
use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::ok;

use super::{rand_token, PG_USERS};

/// Enable OpenBao database secrets engine and create PostgreSQL static roles.
pub async fn configure_db_engine(bao: &BaoClient) -> Result<()> {
    ok("Configuring OpenBao database secrets engine...");
    let pg_rw = "postgres-rw.data.svc.cluster.local:5432";

    let _ = bao.enable_secrets_engine("database", "database").await;

    // ── vault PG user setup ─────────────────────────────────────────────
    let client = k::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), "data");
    let lp = ListParams::default().labels("cnpg.io/cluster=postgres,role=primary");
    let pod_list = pods.list(&lp).await?;
    let cnpg_pod = pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())
        .ctx("Could not find CNPG primary pod for vault user setup.")?
        .to_string();

    let existing_vault_pass = bao.kv_get_field("secret", "vault", "pg-password").await?;
    let vault_pg_pass = if existing_vault_pass.is_empty() {
        let new_pass = rand_token();
        let mut vault_data = HashMap::new();
        vault_data.insert("pg-password".to_string(), new_pass.clone());
        bao.kv_put("secret", "vault", &vault_data).await?;
        ok("vault KV entry written.");
        new_pass
    } else {
        ok("vault KV entry already present -- skipping write.");
        existing_vault_pass
    };

    let create_vault_sql = concat!(
        "DO $$ BEGIN ",
        "IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'vault') THEN ",
        "CREATE USER vault WITH LOGIN CREATEROLE; ",
        "END IF; ",
        "END $$;"
    );

    psql_exec(&cnpg_pod, create_vault_sql).await?;
    psql_exec(
        &cnpg_pod,
        &format!("ALTER USER vault WITH PASSWORD '{vault_pg_pass}';"),
    )
    .await?;

    for user in PG_USERS {
        psql_exec(
            &cnpg_pod,
            &format!("GRANT {user} TO vault WITH ADMIN OPTION;"),
        )
        .await?;
    }
    ok("vault PG user configured with ADMIN OPTION on all service roles.");

    let conn_url = format!(
        "postgresql://{{{{username}}}}:{{{{password}}}}@{pg_rw}/postgres?sslmode=disable"
    );

    bao.write_db_config(
        "cnpg-postgres",
        "postgresql-database-plugin",
        &conn_url,
        "vault",
        &vault_pg_pass,
        "*",
    )
    .await?;
    ok("DB engine connection configured (vault user).");

    let rotation_stmt = r#"ALTER USER "{{name}}" WITH PASSWORD '{{password}}';"#;

    for user in PG_USERS {
        bao.write_db_static_role(user, "cnpg-postgres", user, 86400, &[rotation_stmt])
            .await?;
        ok(&format!("  static-role/{user}"));
    }

    ok("Database secrets engine configured.");
    Ok(())
}

/// Execute a psql command on the CNPG primary pod.
async fn psql_exec(cnpg_pod: &str, sql: &str) -> Result<(i32, String)> {
    k::kube_exec(
        "data",
        cnpg_pod,
        &["psql", "-U", "postgres", "-c", sql],
        Some("postgres"),
    )
    .await
}
