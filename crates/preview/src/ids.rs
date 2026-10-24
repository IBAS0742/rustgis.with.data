//! Opaque, filesystem-safe identifiers.
//!
//! IDs are generated with an OS-seeded hasher, the process id and a monotonic counter. They are
//! not secrets and are never accepted as path fragments without [`is_component`].

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a new ID such as `ver_1f3a...`.
pub fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut a = std::collections::hash_map::RandomState::new().build_hasher();
    a.write_u128(nanos);
    a.write_u32(std::process::id());
    a.write_u64(counter);
    let mut b = std::collections::hash_map::RandomState::new().build_hasher();
    b.write_u128(nanos ^ 0x9e37_79b9_7f4a_7c15);
    b.write_u32(std::process::id());
    b.write_u64(counter.wrapping_add(1));
    format!("{prefix}_{:016x}{:016x}", a.finish(), b.finish())
}

/// Whether `s` is one path component that this crate will safely join under a directory.
pub fn is_component(s: &str) -> bool {
    !s.is_empty() && s.len() <= 160 && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `s` is a generated ID with the given prefix.
pub fn is_id(s: &str, prefix: &str) -> bool {
    is_component(s) && s.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('_') && rest.len() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_components_and_distinct() {
        let a = new_id("ver");
        let b = new_id("ver");
        assert_ne!(a, b);
        assert!(is_component(&a));
        assert!(is_id(&a, "ver"));
    }

    #[test]
    fn traversal_is_not_a_component() {
        for name in ["", ".", "..", "../x", "a/b", "a\\b", "C:", "a b", "é"] {
            assert!(!is_component(name), "{name:?}");
        }
    }
}
