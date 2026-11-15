//! Canonical JSON: object keys sorted by code point, no insignificant whitespace, numbers as `serde_json` writes them.
//! Written out explicitly rather than relying on `serde_json`'s map order, which a `preserve_order` feature enabled
//! anywhere in a dependency graph would change.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                if let Some(v) = map.get(key) {
                    write(v, out);
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// `sha256:` followed by the lowercase hex SHA-256 of the text.
pub fn sha256_digest(text: &str) -> String {
    let hex: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_change_the_text() {
        let a = json!({"b": 1, "a": {"y": [1, {"d": 2, "c": 3}], "x": null}});
        let b: Value = serde_json::from_str(r#"{"a":{"x":null,"y":[1,{"c":3,"d":2}]},"b":1}"#)
            .unwrap_or_default();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(
            canonical_json(&a),
            r#"{"a":{"x":null,"y":[1,{"c":3,"d":2}]},"b":1}"#
        );
    }

    #[test]
    fn digest_has_a_prefix_and_64_hex_digits() {
        let d = sha256_digest("");
        assert_eq!(
            d,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(canonical_json(&json!({"k\"": "a\nb"})), r#"{"k\"":"a\nb"}"#);
    }
}
