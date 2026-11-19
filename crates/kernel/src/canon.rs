//! Canonical serialisation and stable hashing.
//!
//! The canonical form of a value is its JSON encoding with object keys sorted by byte order, no
//! insignificant whitespace, and integers only (the kernel has no floating point). Two equal
//! values always have the same canonical bytes, and therefore the same SHA-256 hash.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Canonical JSON text of any serialisable kernel value.
pub fn canonical_json<T: Serialize>(value: &T) -> String {
    let tree = serde_json::to_value(value).expect("kernel values serialise to JSON");
    let mut out = String::new();
    write_canonical(&tree, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String((*key).clone()).to_string());
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// Lower-case hex SHA-256 of the canonical form.
pub fn hash_of<T: Serialize>(value: &T) -> String {
    hash_bytes(canonical_json(value).as_bytes())
}

/// Lower-case hex SHA-256 of raw bytes.
///
/// Implements: H-010
pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble"));
        hex.push(char::from_digit((byte & 0xf) as u32, 16).expect("nibble"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_sorted_whatever_the_declaration_order() {
        #[derive(Serialize)]
        struct A {
            b: u8,
            a: u8,
        }
        #[derive(Serialize)]
        struct B {
            a: u8,
            b: u8,
        }
        assert_eq!(canonical_json(&A { b: 2, a: 1 }), r#"{"a":1,"b":2}"#);
        assert_eq!(hash_of(&A { b: 2, a: 1 }), hash_of(&B { a: 1, b: 2 }));
    }

    #[test]
    fn hash_is_the_known_sha256() {
        assert_eq!(
            hash_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
