//! `sunbeam vcs signing-key …` — OpenPGP signing-key management.
//!
//! Maps to the `SigningKeyService` gRPC surface:
//!   list       → ListSigningKeys
//!   upload     → UploadUserKey
//!   revoke     → RevokeKey
//!   bundle     → GetKeyDirectory
//!   cross-sign → SubmitCrossCertification
//!   rotate     → ListSigningKeys + RevokeKey(each active) + optionally UploadUserKey

use crate::error::{Result, SunbeamError};
use crate::output::{OutputFormat, render, render_list};
use crate::vcs::client::{connect_signing_key_client, map_status, resolve_token};
use gitserv_proto::pb::{
    GetKeyDirectoryRequest, ListSigningKeysRequest, RevokeKeyRequest, SubmitCrossCertificationRequest,
    UploadUserKeyRequest,
};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct SigningKeyArgs {
    #[command(subcommand)]
    pub command: SigningKeyCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum SigningKeyCmd {
    /// List all signing keys (active + revoked) for a user.
    List {
        /// User ID to list keys for. Defaults to the authenticated user.
        #[arg(long)]
        user: Option<String>,
    },
    /// Upload an OpenPGP certificate from a file (or stdin if path is `-`).
    Upload {
        /// Path to the armored `.asc` certificate file, or `-` to read from stdin.
        path: String,
        /// User ID to upload the cert for. Defaults to the authenticated user.
        #[arg(long)]
        user: Option<String>,
    },
    /// Revoke a signing key by fingerprint.
    Revoke {
        /// Full fingerprint of the cert to revoke.
        fingerprint: String,
        /// User ID that owns the cert. Defaults to the authenticated user.
        #[arg(long)]
        user: Option<String>,
    },
    /// Fetch the published key bundle for a user (armored .asc).
    ///
    /// Writes to stdout by default; use `--out` to save to a file.
    Bundle {
        /// User ID to fetch the bundle for. Defaults to the authenticated user.
        #[arg(long)]
        user: Option<String>,
        /// Write the bundle to this file instead of stdout.
        #[arg(long, short = 'o')]
        out: Option<String>,
    },
    /// Submit a user-issued cross-certification packet for a key.
    ///
    /// The certification packet (`--signature`) must be a raw OpenPGP
    /// signature packet in binary form (not armored).
    #[command(name = "cross-sign")]
    CrossSign {
        /// Fingerprint of the subject cert being cross-signed.
        #[arg(long)]
        subject: String,
        /// Path to the binary OpenPGP signature packet file.
        #[arg(long)]
        signature: String,
        /// User ID that owns the subject cert. Defaults to the authenticated user.
        #[arg(long)]
        user: Option<String>,
    },
    /// Convenience: revoke all active keys and optionally upload a new one.
    ///
    /// Steps:
    ///   1. ListSigningKeys for the user.
    ///   2. RevokeKey for each active key.
    ///   3. UploadUserKey from `--new` if supplied.
    ///   4. Print the new active key set.
    Rotate {
        /// Path to the new armored certificate to upload after revoking. Optional.
        #[arg(long)]
        new: Option<String>,
        /// User ID to rotate keys for. Defaults to the authenticated user.
        #[arg(long)]
        user: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Row types for rendering
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct SigningKeyRow {
    fingerprint: String,
    primary_key_fingerprint: String,
    kind: String,
    active: bool,
    created_at: String,
    revoked_at: String,
}

impl SigningKeyRow {
    fn from_pb(e: gitserv_proto::pb::SigningKeyEntry) -> Self {
        Self {
            fingerprint: e.fingerprint,
            primary_key_fingerprint: e.primary_key_fingerprint,
            kind: e.kind,
            active: e.active,
            created_at: e.created_at,
            revoked_at: e.revoked_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct UploadRow {
    fingerprint: String,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(args: SigningKeyArgs, endpoint: &str, format: OutputFormat) -> Result<()> {
    let domain = crate::config::domain().to_string();
    let token = resolve_token(&domain).await?;
    let mut client = connect_signing_key_client(endpoint, &token).await?;

    match args.command {
        SigningKeyCmd::List { user } => {
            let user_id = resolve_user_id(user)?;
            let resp = client
                .list_signing_keys(ListSigningKeysRequest {
                    user_id: user_id.clone(),
                })
                .await
                .map_err(map_status)?
                .into_inner();
            let rows: Vec<SigningKeyRow> =
                resp.keys.into_iter().map(SigningKeyRow::from_pb).collect();
            render_list(
                &rows,
                &["FINGERPRINT", "KIND", "ACTIVE", "CREATED_AT", "REVOKED_AT"],
                |r| {
                    vec![
                        r.fingerprint.clone(),
                        r.kind.clone(),
                        r.active.to_string(),
                        r.created_at.clone(),
                        r.revoked_at.clone(),
                    ]
                },
                format,
            )
        }

        SigningKeyCmd::Upload { path, user } => {
            let user_id = resolve_user_id(user)?;
            let armored_cert = read_path_or_stdin(&path)?;
            let resp = client
                .upload_user_key(UploadUserKeyRequest {
                    user_id,
                    armored_cert,
                })
                .await
                .map_err(map_status)?
                .into_inner();
            render(&UploadRow { fingerprint: resp.fingerprint }, format)
        }

        SigningKeyCmd::Revoke { fingerprint, user } => {
            let user_id = resolve_user_id(user)?;
            client
                .revoke_key(RevokeKeyRequest {
                    user_id,
                    fingerprint,
                })
                .await
                .map_err(map_status)?;
            Ok(())
        }

        SigningKeyCmd::Bundle { user, out } => {
            let user_id = resolve_user_id(user)?;
            let resp = client
                .get_key_directory(GetKeyDirectoryRequest { user_id })
                .await
                .map_err(map_status)?
                .into_inner();
            match out {
                Some(path) => {
                    std::fs::write(&path, resp.armored_bundle.as_bytes()).map_err(|e| {
                        SunbeamError::Other(format!("write {path}: {e}"))
                    })?;
                }
                None => print!("{}", resp.armored_bundle),
            }
            Ok(())
        }

        SigningKeyCmd::CrossSign { subject, signature, user } => {
            let user_id = resolve_user_id(user)?;
            let packet = std::fs::read(&signature)
                .map_err(|e| SunbeamError::Other(format!("read {signature}: {e}")))?;
            client
                .submit_cross_certification(SubmitCrossCertificationRequest {
                    user_id,
                    subject_fingerprint: subject,
                    certification_packet: packet,
                })
                .await
                .map_err(map_status)?;
            Ok(())
        }

        SigningKeyCmd::Rotate { new, user } => {
            let user_id = resolve_user_id(user)?;

            // 1. List current keys.
            let list_resp = client
                .list_signing_keys(ListSigningKeysRequest {
                    user_id: user_id.clone(),
                })
                .await
                .map_err(map_status)?
                .into_inner();

            // 2. Revoke each active key.
            let active_keys: Vec<_> = list_resp.keys.into_iter().filter(|k| k.active).collect();
            for key in &active_keys {
                client
                    .revoke_key(RevokeKeyRequest {
                        user_id: user_id.clone(),
                        fingerprint: key.fingerprint.clone(),
                    })
                    .await
                    .map_err(map_status)?;
                println!("revoked: {}", key.fingerprint);
            }

            // 3. Optionally upload new cert.
            if let Some(path) = new {
                let armored_cert = read_path_or_stdin(&path)?;
                let up_resp = client
                    .upload_user_key(UploadUserKeyRequest {
                        user_id: user_id.clone(),
                        armored_cert,
                    })
                    .await
                    .map_err(map_status)?
                    .into_inner();
                println!("uploaded: {}", up_resp.fingerprint);
            }

            // 4. Print new active set.
            let final_resp = client
                .list_signing_keys(ListSigningKeysRequest {
                    user_id: user_id.clone(),
                })
                .await
                .map_err(map_status)?
                .into_inner();
            let rows: Vec<SigningKeyRow> = final_resp
                .keys
                .into_iter()
                .filter(|k| k.active)
                .map(SigningKeyRow::from_pb)
                .collect();
            render_list(
                &rows,
                &["FINGERPRINT", "KIND", "CREATED_AT"],
                |r| vec![r.fingerprint.clone(), r.kind.clone(), r.created_at.clone()],
                format,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the effective user ID: use the supplied value, or fall back to
/// the authenticated identity stored in the CLI auth cache.
fn resolve_user_id(user: Option<String>) -> Result<String> {
    match user {
        Some(id) if !id.is_empty() => Ok(id),
        _ => {
            // Read the cached user ID from the auth token store.
            // auth::whoami() returns the `sub` claim from the cached token.
            crate::auth::whoami()
        }
    }
}

/// Read file contents (or stdin when path is `"-"`), trim trailing whitespace.
fn read_path_or_stdin(path: &str) -> Result<String> {
    if path == "-" {
        use std::io::Read as _;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| SunbeamError::Other(format!("stdin: {e}")))?;
        Ok(buf.trim_end().to_owned())
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| SunbeamError::Other(format!("read {path}: {e}")))
            .map(|s| s.trim_end().to_owned())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(clap::Parser)]
    struct Cmd {
        #[command(subcommand)]
        sub: SigningKeyCmd,
    }

    fn parse(args: &[&str]) -> SigningKeyCmd {
        Cmd::try_parse_from(args).unwrap().sub
    }

    #[test]
    fn list_no_user() {
        match parse(&["cmd", "list"]) {
            SigningKeyCmd::List { user } => assert!(user.is_none()),
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn list_with_user() {
        match parse(&["cmd", "list", "--user", "01K000000000000000000000A"]) {
            SigningKeyCmd::List { user } => {
                assert_eq!(user.as_deref(), Some("01K000000000000000000000A"))
            }
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn upload_path() {
        match parse(&["cmd", "upload", "/tmp/key.asc"]) {
            SigningKeyCmd::Upload { path, user } => {
                assert_eq!(path, "/tmp/key.asc");
                assert!(user.is_none());
            }
            _ => panic!("expected Upload"),
        }
    }

    #[test]
    fn upload_stdin() {
        match parse(&["cmd", "upload", "-"]) {
            SigningKeyCmd::Upload { path, .. } => assert_eq!(path, "-"),
            _ => panic!("expected Upload stdin"),
        }
    }

    #[test]
    fn revoke_fingerprint() {
        match parse(&["cmd", "revoke", "DEADBEEF"]) {
            SigningKeyCmd::Revoke { fingerprint, user } => {
                assert_eq!(fingerprint, "DEADBEEF");
                assert!(user.is_none());
            }
            _ => panic!("expected Revoke"),
        }
    }

    #[test]
    fn bundle_to_file() {
        match parse(&["cmd", "bundle", "--out", "/tmp/bundle.asc"]) {
            SigningKeyCmd::Bundle { out, user } => {
                assert_eq!(out.as_deref(), Some("/tmp/bundle.asc"));
                assert!(user.is_none());
            }
            _ => panic!("expected Bundle"),
        }
    }

    #[test]
    fn bundle_stdout() {
        match parse(&["cmd", "bundle"]) {
            SigningKeyCmd::Bundle { out, .. } => assert!(out.is_none()),
            _ => panic!("expected Bundle"),
        }
    }

    #[test]
    fn cross_sign_args() {
        match parse(&[
            "cmd",
            "cross-sign",
            "--subject",
            "AABB",
            "--signature",
            "/tmp/sig.bin",
        ]) {
            SigningKeyCmd::CrossSign { subject, signature, user } => {
                assert_eq!(subject, "AABB");
                assert_eq!(signature, "/tmp/sig.bin");
                assert!(user.is_none());
            }
            _ => panic!("expected CrossSign"),
        }
    }

    #[test]
    fn rotate_no_new() {
        match parse(&["cmd", "rotate"]) {
            SigningKeyCmd::Rotate { new, user } => {
                assert!(new.is_none());
                assert!(user.is_none());
            }
            _ => panic!("expected Rotate"),
        }
    }

    #[test]
    fn rotate_with_new() {
        match parse(&["cmd", "rotate", "--new", "/tmp/new.asc"]) {
            SigningKeyCmd::Rotate { new, .. } => {
                assert_eq!(new.as_deref(), Some("/tmp/new.asc"));
            }
            _ => panic!("expected Rotate"),
        }
    }

    #[test]
    fn signing_key_row_serializes() {
        let row = SigningKeyRow {
            fingerprint: "AABB".into(),
            primary_key_fingerprint: "CCDD".into(),
            kind: "user_provided".into(),
            active: true,
            created_at: "2026-04-01T00:00:00Z".into(),
            revoked_at: "".into(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"kind\":\"user_provided\""), "{json}");
        assert!(json.contains("\"active\":true"), "{json}");
    }
}
