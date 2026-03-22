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
    let bao = client.bao().await?;
    match cmd {
        // -- Status ---------------------------------------------------------
        VaultCommand::Status => {
            let status = bao.seal_status().await?;
            output::render(&status, fmt)
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
    }
}
