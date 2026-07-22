//! K8s secret mirroring steps.
//!
//! CreateK8sSecrets is now handled by the CreateK8sSecret primitive.
//! build_s3_json is used by the SeaweedFS secret creation.

// -- Pure helpers (used by CreateK8sSecret primitive) -------------------------

pub(crate) fn build_s3_json(access_key: &str, secret_key: &str) -> String {
    serde_json::json!({
        "identities": [{
            "name": "seaweed",
            "credentials": [{"accessKey": access_key, "secretKey": secret_key}],
            "actions": ["Admin", "Read", "Write", "List", "Tagging"]
        }]
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_s3_json_valid() {
        let json = build_s3_json("AK", "SK");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["identities"][0]["credentials"][0]["accessKey"], "AK");
    }
}
