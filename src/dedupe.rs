//! Exact duplicate detection: a SHA-256 of what makes an upload itself.
//!
//! For a still, that is the decoded pixel buffer rather than the uploaded
//! bytes: the same image re-saved as a different format, or with different
//! compression settings, produces different file bytes but identical pixels,
//! and is still the same item. For a GIF or a video it is the file itself,
//! since neither is decoded on the server (see `storage`).
//!
//! Nothing here looks at *what* an upload depicts. Near-duplicate detection
//! (a resize, a recompress, a re-encode of a clip) is a deliberate non-goal for
//! now: it is a model or a perceptual hash and a threshold, and thresholds
//! against memes are wrong in both directions.

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_bytes_hash_the_same() {
        assert_eq!(sha256_hex(b"a item"), sha256_hex(b"a item"));
    }

    #[test]
    fn different_bytes_hash_differently() {
        assert_ne!(sha256_hex(b"a item"), sha256_hex(b"a different item"));
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = sha256_hex(b"anything");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Pinned to a known vector so a swapped hasher cannot pass silently.
    #[test]
    fn matches_the_reference_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
