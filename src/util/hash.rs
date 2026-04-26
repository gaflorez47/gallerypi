use sha2::{Digest, Sha256};

/// Generate a cache key for a thumbnail: sha256(path + mtime), hex-encoded first 32 chars.
pub fn thumb_cache_key(path: &str, mtime: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    hasher.update(mtime.to_le_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..16]) // 32 hex chars
}
