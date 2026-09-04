// English comments: follow/tail rotation detection.
// Identity = size + mtime + head fingerprint, so an overwrite that keeps
// the size is read as rotation, not as append.

use std::path::Path;
use std::time::SystemTime;

/// First bytes fingerprinted per poll (cheap, OS-cached).
pub const HEAD_FP_BYTES: usize = 4096;

/// Snapshot of file identity for follow polling.
#[derive(Clone, Debug)]
pub struct FileIdentity {
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// FNV-1a hex of the first bytes (None when unreadable).
    pub head: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowEvent {
    /// File grew with identical head: index only the new tail.
    Appended(u64),
    /// File shrank or was replaced: reopen from start.
    TruncatedOrRotated,
    Unchanged,
}

/// Fingerprint the first bytes of a file (None on any error).
pub fn head_fp(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD_FP_BYTES];
    let n = f.read(&mut buf).ok()?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in &buf[..n] {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    Some(format!("{:016x}:{}", h, n))
}

/// Read current identity. Missing file -> error (caller shows Indonesian message).
pub fn load_identity(path: &Path) -> std::io::Result<FileIdentity> {
    let md = std::fs::metadata(path)?;
    Ok(FileIdentity {
        size: md.len(),
        mtime: md.modified().ok(),
        head: head_fp(path),
    })
}

/// Pure decision: compare previous snapshot with current.
/// `prev_head=None` (unknown) falls back to size-only logic.
pub fn check_follow(
    prev_size: u64,
    prev_head: Option<&str>,
    cur: &FileIdentity,
) -> FollowEvent {
    if cur.size < prev_size {
        return FollowEvent::TruncatedOrRotated;
    }
    let same_head = match (prev_head, cur.head.as_deref()) {
        (Some(a), Some(b)) => a == b,
        // Unknown fingerprint: trust size only (first poll after open).
        _ => true,
    };
    if cur.size == prev_size {
        if same_head {
            return FollowEvent::Unchanged;
        }
        return FollowEvent::TruncatedOrRotated;
    }
    // Grew: same head = append; different head = overwrite.
    if same_head {
        FollowEvent::Appended(cur.size)
    } else {
        FollowEvent::TruncatedOrRotated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(size: u64, head: Option<&str>) -> FileIdentity {
        FileIdentity { size, mtime: None, head: head.map(|s| s.to_string()) }
    }

    #[test]
    fn append_detected() {
        let cur = id(200, Some("aa"));
        assert_eq!(check_follow(100, Some("aa"), &cur), FollowEvent::Appended(200));
    }

    #[test]
    fn truncate_is_rotation() {
        let cur = id(50, Some("aa"));
        assert_eq!(check_follow(100, Some("aa"), &cur), FollowEvent::TruncatedOrRotated);
    }

    #[test]
    fn unchanged_same_size() {
        let cur = id(100, Some("aa"));
        assert_eq!(check_follow(100, Some("aa"), &cur), FollowEvent::Unchanged);
    }

    #[test]
    fn overwrite_same_size_is_rotation() {
        // Same size, different head: replacement, not append.
        let cur = id(100, Some("bb"));
        assert_eq!(
            check_follow(100, Some("aa"), &cur),
            FollowEvent::TruncatedOrRotated
        );
    }

    #[test]
    fn overwrite_grown_is_rotation() {
        let cur = id(200, Some("bb"));
        assert_eq!(
            check_follow(100, Some("aa"), &cur),
            FollowEvent::TruncatedOrRotated
        );
    }

    #[test]
    fn unknown_head_falls_back_to_size() {
        let cur = id(200, None);
        assert_eq!(check_follow(100, None, &cur), FollowEvent::Appended(200));
        let cur = id(100, None);
        assert_eq!(check_follow(100, None, &cur), FollowEvent::Unchanged);
    }
}
