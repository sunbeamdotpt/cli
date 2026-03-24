//! CLI command definitions and dispatch for OpenBao (Vault).

use std::collections::HashMap;

use clap::Subcommand;

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::output::{self, OutputFormat};

// ═══════════════════════════════════════════════════════════════════════════
// Command tree
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum VaultCommand {
    /// Show seal status.
    Status,
    /// Initialize the vault.
    Init {
        /// Number of key shares.
        #[arg(long, default_value = "5")]
        key_shares: u32,
        /// Key threshold for unseal.
        #[arg(long, default_value = "3")]
        key_threshold: u32,
    },
    /// Unseal the vault with a key share.
    Unseal {
        /// Unseal key share.
        #[arg(short, long)]
        key: String,
    },
    /// KV secrets engine operations.
    Kv {
        #[command(subcommand)]
        action: KvAction,
    },
    /// Write a policy from HCL.
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    /// Auth method management.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Secrets engine management.
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },
    /// Read from an arbitrary API path.
    Read {
        /// API path (e.g. "auth/token/lookup-self").
        #[arg(short, long)]
        path: String,
    },
    /// Write to an arbitrary API path.
    Write {
        /// API path.
        #[arg(short, long)]
        path: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Re-initialize the vault (destructive — wipes all secrets).
    Reinit,
    /// Show local keystore status.
    Keys,
    /// Export vault keys as plaintext (for machine migration).
    ExportKeys,
}

#[derive(Subcommand, Debug)]
pub enum KvAction {
    /// Get a secret.
    Get {
        /// Secret path.
        #[arg(short, long)]
        path: String,
        /// Secrets engine mount point.
        #[arg(long, default_value = "secret")]
        mount: String,
    },
    /// Put (create/overwrite) a secret.
    Put {
        /// Secret path.
        #[arg(short, long)]
        path: String,
        /// Secrets engine mount point.
        #[arg(long, default_value = "secret")]
        mount: String,
        /// JSON object or key=value pairs.
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Patch (merge) fields into a secret.
    Patch {
        /// Secret path.
        #[arg(short, long)]
        path: String,
        /// Secrets engine mount point.
        #[arg(long, default_value = "secret")]
        mount: String,
        /// JSON object or key=value pairs.
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a secret.
    Delete {
        /// Secret path.
        #[arg(short, long)]
        path: String,
        /// Secrets engine mount point.
        #[arg(long, default_value = "secret")]
        mount: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PolicyAction {
    /// Write a policy from HCL.
    Write {
        /// Policy name.
        #[arg(short, long)]
        name: String,
        /// HCL policy body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Enable an auth method.
    Enable {
        /// Mount path (e.g. "kubernetes").
        #[arg(short, long)]
        path: String,
        /// Auth method type (e.g. "kubernetes").
        #[arg(short = 't', long = "type")]
        method_type: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SecretsAction {
    /// Enable a secrets engine.
    Enable {
        /// Mount path (e.g. "database").
        #[arg(short, long)]
        path: String,
        /// Engine type (e.g. "database", "kv").
        #[arg(short = 't', long = "type")]
        engine_type: String,
    },
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Parse a `--data` value as either a JSON object `{"k":"v"}` or as
/// `key=value` pairs (one per line or comma-separated), returning a
/// `HashMap<String, String>`.
fn parse_kv_data(raw: &str) -> Result<HashMap<String, String>> {
    // Try JSON first
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(obj) = val.as_object() {
            let map: HashMap<String, String> = obj
                .iter()
                .map(|(k, v)| {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), s)
                })
                .collect();
            return Ok(map);
        }
    }

    // Fallback: key=value pairs separated by newlines or commas
    let mut map = HashMap::new();
    for token in raw.split(|c| c == '\n' || c == ',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((k, v)) = token.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        } else {
            return Err(crate::error::SunbeamError::Other(format!(
                "invalid key=value pair: {token}"
            )));
        }
    }
    Ok(map)
}

/// Read kv data from `--data` flag or stdin.
fn read_kv_input(flag: Option<&str>) -> Result<HashMap<String, String>> {
    let raw = match flag {
        Some("-") | None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            buf
        }
        Some(v) => v.to_string(),
    };
    parse_kv_data(&raw)
}

/// Read raw text from `--data` flag or stdin (for policy HCL).
fn read_text_input(flag: Option<&str>) -> Result<String> {
    match flag {
        Some("-") | None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            Ok(buf)
        }
        Some(v) => Ok(v.to_string()),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Dispatch
// ═══════════════════════════════════════════════════════════════════════════

pub async fn dispatch(
    cmd: VaultCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    // -- Commands that don't need a BaoClient -------------------------------
    match cmd {
        VaultCommand::Keys => {
            let domain = crate::config::domain();
            let path = crate::vault_keystore::keystore_path(domain);

            if !crate::vault_keystore::keystore_exists(domain) {
                output::warn(&format!("No local keystore found at {}", path.display()));
                output::warn("Run `sunbeam seed` to create one, or `sunbeam vault reinit` to start fresh.");
                return Ok(());
            }

            match crate::vault_keystore::verify_vault_keys(domain) {
                Ok(ks) => {
                    output::ok(&format!("Domain:     {}", ks.domain));
                    output::ok(&format!("Created:    {}", ks.created_at.format("%Y-%m-%d %H:%M:%S UTC")));
                    output::ok(&format!("Updated:    {}", ks.updated_at.format("%Y-%m-%d %H:%M:%S UTC")));
                    output::ok(&format!("Shares:     {}/{}", ks.key_threshold, ks.key_shares));
                    output::ok(&format!(
                        "Token:      {}...{}",
                        &ks.root_token[..8.min(ks.root_token.len())],
                        &ks.root_token[ks.root_token.len().saturating_sub(4)..]
                    ));
                    output::ok(&format!("Unseal keys: {}", ks.unseal_keys_b64.len()));
                    output::ok(&format!("Path:       {}", path.display()));
                }
                Err(e) => {
                    output::warn(&format!("Keystore at {} is invalid: {e}", path.display()));
                }
            }
            return Ok(());
        }

        VaultCommand::ExportKeys => {
            let domain = crate::config::domain();
            output::warn("WARNING: This prints vault root token and unseal keys in PLAINTEXT.");
            output::warn("Only use this for machine migration. Do not share or log this output.");
            eprint!("  Type 'export' to confirm: ");
            let mut answer = String::new();
            std::io::stdin()
                .read_line(&mut answer)
                .map_err(|e| crate::error::SunbeamError::Other(format!("stdin: {e}")))?;
            if answer.trim() != "export" {
                output::ok("Aborted.");
                return Ok(());
            }
            let json = crate::vault_keystore::export_plaintext(domain)?;
            println!("{json}");
            return Ok(());
        }

        VaultCommand::Reinit => {
            return dispatch_reinit().await;
        }

        // All other commands need a BaoClient — fall through.
        _ => {}
    }

    let bao = client.bao().await?;
    match cmd {
        // -- Status ---------------------------------------------------------
        VaultCommand::Status => {
            let status = bao.seal_status().await?;
            output::render(&status, fmt)?;
            // Show local keystore status
            let domain = crate::config::domain();
            if crate::vault_keystore::keystore_exists(domain) {
                match crate::vault_keystore::load_keystore(domain) {
                    Ok(ks) => {
                        output::ok(&format!(
                            "Local keystore: valid (updated {})",
                            ks.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
                        ));
                    }
                    Err(e) => {
                        output::warn(&format!("Local keystore: corrupt ({e})"));
                    }
                }
            } else {
                output::warn("Local keystore: not found");
            }
            Ok(())
        }

        // -- Init -----------------------------------------------------------
        VaultCommand::Init {
            key_shares,
            key_threshold,
        } => {
            let resp = bao.init(key_shares, key_threshold).await?;
            output::render(&resp, fmt)
        }

        // -- Unseal ---------------------------------------------------------
        VaultCommand::Unseal { key } => {
            let resp = bao.unseal(&key).await?;
            output::render(&resp, fmt)
        }

        // -- KV operations --------------------------------------------------
        VaultCommand::Kv { action } => match action {
            KvAction::Get { path, mount } => {
                let data = bao.kv_get(&mount, &path).await?;
                match data {
                    Some(map) => output::render(&map, fmt),
                    None => {
                        output::ok(&format!("No secret found at {mount}/data/{path}"));
                        Ok(())
                    }
                }
            }
            KvAction::Put { path, mount, data } => {
                let map = read_kv_input(data.as_deref())?;
                bao.kv_put(&mount, &path, &map).await?;
                output::ok(&format!("Written to {mount}/data/{path}"));
                Ok(())
            }
            KvAction::Patch { path, mount, data } => {
                let map = read_kv_input(data.as_deref())?;
                bao.kv_patch(&mount, &path, &map).await?;
                output::ok(&format!("Patched {mount}/data/{path}"));
                Ok(())
            }
            KvAction::Delete { path, mount } => {
                bao.kv_delete(&mount, &path).await?;
                output::ok(&format!("Deleted {mount}/data/{path}"));
                Ok(())
            }
        },

        // -- Policy ---------------------------------------------------------
        VaultCommand::Policy { action } => match action {
            PolicyAction::Write { name, data } => {
                let hcl = read_text_input(data.as_deref())?;
                bao.write_policy(&name, &hcl).await?;
                output::ok(&format!("Written policy {name}"));
                Ok(())
            }
        },

        // -- Auth -----------------------------------------------------------
        VaultCommand::Auth { action } => match action {
            AuthAction::Enable { path, method_type } => {
                bao.auth_enable(&path, &method_type).await?;
                output::ok(&format!("Enabled auth method {method_type} at {path}"));
                Ok(())
            }
        },

        // -- Secrets engine -------------------------------------------------
        VaultCommand::Secrets { action } => match action {
            SecretsAction::Enable { path, engine_type } => {
                bao.enable_secrets_engine(&path, &engine_type).await?;
                output::ok(&format!("Enabled secrets engine {engine_type} at {path}"));
                Ok(())
            }
        },

        // -- Read -----------------------------------------------------------
        VaultCommand::Read { path } => {
            let data = bao.read(&path).await?;
            match data {
                Some(val) => output::render(&val, fmt),
                None => {
                    output::ok(&format!("No data at {path}"));
                    Ok(())
                }
            }
        }

        // -- Write ----------------------------------------------------------
        VaultCommand::Write { path, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let resp = bao.write(&path, &json).await?;
            if resp.is_null() {
                output::ok(&format!("Written to {path}"));
                Ok(())
            } else {
                output::render(&resp, fmt)
            }
        }

        // Already handled above; unreachable.
        VaultCommand::Keys | VaultCommand::ExportKeys | VaultCommand::Reinit => unreachable!(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Reinit
// ═══════════════════════════════════════════════════════════════════════════

/// Run a kubectl command, returning Ok(()) on success.
async fn kubectl(args: &[&str]) -> Result<()> {
    crate::kube::ensure_tunnel().await?;
    let ctx = format!("--context={}", crate::kube::context());
    let status = tokio::process::Command::new("kubectl")
        .arg(&ctx)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| crate::error::SunbeamError::Other(format!("kubectl: {e}")))?;
    if !status.success() {
        return Err(crate::error::SunbeamError::Other(format!(
            "kubectl {} exited with {}",
            args.join(" "),
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Port-forward guard — cancels the background forwarder on drop.
struct PortForwardGuard {
    _abort_handle: tokio::task::AbortHandle,
    pub local_port: u16,
}

impl Drop for PortForwardGuard {
    fn drop(&mut self) {
        self._abort_handle.abort();
    }
}

/// Open a kube-rs port-forward to `pod_name` in `namespace` on `remote_port`.
async fn port_forward(namespace: &str, pod_name: &str, remote_port: u16) -> Result<PortForwardGuard> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, ListParams};
    use tokio::net::TcpListener;

    let client = crate::kube::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| crate::error::SunbeamError::Other(format!("bind: {e}")))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| crate::error::SunbeamError::Other(format!("local_addr: {e}")))?
        .port();

    let pod_name = pod_name.to_string();
    let ns = namespace.to_string();
    let task = tokio::spawn(async move {
        let mut current_pod = pod_name;
        loop {
            let (mut client_stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };

            let pf_result = pods.portforward(&current_pod, &[remote_port]).await;
            let mut pf = match pf_result {
                Ok(pf) => pf,
                Err(e) => {
                    tracing::warn!("Port-forward failed, re-resolving pod: {e}");
                    if let Ok(new_client) = crate::kube::get_client().await {
                        let new_pods: Api<Pod> = Api::namespaced(new_client.clone(), &ns);
                        let lp = ListParams::default();
                        if let Ok(pod_list) = new_pods.list(&lp).await {
                            if let Some(name) = pod_list
                                .items
                                .iter()
                                .find(|p| {
                                    p.metadata
                                        .name
                                        .as_deref()
                                        .map(|n| n.starts_with(current_pod.split('-').next().unwrap_or("")))
                                        .unwrap_or(false)
                                })
                                .and_then(|p| p.metadata.name.clone())
                            {
                                current_pod = name;
                            }
                        }
                    }
                    continue;
                }
            };

            let mut upstream = match pf.take_stream(remote_port) {
                Some(s) => s,
                None => continue,
            };

            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut upstream).await;
            });
        }
    });

    let abort_handle = task.abort_handle();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok(PortForwardGuard {
        _abort_handle: abort_handle,
        local_port,
    })
}

/// Destructive vault re-initialization workflow.
async fn dispatch_reinit() -> Result<()> {
    output::warn("This will DESTROY all vault secrets. You must re-run `sunbeam seed` after.");
    eprint!("  Type 'reinit' to confirm: ");
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| crate::error::SunbeamError::Other(format!("stdin: {e}")))?;
    if answer.trim() != "reinit" {
        output::ok("Aborted.");
        return Ok(());
    }

    output::step("Re-initializing vault...");

    // Delete PVC and pod
    output::ok("Deleting vault storage...");
    let _ = kubectl(&["-n", "data", "delete", "pvc", "data-openbao-0", "--ignore-not-found"]).await;
    let _ = kubectl(&["-n", "data", "delete", "pod", "openbao-0", "--ignore-not-found"]).await;

    // Wait for pod to come back
    output::ok("Waiting for vault pod to restart...");
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    let _ = kubectl(&[
        "-n", "data", "wait", "--for=condition=Ready", "pod/openbao-0",
        "--timeout=120s",
    ])
    .await;

    // Port-forward and init
    let pf = port_forward("data", "openbao-0", 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);
    let fresh_bao = crate::openbao::BaoClient::new(&bao_url);

    let init = fresh_bao.init(1, 1).await?;
    let unseal_key = init.unseal_keys_b64[0].clone();
    let root_token = init.root_token.clone();

    // Save to local keystore
    let domain = crate::config::domain();
    let ks = crate::vault_keystore::VaultKeystore {
        version: 1,
        domain: domain.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        root_token: root_token.clone(),
        unseal_keys_b64: vec![unseal_key.clone()],
        key_shares: 1,
        key_threshold: 1,
    };
    crate::vault_keystore::save_keystore(&ks)?;
    output::ok(&format!(
        "Keys saved to local keystore at {}",
        crate::vault_keystore::keystore_path(domain).display()
    ));

    // Save to K8s Secret
    let mut data = HashMap::new();
    data.insert("key".to_string(), unseal_key.clone());
    data.insert("root-token".to_string(), root_token.clone());
    crate::kube::create_secret("data", "openbao-keys", data).await?;
    output::ok("Keys stored in K8s Secret openbao-keys.");

    // Unseal
    fresh_bao.unseal(&unseal_key).await?;
    output::ok("Vault unsealed.");

    output::step("Vault re-initialized. Run `sunbeam seed` now to restore all secrets.");
    Ok(())
}
