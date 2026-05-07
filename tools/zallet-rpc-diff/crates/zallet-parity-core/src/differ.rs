//! Structured JSON diff walker.
//!
//! Compares two `serde_json::Value` trees recursively and returns a list
//! of leaf-level differences as [`DiffEntry`] items, each identified by
//! its JSON Pointer path (RFC 6901).
//!
//! This replaces the plain `diff_message: String` from `assert-json-diff`
//! with a structured, serializable output suitable for the parity report.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single leaf-level difference between upstream and target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffEntry {
    /// RFC 6901 JSON Pointer to the location of the difference.
    /// Example: `/softforks/0/id`, `/chain`
    pub path: String,
    /// The value from the upstream (zcashd) response.
    pub upstream: Value,
    /// The value from the target (Zallet) response.
    pub target: Value,
}

/// Recursively compares `upstream` and `target`, collecting all leaf-level
/// differences into a `Vec<DiffEntry>`.
///
/// - Object keys present in one but absent from the other are reported.
/// - Array length mismatches are reported at the array path, then elements
///   are compared index-by-index up to the shorter length.
/// - Scalar differences are reported at the exact path.
pub fn diff_values(upstream: &Value, target: &Value) -> Vec<DiffEntry> {
    let mut entries = Vec::new();
    diff_recursive(upstream, target, String::new(), &mut entries);
    entries
}

fn diff_recursive(upstream: &Value, target: &Value, path: String, out: &mut Vec<DiffEntry>) {
    match (upstream, target) {
        (Value::Object(u_map), Value::Object(t_map)) => {
            // Keys in upstream
            for (key, u_val) in u_map {
                let child_path = format!("{}/{}", path, escape_token(key));
                match t_map.get(key) {
                    Some(t_val) => diff_recursive(u_val, t_val, child_path, out),
                    None => out.push(DiffEntry {
                        path: child_path,
                        upstream: u_val.clone(),
                        target: Value::Null,
                    }),
                }
            }
            // Keys only in target
            for (key, t_val) in t_map {
                if !u_map.contains_key(key) {
                    out.push(DiffEntry {
                        path: format!("{}/{}", path, escape_token(key)),
                        upstream: Value::Null,
                        target: t_val.clone(),
                    });
                }
            }
        }
        (Value::Array(u_arr), Value::Array(t_arr)) => {
            let max_len = u_arr.len().max(t_arr.len());
            for i in 0..max_len {
                let child_path = format!("{}/{}", path, i);
                match (u_arr.get(i), t_arr.get(i)) {
                    (Some(u), Some(t)) => diff_recursive(u, t, child_path, out),
                    (Some(u), None) => out.push(DiffEntry {
                        path: child_path,
                        upstream: u.clone(),
                        target: Value::Null,
                    }),
                    (None, Some(t)) => out.push(DiffEntry {
                        path: child_path,
                        upstream: Value::Null,
                        target: t.clone(),
                    }),
                    (None, None) => {}
                }
            }
        }
        (u, t) if u != t => {
            out.push(DiffEntry {
                path: if path.is_empty() { "/".to_string() } else { path },
                upstream: u.clone(),
                target: t.clone(),
            });
        }
        _ => {} // Equal scalars / null — no diff
    }
}

/// Escapes a JSON Pointer token as per RFC 6901:
/// `~` → `~0`, `/` → `~1`
fn escape_token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_equal_values_produce_no_diff() {
        let a = json!({"chain": "main", "blocks": 100});
        let b = json!({"chain": "main", "blocks": 100});
        assert!(diff_values(&a, &b).is_empty());
    }

    #[test]
    fn test_scalar_diff_at_root_field() {
        let a = json!({"chain": "main"});
        let b = json!({"chain": "test"});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "/chain");
        assert_eq!(diffs[0].upstream, json!("main"));
        assert_eq!(diffs[0].target, json!("test"));
    }

    #[test]
    fn test_nested_diff_has_correct_path() {
        let a = json!({"status": {"synced": true, "blocks": 100}});
        let b = json!({"status": {"synced": false, "blocks": 100}});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "/status/synced");
    }

    #[test]
    fn test_missing_key_in_target_is_reported() {
        let a = json!({"chain": "main", "extra": "field"});
        let b = json!({"chain": "main"});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "/extra");
        assert_eq!(diffs[0].target, Value::Null);
    }

    #[test]
    fn test_extra_key_in_target_is_reported() {
        let a = json!({"chain": "main"});
        let b = json!({"chain": "main", "extra": "field"});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "/extra");
        assert_eq!(diffs[0].upstream, Value::Null);
    }

    #[test]
    fn test_array_element_diff_path_includes_index() {
        let a = json!({"softforks": [{"id": "csv"}, {"id": "segwit"}]});
        let b = json!({"softforks": [{"id": "csv"}, {"id": "taproot"}]});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "/softforks/1/id");
    }

    #[test]
    fn test_key_with_special_chars_is_escaped() {
        let a = json!({"a/b": 1});
        let b = json!({"a/b": 2});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 1);
        // '/' in key must be escaped as '~1'
        assert_eq!(diffs[0].path, "/a~1b");
    }

    #[test]
    fn test_multiple_diffs_all_reported() {
        let a = json!({"x": 1, "y": 2, "z": 3});
        let b = json!({"x": 1, "y": 99, "z": 99});
        let diffs = diff_values(&a, &b);
        assert_eq!(diffs.len(), 2);
        let paths: Vec<&str> = diffs.iter().map(|d| d.path.as_str()).collect();
        assert!(paths.contains(&"/y"));
        assert!(paths.contains(&"/z"));
    }
}
