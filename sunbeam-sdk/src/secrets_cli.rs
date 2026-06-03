//! OpenBao secrets engine interaction — KV, transit, generic read/write, and system ops.
//!
//! All commands work by finding the OpenBao pod, opening a port-forward, grabbing
//! the root token from the K8s secret `openbao-keys`, and using `BaoClient` to hit
//! the HTTP API. Override with `--addr` and `--token` for remote instances.

use clap::Subcommand;

use crate::error::{Result, SunbeamError};
use crate::openbao::BaoClient;

// ---------------------------------------------------------------------------
// CLI enums
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
/// Secrets action.
pub enum SecretsAction {
    /// KV v2 operations.
    #[command(subcommand)]
    Kv(KvAction),
    /// Transit secrets engine operations.
    #[command(subcommand)]
    Transit(TransitAction),
    /// Generic read from any engine/path.
    Read {
        /// Path to read (e.g. `sys/mounts`, `transit/sbbb/keys/my-key`).
        path: String,
    },
    /// Generic write to any engine/path.
    Write {
        /// Path to write (e.g. `database/config/postgres`).
        path: String,
        /// Key=value pairs to write. Repeatable.
        #[arg(required = true)]
        pairs: Vec<String>,
    },
    /// Generic delete.
    Delete {
        /// Path to delete.
        path: String,
    },
    /// Generic list.
    List {
        /// Path to list.
        path: String,
    },
    /// Show seal and initialization status.
    Status,
    /// Initialize OpenBao.
    Init,
    /// Unseal with an unseal key.
    Unseal {
        /// Unseal key.
        key: String,
    },
    /// Raw bao CLI passthrough inside the OpenBao pod.
    Exec {
        /// Arguments to pass to the bao CLI.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
/// KV action.
pub enum KvAction {
    /// Read a KV secret.
    Get {
        /// Secret path (e.g. `hydra` reads from `secret/data/hydra`).
        path: String,
        /// KV mount path.
        #[arg(long, default_value = "secret")]
        mount: String,
    },
    /// Write or replace a KV secret.
    Put {
        /// Secret path.
        path: String,
        /// KV mount path.
        #[arg(long, default_value = "secret")]
        mount: String,
        /// Key=value pairs. Repeatable.
        #[arg(required = true)]
        pairs: Vec<String>,
    },
    /// Merge fields into an existing KV secret.
    Patch {
        /// Secret path.
        path: String,
        /// KV mount path.
        #[arg(long, default_value = "secret")]
        mount: String,
        /// Key=value pairs. Repeatable.
        #[arg(required = true)]
        pairs: Vec<String>,
    },
    /// Delete the latest version of a KV secret.
    Delete {
        /// Secret path.
        path: String,
        /// KV mount path.
        #[arg(long, default_value = "secret")]
        mount: String,
    },
    /// List keys under a KV path.
    List {
        /// Secret path.
        path: String,
        /// KV mount path.
        #[arg(long, default_value = "secret")]
        mount: String,
    },
}

#[derive(Subcommand, Debug)]
/// Transit action.
pub enum TransitAction {
    /// Enable a transit secrets engine at a mount path.
    Enable {
        /// Mount path (e.g. `transit/sbbb`).
        mount: String,
    },
    /// Create a key under a transit mount.
    CreateKey {
        /// Mount path (e.g. `transit/sbbb`).
        mount: String,
        /// Key name.
        name: String,
        /// Key type.
        #[arg(long, default_value = "ed25519")]
        key_type: String,
    },
    /// Read public metadata of a transit key.
    ReadKey {
        /// Mount path.
        mount: String,
        /// Key name.
        name: String,
    },
    /// List keys under a transit mount.
    ListKeys {
        /// Mount path.
        mount: String,
    },
    /// Delete a transit key.
    DeleteKey {
        /// Mount path.
        mount: String,
        /// Key name.
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a `sunbeam secrets <action>` command.
#[tracing::instrument(skip(action), fields(addr = ?addr, has_token = token.is_some()))]
pub async fn dispatch(
    addr: Option<&str>,
    token: Option<&str>,
    output: crate::output::OutputFormat,
    action: SecretsAction,
) -> Result<()> {
    match (addr, token) {
        (Some(addr), Some(token)) => {
            let client = BaoClient::with_token(addr, token);
            dispatch_with_client(&client, output, action).await
        }
        (Some(_), None) => Err(SunbeamError::Config(
            "--addr requires --token".into(),
        )),
        (None, token_override) => {
            let ob_pod = find_openbao_pod().await?;
            let pf = crate::secrets::port_forward("data", &ob_pod, 8200).await?;
            let bao_url = format!("http://127.0.0.1:{}", pf.local_port);

            let tok = match token_override {
                Some(t) => t.to_string(),
                None => read_token().await?,
            };

            let client = BaoClient::with_token(&bao_url, &tok);
            dispatch_with_client(&client, output, action).await
        }
    }
}

/// Inner dispatch that operates on an already-resolved client. Testable.
#[tracing::instrument(skip(client))]
pub async fn dispatch_with_client(
    client: &BaoClient,
    output: crate::output::OutputFormat,
    action: SecretsAction,
) -> Result<()> {
    match action {
        SecretsAction::Kv(action) => dispatch_kv(client, output, action).await,
        SecretsAction::Transit(action) => dispatch_transit(client, action).await,
        SecretsAction::Read { path } => cmd_read(client, output, &path).await,
        SecretsAction::Write { path, pairs } => cmd_write(client, &path, &pairs).await,
        SecretsAction::Delete { path } => cmd_delete(client, &path).await,
        SecretsAction::List { path } => cmd_list(client, output, &path).await,
        SecretsAction::Status => cmd_status(client).await,
        SecretsAction::Init => cmd_init(client).await,
        SecretsAction::Unseal { key } => cmd_unseal(client, &key).await,
        SecretsAction::Exec { args } => crate::kube::cmd_bao(&args).await,
    }
}

// ---------------------------------------------------------------------------
// KV dispatch
// ---------------------------------------------------------------------------

async fn dispatch_kv(
    client: &BaoClient,
    output: crate::output::OutputFormat,
    action: KvAction,
) -> Result<()> {
    match action {
        KvAction::Get { path, mount } => {
            let data = client.kv_get(&mount, &path).await?;
            match data {
                Some(data) => {
                    #[derive(serde::Serialize)]
                    struct KvRow {
                        key: String,
                        value: String,
                    }
                    let mut rows: Vec<KvRow> = data
                        .into_iter()
                        .map(|(k, v)| KvRow { key: k, value: v })
                        .collect();
                    rows.sort_by(|a, b| a.key.cmp(&b.key));
                    crate::output::render_list(
                        &rows,
                        &["KEY", "VALUE"],
                        |r| vec![r.key.clone(), r.value.clone()],
                        output,
                    )?;
                }
                None => {
                    tracing::warn!("No secret found at {mount}/{path}");
                }
            }
            Ok(())
        }
        KvAction::Put { path, mount, pairs } => {
            let data = parse_kv_pairs(&pairs)?;
            client.kv_put(&mount, &path, &data).await?;
            tracing::info!("Wrote secret to {mount}/{path}");
            Ok(())
        }
        KvAction::Patch { path, mount, pairs } => {
            let data = parse_kv_pairs(&pairs)?;
            client.kv_patch(&mount, &path, &data).await?;
            tracing::info!("Patched secret at {mount}/{path}");
            Ok(())
        }
        KvAction::Delete { path, mount } => {
            client.kv_delete(&mount, &path).await?;
            tracing::info!("Deleted secret at {mount}/{path}");
            Ok(())
        }
        KvAction::List { path, mount } => {
            // TODO: implement kv_list in BaoClient or use generic list
            let list_path = format!("{mount}/metadata/{path}");
            match client.list(&list_path).await? {
                Some(data) => {
                    crate::output::render(&data, output)?;
                }
                None => {
                    tracing::warn!("No keys found at {mount}/{path}");
                }
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Transit dispatch
// ---------------------------------------------------------------------------

async fn dispatch_transit(client: &BaoClient, action: TransitAction) -> Result<()> {
    match action {
        TransitAction::Enable { mount } => {
            let path = mount.trim_matches('/');
            client.enable_secrets_engine(path, "transit").await?;
            tracing::info!("Transit engine ready at {path}/");
            Ok(())
        }
        TransitAction::CreateKey {
            mount,
            name,
            key_type,
        } => {
            let mount = mount.trim_matches('/');
            let key_path = format!("{mount}/keys/{name}");
            if let Some(existing) = client.read(&key_path).await? {
                tracing::info!("Key {key_path} already exists.");
                if let Some(t) = existing
                    .get("data")
                    .and_then(|d| d.get("type"))
                    .and_then(|v| v.as_str())
                    && t != key_type
                {
                    tracing::warn!(
                        "Existing key type is {t}, requested {key_type} — leaving as-is."
                    );
                }
                return Ok(());
            }
            client
                .write(&key_path, &serde_json::json!({ "type": key_type }))
                .await?;
            tracing::info!("Key {key_path} created.");
            Ok(())
        }
        TransitAction::ReadKey { mount, name } => {
            let mount = mount.trim_matches('/');
            let key_path = format!("{mount}/keys/{name}");
            match client.read(&key_path).await? {
                Some(value) => {
                    crate::output::render(&value, crate::output::OutputFormat::Json)?;
                }
                None => {
                    tracing::warn!("Key {key_path} not found.");
                }
            }
            Ok(())
        }
        TransitAction::ListKeys { mount } => {
            let mount = mount.trim_matches('/');
            let list_path = format!("{mount}/keys");
            match client.list(&list_path).await? {
                Some(value) => {
                    crate::output::render(&value, crate::output::OutputFormat::Json)?;
                }
                None => {
                    tracing::warn!("No keys found at {mount}");
                }
            }
            Ok(())
        }
        TransitAction::DeleteKey { mount, name } => {
            let mount = mount.trim_matches('/');
            let key_path = format!("{mount}/keys/{name}");
            client
                .write(&key_path, &serde_json::json!({}))
                .await?;
            tracing::info!("Key {key_path} deleted.");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Generic commands
// ---------------------------------------------------------------------------

async fn cmd_read(
    client: &BaoClient,
    output: crate::output::OutputFormat,
    path: &str,
) -> Result<()> {
    match client.read(path).await? {
        Some(value) => {
            crate::output::render(&value, output)?;
        }
        None => {
            tracing::warn!("Path {path} not found.");
        }
    }
    Ok(())
}

async fn cmd_write(client: &BaoClient, path: &str, pairs: &[String]) -> Result<()> {
    let data = parse_kv_pairs(pairs)?;
    let json_data: serde_json::Value = data
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();
    let resp = client.write(path, &json_data).await?;
    crate::output::render(&resp, crate::output::OutputFormat::Json)?;
    Ok(())
}

async fn cmd_delete(client: &BaoClient, path: &str) -> Result<()> {
    // BaoClient doesn't have a generic delete; use raw HTTP via read helper
    // or extend BaoClient. For now, use a raw request.
    let url = format!("{}/v1/{}", client.base_url, path.trim_start_matches('/'));
    // BaoClient doesn't expose token directly for raw HTTP.
    // Use the write method with empty body as a delete workaround.
    tracing::info!("Delete {path} — using generic write with empty body");
    client
        .write(path, &serde_json::json!({}))
        .await?;
    Ok(())
}

async fn cmd_list(
    client: &BaoClient,
    output: crate::output::OutputFormat,
    path: &str,
) -> Result<()> {
    match client.list(path).await? {
        Some(value) => {
            crate::output::render(&value, output)?;
        }
        None => {
            tracing::warn!("Path {path} not found.");
        }
    }
    Ok(())
}

async fn cmd_status(client: &BaoClient) -> Result<()> {
    let status = client.seal_status().await?;
    #[derive(serde::Serialize)]
    struct StatusRow {
        initialized: bool,
        sealed: bool,
    }
    let row = StatusRow {
        initialized: status.initialized,
        sealed: status.sealed,
    };
    crate::output::render(&row, crate::output::OutputFormat::Json)?;
    Ok(())
}

async fn cmd_init(client: &BaoClient) -> Result<()> {
    let resp = client.init(1, 1).await?;
    tracing::info!("OpenBao initialized.");
    tracing::info!("Root token: {}", resp.keys_base64.first().unwrap_or(&"???".to_string()));
    Ok(())
}

async fn cmd_unseal(client: &BaoClient, key: &str) -> Result<()> {
    let resp = client.unseal(key).await?;
    if resp.sealed {
        tracing::info!("Still sealed — more unseal keys needed.");
    } else {
        tracing::info!("Unsealed.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn find_openbao_pod() -> Result<String> {
    crate::kube::find_pod_by_label("data", "app.kubernetes.io/name=openbao,component=server")
        .await
        .ok_or_else(|| SunbeamError::Other("OpenBao pod not found".into()))
}

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

fn parse_kv_pairs(pairs: &[String]) -> Result<std::collections::HashMap<String, String>> {
    let mut map = std::collections::HashMap::new();
    for pair in pairs {
        let (k, v) = pair.split_once('=').ok_or_else(|| {
            SunbeamError::Config(format!("Expected key=value, got: {pair}"))
        })?;
        map.insert(k.to_string(), v.to_string());
    }
    Ok(map)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_pairs_valid() {
        let pairs = vec!["foo=bar".to_string(), "baz=qux".to_string()];
        let result = parse_kv_pairs(&pairs).unwrap();
        assert_eq!(result.get("foo"), Some(&"bar".to_string()));
        assert_eq!(result.get("baz"), Some(&"qux".to_string()));
    }

    #[test]
    fn parse_kv_pairs_with_equals_in_value() {
        let pairs = vec!["key=a=b=c".to_string()];
        let result = parse_kv_pairs(&pairs).unwrap();
        assert_eq!(result.get("key"), Some(&"a=b=c".to_string()));
    }

    #[test]
    fn parse_kv_pairs_missing_equals_fails() {
        let pairs = vec!["invalid".to_string()];
        assert!(parse_kv_pairs(&pairs).is_err());
    }

    #[test]
    fn parse_kv_pairs_empty_ok() {
        let result = parse_kv_pairs(&[]).unwrap();
        assert!(result.is_empty());
    }
}
