//! Encrypted local keystore for OpenBao vault keys.
//!
//! Stores root tokens and unseal keys locally, encrypted with AES-256-GCM.
//! Key derivation uses Argon2id with a machine-specific salt. This ensures
//! vault keys survive K8s Secret overwrites and are never lost.

use crate::error::{Result, SunbeamError};
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// AES-256-GCM nonce size.
const NONCE_LEN: usize = 12;
/// Argon2 salt size.
const SALT_LEN: usize = 16;
/// Machine salt size (stored in .machine-salt file).
const MACHINE_SALT_LEN: usize = 32;

/// Vault keys stored in the encrypted keystore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultKeystore {
    pub version: u32,
    pub domain: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub root_token: String,
    pub unseal_keys_b64: Vec<String>,
    pub key_shares: u32,
    pub key_threshold: u32,
}

/// Result of comparing local keystore with cluster state.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncStatus {
    /// Local and cluster keys match.
    InSync,
    /// Local keystore exists but cluster secret is missing/empty.
    ClusterMissing,
    /// Cluster secret exists but no local keystore.
    LocalMissing,
    /// Both exist but differ.
    Mismatch,
    /// Neither exists.
    NoKeys,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Legacy vault dir — used only for migration.
fn legacy_vault_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/share")
        })
        .join("sunbeam")
        .join("vault")
}

/// Base directory for vault keystore files: ~/.sunbeam/vault/
fn base_dir(override_dir: Option<&Path>) -> PathBuf {
    if let Some(d) = override_dir {
        return d.to_path_buf();
    }
    let new_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sunbeam")
        .join("vault");

    // Migration: copy files from legacy location if new dir doesn't exist yet
    if !new_dir.exists() {
        let legacy = legacy_vault_dir();
        if legacy.is_dir() {
            let _ = std::fs::create_dir_all(&new_dir);
            if let Ok(entries) = std::fs::read_dir(&legacy) {
                for entry in entries.flatten() {
                    let dest = new_dir.join(entry.file_name());
                    let _ = std::fs::copy(entry.path(), &dest);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            &dest,
                            std::fs::Permissions::from_mode(0o600),
                        );
                    }
                }
            }
        }
    }

    new_dir
}

/// Path to the encrypted keystore file for a domain.
pub fn keystore_path(domain: &str) -> PathBuf {
    keystore_path_in(domain, None)
}

fn keystore_path_in(domain: &str, override_dir: Option<&Path>) -> PathBuf {
    let dir = base_dir(override_dir);
    let safe = domain.replace(['/', '\\', ':'], "_");
    let name = if safe.is_empty() { "default" } else { &safe };
    dir.join(format!("{name}.enc"))
}

/// Whether a local keystore exists for this domain.
pub fn keystore_exists(domain: &str) -> bool {
    keystore_path(domain).exists()
}

/// Whether a keystore exists in a specific directory (context-aware).
pub fn keystore_exists_at(domain: &str, dir: &Path) -> bool {
    keystore_path_in(domain, Some(dir)).exists()
}

fn keystore_exists_in(domain: &str, dir: Option<&Path>) -> bool {
    keystore_path_in(domain, dir).exists()
}

// ---------------------------------------------------------------------------
// Machine salt
// ---------------------------------------------------------------------------

fn machine_salt_path(override_dir: Option<&Path>) -> PathBuf {
    if let Some(d) = override_dir {
        return d.join(".machine-salt");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sunbeam")
        .join(".machine-salt")
}

fn load_or_create_machine_salt(override_dir: Option<&Path>) -> Result<Vec<u8>> {
    let path = machine_salt_path(override_dir);
    if path.exists() {
        let data = std::fs::read(&path)
            .map_err(|e| SunbeamError::Other(format!("reading machine salt: {e}")))?;
        if data.len() == MACHINE_SALT_LEN {
            return Ok(data);
        }
        // Wrong length — regenerate
    }

    // Create parent directories
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SunbeamError::Other(format!("creating vault dir: {e}")))?;
    }

    // Generate new salt
    let mut salt = vec![0u8; MACHINE_SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    std::fs::write(&path, &salt)
        .map_err(|e| SunbeamError::Other(format!("writing machine salt: {e}")))?;

    // Set 0600 permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms)
            .map_err(|e| SunbeamError::Other(format!("setting salt permissions: {e}")))?;
    }

    Ok(salt)
}

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

fn derive_key(domain: &str, argon2_salt: &[u8], override_dir: Option<&Path>) -> Result<[u8; 32]> {
    let machine_salt = load_or_create_machine_salt(override_dir)?;
    // Combine machine salt + domain for input
    let mut input = machine_salt;
    input.extend_from_slice(b"sunbeam-vault-keystore:");
    input.extend_from_slice(domain.as_bytes());

    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(&input, argon2_salt, &mut key)
        .map_err(|e| SunbeamError::Other(format!("argon2 key derivation: {e}")))?;
    Ok(key)
}

// ---------------------------------------------------------------------------
// Encrypt / decrypt
// ---------------------------------------------------------------------------

fn encrypt(plaintext: &[u8], domain: &str, override_dir: Option<&Path>) -> Result<Vec<u8>> {
    // Generate random nonce and argon2 salt
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut argon2_salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    OsRng.fill_bytes(&mut argon2_salt);

    let key = derive_key(domain, &argon2_salt, override_dir)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| SunbeamError::Other(format!("AES init: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| SunbeamError::Other(format!("AES encrypt: {e}")))?;

    // Output: [nonce (12)][argon2_salt (16)][ciphertext+tag]
    let mut output = Vec::with_capacity(NONCE_LEN + SALT_LEN + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&argon2_salt);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

fn decrypt(data: &[u8], domain: &str, override_dir: Option<&Path>) -> Result<Vec<u8>> {
    let header_len = NONCE_LEN + SALT_LEN;
    if data.len() < header_len + 16 {
        // 16 bytes minimum for AES-GCM tag
        return Err(SunbeamError::Other(
            "vault keystore file is too short or corrupt".into(),
        ));
    }

    let nonce_bytes = &data[..NONCE_LEN];
    let argon2_salt = &data[NONCE_LEN..header_len];
    let ciphertext = &data[header_len..];

    let key = derive_key(domain, argon2_salt, override_dir)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| SunbeamError::Other(format!("AES init: {e}")))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| SunbeamError::Other("vault keystore decryption failed — file is corrupt or was encrypted on a different machine".into()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Save a keystore, encrypted, to the local filesystem (default dir).
pub fn save_keystore(ks: &VaultKeystore) -> Result<()> {
    save_keystore_in(ks, None)
}

/// Save a keystore to a specific directory (context-aware).
pub fn save_keystore_to(ks: &VaultKeystore, dir: &Path) -> Result<()> {
    save_keystore_in(ks, Some(dir))
}

fn save_keystore_in(ks: &VaultKeystore, override_dir: Option<&Path>) -> Result<()> {
    let path = keystore_path_in(&ks.domain, override_dir);

    // Create parent directories
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SunbeamError::Other(format!("creating vault dir: {e}")))?;
    }

    let plaintext = serde_json::to_vec_pretty(ks)?;
    let encrypted = encrypt(&plaintext, &ks.domain, override_dir)?;

    std::fs::write(&path, &encrypted)
        .map_err(|e| SunbeamError::Other(format!("writing keystore: {e}")))?;

    // Set 0600 permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms)
            .map_err(|e| SunbeamError::Other(format!("setting keystore permissions: {e}")))?;
    }

    Ok(())
}

/// Load and decrypt a keystore from the local filesystem (default dir).
pub fn load_keystore(domain: &str) -> Result<VaultKeystore> {
    load_keystore_in(domain, None)
}

/// Load a keystore from a specific directory (context-aware).
pub fn load_keystore_from(domain: &str, dir: &Path) -> Result<VaultKeystore> {
    load_keystore_in(domain, Some(dir))
}

fn load_keystore_in(domain: &str, override_dir: Option<&Path>) -> Result<VaultKeystore> {
    let path = keystore_path_in(domain, override_dir);
    if !path.exists() {
        return Err(SunbeamError::Other(format!(
            "no vault keystore found for domain '{domain}' at {}",
            path.display()
        )));
    }

    let data = std::fs::read(&path)
        .map_err(|e| SunbeamError::Other(format!("reading keystore: {e}")))?;

    if data.is_empty() {
        return Err(SunbeamError::Other("vault keystore file is empty".into()));
    }

    let plaintext = decrypt(&data, domain, override_dir)?;
    let ks: VaultKeystore = serde_json::from_slice(&plaintext)
        .map_err(|e| SunbeamError::Other(format!("parsing keystore JSON: {e}")))?;

    if ks.version > 1 {
        return Err(SunbeamError::Other(format!(
            "vault keystore version {} is not supported (max: 1)",
            ks.version
        )));
    }

    Ok(ks)
}

/// Load and validate a keystore — fails if any critical fields are empty.
pub fn verify_vault_keys(domain: &str) -> Result<VaultKeystore> {
    verify_vault_keys_in(domain, None)
}

/// Verify vault keys from a specific directory (context-aware).
pub fn verify_vault_keys_from(domain: &str, dir: &Path) -> Result<VaultKeystore> {
    verify_vault_keys_in(domain, Some(dir))
}

fn verify_vault_keys_in(domain: &str, override_dir: Option<&Path>) -> Result<VaultKeystore> {
    let ks = load_keystore_in(domain, override_dir)?;

    if ks.root_token.is_empty() {
        return Err(SunbeamError::Other(
            "vault keystore has empty root_token".into(),
        ));
    }
    if ks.unseal_keys_b64.is_empty() {
        return Err(SunbeamError::Other(
            "vault keystore has no unseal keys".into(),
        ));
    }
    if ks.key_shares == 0 {
        return Err(SunbeamError::Other(
            "vault keystore has key_shares=0".into(),
        ));
    }
    if ks.key_threshold == 0 || ks.key_threshold > ks.key_shares {
        return Err(SunbeamError::Other(format!(
            "vault keystore has invalid threshold={}/shares={}",
            ks.key_threshold, ks.key_shares
        )));
    }

    Ok(ks)
}

/// Export keystore as plaintext JSON (for machine migration).
pub fn export_plaintext(domain: &str) -> Result<String> {
    let ks = load_keystore(domain)?;
    serde_json::to_string_pretty(&ks)
        .map_err(|e| SunbeamError::Other(format!("serializing keystore: {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_keystore(domain: &str) -> VaultKeystore {
        VaultKeystore {
            version: 1,
            domain: domain.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            root_token: "hvs.test-root-token-abc123".to_string(),
            unseal_keys_b64: vec!["dGVzdC11bnNlYWwta2V5".to_string()],
            key_shares: 1,
            key_threshold: 1,
        }
    }

    // -- Encryption roundtrip ------------------------------------------------

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let loaded = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(loaded.root_token, ks.root_token);
        assert_eq!(loaded.unseal_keys_b64, ks.unseal_keys_b64);
        assert_eq!(loaded.domain, ks.domain);
        assert_eq!(loaded.key_shares, ks.key_shares);
        assert_eq!(loaded.key_threshold, ks.key_threshold);
    }

    #[test]
    fn test_encrypt_decrypt_large_token() {
        let dir = TempDir::new().unwrap();
        let mut ks = test_keystore("sunbeam.pt");
        ks.root_token = format!("hvs.{}", "a".repeat(200));
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let loaded = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(loaded.root_token, ks.root_token);
    }

    #[test]
    fn test_different_domains_different_ciphertext() {
        let dir = TempDir::new().unwrap();
        let ks_a = test_keystore("a.example.com");
        let ks_b = VaultKeystore {
            domain: "b.example.com".into(),
            ..test_keystore("b.example.com")
        };
        save_keystore_in(&ks_a, Some(dir.path())).unwrap();
        save_keystore_in(&ks_b, Some(dir.path())).unwrap();
        let file_a = std::fs::read(keystore_path_in("a.example.com", Some(dir.path()))).unwrap();
        let file_b = std::fs::read(keystore_path_in("b.example.com", Some(dir.path()))).unwrap();
        // Different ciphertext (random nonce + different key derivation)
        assert_ne!(file_a, file_b);
    }

    #[test]
    fn test_domain_binding() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        // Try to load with wrong domain — should fail decryption
        let path_a = keystore_path_in("sunbeam.pt", Some(dir.path()));
        let path_b = keystore_path_in("evil.com", Some(dir.path()));
        std::fs::copy(&path_a, &path_b).unwrap();
        let result = load_keystore_in("evil.com", Some(dir.path()));
        assert!(result.is_err());
    }

    // -- Machine salt --------------------------------------------------------

    #[test]
    fn test_machine_salt_created_on_first_use() {
        let dir = TempDir::new().unwrap();
        let salt_path = machine_salt_path(Some(dir.path()));
        assert!(!salt_path.exists());
        let salt = load_or_create_machine_salt(Some(dir.path())).unwrap();
        assert!(salt_path.exists());
        assert_eq!(salt.len(), MACHINE_SALT_LEN);
    }

    #[test]
    fn test_machine_salt_reused_on_subsequent_calls() {
        let dir = TempDir::new().unwrap();
        let salt1 = load_or_create_machine_salt(Some(dir.path())).unwrap();
        let salt2 = load_or_create_machine_salt(Some(dir.path())).unwrap();
        assert_eq!(salt1, salt2);
    }

    #[cfg(unix)]
    #[test]
    fn test_machine_salt_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        load_or_create_machine_salt(Some(dir.path())).unwrap();
        let path = machine_salt_path(Some(dir.path()));
        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn test_machine_salt_32_bytes() {
        let dir = TempDir::new().unwrap();
        let salt = load_or_create_machine_salt(Some(dir.path())).unwrap();
        assert_eq!(salt.len(), 32);
    }

    // -- File integrity ------------------------------------------------------

    #[test]
    fn test_corrupt_nonce() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let path = keystore_path_in("sunbeam.pt", Some(dir.path()));
        let mut data = std::fs::read(&path).unwrap();
        data[0] ^= 0xFF; // flip bits in nonce
        std::fs::write(&path, &data).unwrap();
        assert!(load_keystore_in("sunbeam.pt", Some(dir.path())).is_err());
    }

    #[test]
    fn test_corrupt_ciphertext() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let path = keystore_path_in("sunbeam.pt", Some(dir.path()));
        let mut data = std::fs::read(&path).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0xFF; // flip bits in ciphertext
        std::fs::write(&path, &data).unwrap();
        assert!(load_keystore_in("sunbeam.pt", Some(dir.path())).is_err());
    }

    #[test]
    fn test_truncated_file() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let path = keystore_path_in("sunbeam.pt", Some(dir.path()));
        std::fs::write(&path, &[0u8; 10]).unwrap(); // too short
        let result = load_keystore_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn test_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = keystore_path_in("sunbeam.pt", Some(dir.path()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &[]).unwrap();
        let result = load_keystore_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_wrong_version() {
        let dir = TempDir::new().unwrap();
        let mut ks = test_keystore("sunbeam.pt");
        ks.version = 99;
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let result = load_keystore_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not supported"));
    }

    // -- Concurrency / edge cases -------------------------------------------

    #[test]
    fn test_save_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let ks1 = test_keystore("sunbeam.pt");
        save_keystore_in(&ks1, Some(dir.path())).unwrap();
        let mut ks2 = test_keystore("sunbeam.pt");
        ks2.root_token = "hvs.new-token".into();
        save_keystore_in(&ks2, Some(dir.path())).unwrap();
        let loaded = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(loaded.root_token, "hvs.new-token");
    }

    #[test]
    fn test_load_nonexistent_domain() {
        let dir = TempDir::new().unwrap();
        let result = load_keystore_in("nonexistent.example.com", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no vault keystore"));
    }

    #[test]
    fn test_keystore_exists_true() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        assert!(keystore_exists_in("sunbeam.pt", Some(dir.path())));
    }

    #[test]
    fn test_keystore_exists_false() {
        let dir = TempDir::new().unwrap();
        assert!(!keystore_exists_in("sunbeam.pt", Some(dir.path())));
    }

    #[test]
    fn test_save_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("deeply").join("nested").join("vault");
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(&nested)).unwrap();
        assert!(keystore_path_in("sunbeam.pt", Some(&nested)).exists());
    }

    // -- Field validation ---------------------------------------------------

    #[test]
    fn test_verify_rejects_empty_root_token() {
        let dir = TempDir::new().unwrap();
        let mut ks = test_keystore("sunbeam.pt");
        ks.root_token = String::new();
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let result = verify_vault_keys_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty root_token"));
    }

    #[test]
    fn test_verify_rejects_empty_unseal_keys() {
        let dir = TempDir::new().unwrap();
        let mut ks = test_keystore("sunbeam.pt");
        ks.unseal_keys_b64 = vec![];
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let result = verify_vault_keys_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no unseal keys"));
    }

    #[test]
    fn test_verify_rejects_zero_shares() {
        let dir = TempDir::new().unwrap();
        let mut ks = test_keystore("sunbeam.pt");
        ks.key_shares = 0;
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let result = verify_vault_keys_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("key_shares=0"));
    }

    #[test]
    fn test_verify_rejects_invalid_threshold() {
        let dir = TempDir::new().unwrap();
        let mut ks = test_keystore("sunbeam.pt");
        ks.key_shares = 3;
        ks.key_threshold = 5; // threshold > shares
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let result = verify_vault_keys_in("sunbeam.pt", Some(dir.path()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid threshold"));
    }

    // -- Integration-style ---------------------------------------------------

    #[test]
    fn test_full_lifecycle() {
        let dir = TempDir::new().unwrap();
        // Create
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        // Verify
        let verified = verify_vault_keys_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(verified.root_token, ks.root_token);
        // Modify
        let mut ks2 = verified;
        ks2.root_token = "hvs.rotated-token".into();
        ks2.updated_at = Utc::now();
        save_keystore_in(&ks2, Some(dir.path())).unwrap();
        // Reload
        let reloaded = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(reloaded.root_token, "hvs.rotated-token");
    }

    #[test]
    fn test_export_plaintext_format() {
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        // Export by loading and serializing (mirrors the public function logic)
        let loaded = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        let json = serde_json::to_string_pretty(&loaded).unwrap();
        assert!(json.contains("hvs.test-root-token-abc123"));
        assert!(json.contains("sunbeam.pt"));
        assert!(json.contains("\"version\": 1"));
    }

    #[test]
    fn test_reinit_flow() {
        let dir = TempDir::new().unwrap();
        // Initial save
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        // Simulate: cluster keys are lost — local keystore still has them
        let recovered = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(recovered.root_token, ks.root_token);
        assert_eq!(recovered.unseal_keys_b64, ks.unseal_keys_b64);
        // Simulate: reinit with new keys
        let mut new_ks = test_keystore("sunbeam.pt");
        new_ks.root_token = "hvs.new-after-reinit".into();
        new_ks.unseal_keys_b64 = vec!["bmV3LXVuc2VhbC1rZXk=".into()];
        save_keystore_in(&new_ks, Some(dir.path())).unwrap();
        let final_ks = load_keystore_in("sunbeam.pt", Some(dir.path())).unwrap();
        assert_eq!(final_ks.root_token, "hvs.new-after-reinit");
    }

    #[cfg(unix)]
    #[test]
    fn test_keystore_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let ks = test_keystore("sunbeam.pt");
        save_keystore_in(&ks, Some(dir.path())).unwrap();
        let path = keystore_path_in("sunbeam.pt", Some(dir.path()));
        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }
}
