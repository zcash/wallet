//! Canonical JSON normalization for parity comparison.
//!
//! This module eliminates ordering-only false positives by:
//! 1. Recursively sorting all object keys (stable lexicographic order).
//! 2. Removing user-specified ignore paths (JSON Pointer, RFC 6901) before comparison.

use jsonptr::PointerBuf;
use serde_json::Value;
use std::collections::BTreeMap;

/// Recursively sorts all object keys in a JSON value so that
/// two objects with the same data but different key insertion
/// orders are considered equal after normalization.
pub fn sort_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> =
                map.into_iter().map(|(k, v)| (k, sort_keys(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sort_keys).collect()),
        scalar => scalar,
    }
}

/// Removes each JSON Pointer path from `value` in-place.
/// Silently ignores paths that do not exist in the value.
pub fn apply_ignore_paths(mut value: Value, paths: &[PointerBuf]) -> Value {
    for path in paths {
        // delete() returns Ok(None) when path is not found — we discard both outcomes.
        let _ = path.delete(&mut value);
    }
    value
}

/// Full normalization pipeline: sort keys, then remove ignore paths.
pub fn normalize(value: Value, ignore_paths: &[PointerBuf]) -> Value {
    apply_ignore_paths(sort_keys(value), ignore_paths)
}

/// Parses a slice of JSON Pointer strings into `PointerBuf`s.
/// Returns an error string for any invalid pointer.
pub fn parse_ignore_paths(raw: &[String]) -> Result<Vec<PointerBuf>, String> {
    raw.iter()
        .map(|s| PointerBuf::parse(s).map_err(|e| format!("Invalid JSON Pointer '{}': {}", s, e)))
        .collect()
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── sort_keys ─────────────────────────────────────────────────────────────

    #[test]
    fn test_sort_keys_eliminates_ordering_diff() {
        let a = json!({"z": 1, "a": 2, "m": 3});
        let b = json!({"a": 2, "m": 3, "z": 1});
        assert_eq!(sort_keys(a), sort_keys(b));
    }

    #[test]
    fn test_sort_keys_nested_objects_are_sorted() {
        let a = json!({"outer": {"z": 1, "a": 2}});
        let b = json!({"outer": {"a": 2, "z": 1}});
        assert_eq!(sort_keys(a), sort_keys(b));
    }

    #[test]
    fn test_sort_keys_preserves_real_diff() {
        let a = json!({"chain": "main", "blocks": 100});
        let b = json!({"chain": "test", "blocks": 100});
        assert_ne!(sort_keys(a), sort_keys(b));
    }

    #[test]
    fn test_sort_keys_arrays_are_preserved_in_order() {
        // Array element order must NOT be changed — only object keys are sorted.
        let a = json!({"list": [3, 1, 2]});
        let b = json!({"list": [1, 2, 3]});
        // After sort_keys these should still differ (array order preserved)
        assert_ne!(sort_keys(a), sort_keys(b));
    }

    // ── apply_ignore_paths ────────────────────────────────────────────────────

    #[test]
    fn test_ignore_paths_removes_volatile_field() {
        let value = json!({"blocks": 100, "chain": "main"});
        let paths = parse_ignore_paths(&["/blocks".to_string()]).unwrap();
        let result = apply_ignore_paths(value, &paths);
        assert_eq!(result, json!({"chain": "main"}));
    }

    #[test]
    fn test_ignore_paths_nested_field() {
        let value = json!({"status": {"synced": true, "timestamp": 12345}});
        let paths = parse_ignore_paths(&["/status/timestamp".to_string()]).unwrap();
        let result = apply_ignore_paths(value, &paths);
        assert_eq!(result, json!({"status": {"synced": true}}));
    }

    #[test]
    fn test_ignore_paths_nonexistent_is_noop() {
        let value = json!({"chain": "main"});
        let paths = parse_ignore_paths(&["/nonexistent/path".to_string()]).unwrap();
        let result = apply_ignore_paths(value.clone(), &paths);
        assert_eq!(result, value);
    }

    #[test]
    fn test_ignore_paths_multiple_paths() {
        let value = json!({"a": 1, "b": 2, "c": 3});
        let paths = parse_ignore_paths(&["/a".to_string(), "/b".to_string()]).unwrap();
        let result = apply_ignore_paths(value, &paths);
        assert_eq!(result, json!({"c": 3}));
    }

    // ── normalize (combined) ──────────────────────────────────────────────────

    #[test]
    fn test_normalize_ordering_only_diff_becomes_equal() {
        let upstream = json!({"z": 1, "a": 2});
        let target = json!({"a": 2, "z": 1});
        let paths = vec![];
        assert_eq!(normalize(upstream, &paths), normalize(target, &paths));
    }

    #[test]
    fn test_normalize_ignore_paths_plus_ordering() {
        let upstream = json!({"z": 1, "a": 2, "volatile": 999});
        let target = json!({"a": 2, "z": 1, "volatile": 888});
        let paths = parse_ignore_paths(&["/volatile".to_string()]).unwrap();
        // After removing volatile and sorting, they should be equal
        assert_eq!(normalize(upstream, &paths), normalize(target, &paths));
    }
}
