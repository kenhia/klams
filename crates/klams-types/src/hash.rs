//! Canonical SHA-256 hashing for dedupe (facts and knowledge).

use sha2::{Digest, Sha256};

/// Compute a deterministic SHA-256 of `(type, payload)` where `payload`
/// is re-encoded with object keys sorted lexicographically and no
/// extraneous whitespace.
///
/// The hash is stable across:
/// - object key order
/// - insignificant whitespace in the source JSON
/// - any unicode escaping choice (since `serde_json` re-encodes normally)
#[must_use]
pub fn canonical_json_hash(kind: &str, payload: &serde_json::Value) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(b"\0");
    write_canonical(&mut hasher, payload);
    let out = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

fn write_canonical(hasher: &mut Sha256, v: &serde_json::Value) {
    match v {
        serde_json::Value::Null => hasher.update(b"null"),
        serde_json::Value::Bool(b) => hasher.update(if *b { b"true".as_ref() } else { b"false" }),
        serde_json::Value::Number(n) => hasher.update(n.to_string().as_bytes()),
        serde_json::Value::String(s) => {
            let encoded = serde_json::to_string(s).expect("string serialization is infallible");
            hasher.update(encoded.as_bytes());
        }
        serde_json::Value::Array(arr) => {
            hasher.update(b"[");
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    hasher.update(b",");
                }
                write_canonical(hasher, item);
            }
            hasher.update(b"]");
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            hasher.update(b"{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    hasher.update(b",");
                }
                let encoded = serde_json::to_string(k).expect("string serialization is infallible");
                hasher.update(encoded.as_bytes());
                hasher.update(b":");
                write_canonical(hasher, &map[*k]);
            }
            hasher.update(b"}");
        }
    }
}

#[cfg(test)]
mod task_id_dedupe_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_payload_and_task_id_hashes_equal() {
        let a = json!({"key": "GPU_COUNT", "value": "2", "task_id": "ansible-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
        let b = json!({"task_id": "ansible-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "value": "2", "key": "GPU_COUNT"});
        assert_eq!(
            canonical_json_hash("EnvFact", &a),
            canonical_json_hash("EnvFact", &b)
        );
    }

    #[test]
    fn different_task_id_changes_hash() {
        let a = json!({"key": "GPU_COUNT", "value": "2", "task_id": "ansible-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
        let b = json!({"key": "GPU_COUNT", "value": "2", "task_id": "ansible-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"});
        assert_ne!(
            canonical_json_hash("EnvFact", &a),
            canonical_json_hash("EnvFact", &b)
        );
    }
}
