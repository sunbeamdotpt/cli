use std::path::Path;

use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::error::ResultExt;

const KEYS_FILE: &str = "keys.json";

/// The three x25519 key pairs used by a node.
#[derive(Clone)]
pub struct NodeKeys {
    pub node_private: StaticSecret,
    pub node_public: PublicKey,
    pub disco_private: StaticSecret,
    pub disco_public: PublicKey,
    pub wg_private: StaticSecret,
    pub wg_public: PublicKey,
}

impl std::fmt::Debug for NodeKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeKeys")
            .field("node_public", &self.node_key_str())
            .field("disco_public", &self.disco_key_str())
            .field("wg_public", &hex::encode(self.wg_public.as_bytes()))
            .finish()
    }
}

/// On-disk representation of persisted keys.
#[derive(Serialize, Deserialize)]
struct PersistedKeys {
    node_private: String,
    disco_private: String,
    wg_private: String,
}

// Hex helpers — we avoid pulling in a hex crate by implementing locally.
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if !s.len().is_multiple_of(2) {
            return Err("odd length hex string".into());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    }
}

impl NodeKeys {
    /// Generate fresh random key pairs.
    pub fn generate() -> Self {
        let node_private = StaticSecret::random_from_rng(OsRng);
        let node_public = PublicKey::from(&node_private);

        let disco_private = StaticSecret::random_from_rng(OsRng);
        let disco_public = PublicKey::from(&disco_private);

        let wg_private = StaticSecret::random_from_rng(OsRng);
        let wg_public = PublicKey::from(&wg_private);

        Self {
            node_private,
            node_public,
            disco_private,
            disco_public,
            wg_private,
            wg_public,
        }
    }

    /// Load keys from `state_dir/keys.json`, or generate and persist new ones.
    pub fn load_or_generate(state_dir: &Path) -> crate::Result<Self> {
        let path = state_dir.join(KEYS_FILE);
        if path.exists() {
            let data = std::fs::read_to_string(&path).ctx("reading keys file")?;
            let persisted: PersistedKeys = serde_json::from_str(&data)?;
            return Self::from_persisted(&persisted);
        }

        let keys = Self::generate();
        std::fs::create_dir_all(state_dir).ctx("creating state directory")?;
        let persisted = keys.to_persisted();
        let data = serde_json::to_string_pretty(&persisted)?;
        std::fs::write(&path, data).ctx("writing keys file")?;
        tracing::debug!("generated new node keys at {}", path.display());
        Ok(keys)
    }

    /// Rotate only the `node_private` key in `state_dir/keys.json`, preserving
    /// the disco and wg keys. Writes atomically: temp file then rename so a
    /// crash mid-write can't corrupt the existing key.
    pub fn rotate_node_key(state_dir: &Path) -> crate::Result<()> {
        let path = state_dir.join(KEYS_FILE);
        let existing_data = std::fs::read_to_string(&path).ctx("reading keys for rotation")?;
        let mut persisted: PersistedKeys =
            serde_json::from_str(&existing_data).map_err(crate::Error::Json)?;
        let new_node_private = StaticSecret::random_from_rng(OsRng);
        persisted.node_private = hex::encode(new_node_private.as_bytes());
        let new_data = serde_json::to_string_pretty(&persisted)?;
        // Write to a temp file in the same directory then atomically rename.
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, new_data).ctx("writing rotated key to temp file")?;
        std::fs::rename(&tmp_path, &path).ctx("renaming rotated key file")?;
        tracing::info!("node key rotated and persisted to {}", path.display());
        Ok(())
    }

    /// Tailscale-style node key string: `nodekey:<hex>`.
    pub fn node_key_str(&self) -> String {
        format!("nodekey:{}", hex::encode(self.node_public.as_bytes()))
    }

    /// Tailscale-style disco key string: `discokey:<hex>`.
    pub fn disco_key_str(&self) -> String {
        format!("discokey:{}", hex::encode(self.disco_public.as_bytes()))
    }

    fn to_persisted(&self) -> PersistedKeys {
        PersistedKeys {
            node_private: hex::encode(self.node_private.as_bytes()),
            disco_private: hex::encode(self.disco_private.as_bytes()),
            wg_private: hex::encode(self.wg_private.as_bytes()),
        }
    }

    fn from_persisted(p: &PersistedKeys) -> crate::Result<Self> {
        let node_bytes = parse_key_hex(&p.node_private, "node")?;
        let disco_bytes = parse_key_hex(&p.disco_private, "disco")?;
        let wg_bytes = parse_key_hex(&p.wg_private, "wg")?;

        let node_private = StaticSecret::from(node_bytes);
        let node_public = PublicKey::from(&node_private);

        let disco_private = StaticSecret::from(disco_bytes);
        let disco_public = PublicKey::from(&disco_private);

        let wg_private = StaticSecret::from(wg_bytes);
        let wg_public = PublicKey::from(&wg_private);

        Ok(Self {
            node_private,
            node_public,
            disco_private,
            disco_public,
            wg_private,
            wg_public,
        })
    }
}

fn parse_key_hex(s: &str, name: &str) -> crate::Result<[u8; 32]> {
    let bytes =
        hex::decode(s).map_err(|e| crate::Error::Other(format!("bad {name} key hex: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| crate::Error::Other(format!("{name} key must be 32 bytes")))?;
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generate_produces_valid_keys() {
        let keys = NodeKeys::generate();
        assert!(keys.node_key_str().starts_with("nodekey:"));
        assert!(keys.disco_key_str().starts_with("discokey:"));
        assert_eq!(keys.node_key_str().len(), "nodekey:".len() + 64);
    }

    #[test]
    fn load_or_generate_round_trips() {
        let dir = TempDir::new().unwrap();
        let keys1 = NodeKeys::load_or_generate(dir.path()).unwrap();
        let keys2 = NodeKeys::load_or_generate(dir.path()).unwrap();
        assert_eq!(keys1.node_key_str(), keys2.node_key_str());
        assert_eq!(keys1.disco_key_str(), keys2.disco_key_str());
    }

    #[test]
    fn hex_round_trip() {
        let input = [0xde, 0xad, 0xbe, 0xef];
        let encoded = super::hex::encode(&input);
        assert_eq!(encoded, "deadbeef");
        let decoded = super::hex::decode(&encoded).unwrap();
        assert_eq!(decoded, input);
    }
}
