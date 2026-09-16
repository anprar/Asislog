// English comments: follow/tail rotation detection.
// Identity = size + mtime + head/tail fingerprints, so an overwrite that
// keeps the size is read as rotation, not as append. Head catches rewrites
// of the beginning; tail catches same-size edits beyond the head window
// (klogg parity: hash header + tail instead of header only).

use std::path::Path;
use std::time::SystemTime;

/// First bytes fingerprinted per poll (cheap, OS-cached).
pub const HEAD_FP_BYTES: usize = 64 * 1024;
/// Tail bytes fingerprinted per poll (rotation detection far from head).
pub const TAIL_FP_BYTES: usize = 16 * 1024;
/// Follow poll interval (ms), shared by every tab.
pub const FOLLOW_POLL_MS: u64 = 250;

fn fnv1a(buf: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in buf {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Snapshot of file identity for follow polling.
#[derive(Clone, Debug)]
pub struct FileIdentity {
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// FNV-1a hex of the first bytes (None when unreadable).
    pub head: Option<String>,
    /// FNV-1a hex of the last TAIL_FP_BYTES (None when unreadable/too small).
    pub tail: Option<String>,
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
    Some(format!("{:016x}:{}", fnv1a(&buf[..n]), n))
}

/// Fingerprint the last TAIL_FP_BYTES (None for empty/unreadable/small files).
fn tail_fp(path: &Path, size: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    if size < TAIL_FP_BYTES as u64 {
        return None; // head covers small files entirely
    }
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::End(-(TAIL_FP_BYTES as i64))).ok()?;
    let mut buf = vec![0u8; TAIL_FP_BYTES];
    let n = f.read(&mut buf).ok()?;
    Some(format!("{:016x}:{}", fnv1a(&buf[..n]), n))
}

/// Read current identity. Missing file -> error (caller shows Indonesian message).
pub fn load_identity(path: &Path) -> std::io::Result<FileIdentity> {
    let md = std::fs::metadata(path)?;
    let size = md.len();
    Ok(FileIdentity {
        size,
        mtime: md.modified().ok(),
        head: head_fp(path),
        tail: tail_fp(path, size),
    })
}

/// Pure decision: compare previous snapshot with current.
/// `prev_head=None` (unknown) falls back to size-only logic.
/// Tail guards same-size overwrites beyond the head window.
pub fn check_follow(
    prev_size: u64,
    prev_head: Option<&str>,
    cur: &FileIdentity,
) -> FollowEvent {
    check_follow_full(prev_size, prev_head, None, cur)
}

/// Full decision with tail fingerprint (klogg-style header+tail check).
/// `prev_tail=None` = unknown (first poll) — head/size still decide.
/// Append pada file kecil: head window mencakup seluruh file, sehingga
/// append mengubah head. Deteksi: bila size NAIK dan seluruh konten lama
/// (panjang n_old dari head lama) masih menjadi prefix head baru yang
/// lebih panjang, itu append murni — bukan overwrite.
pub fn check_follow_full(
    prev_size: u64,
    prev_head: Option<&str>,
    prev_tail: Option<&str>,
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
            // Same size + same head: check the tail too, so a same-size
            // rewrite past the head window is still caught as rotation.
            let same_tail = match (prev_tail, cur.tail.as_deref()) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            if same_tail {
                return FollowEvent::Unchanged;
            }
            return FollowEvent::TruncatedOrRotated;
        }
        return FollowEvent::TruncatedOrRotated;
    }
    // Grew. Same head = append. Different head = maybe append on a SMALL
    // file (head window covers the whole file, so appended bytes change it):
    // treat as append when the old content is a strict prefix of the new
    // head content; otherwise the beginning was rewritten -> rotation.
    if same_head {
        return FollowEvent::Appended(cur.size);
    }
    if let (Some(old), Some(new)) = (prev_head, cur.head.as_deref()) {
        if head_is_grown_prefix(old, new, prev_size) {
            return FollowEvent::Appended(cur.size);
        }
    }
    FollowEvent::TruncatedOrRotated
}

/// True when the old head fingerprint (hash:len) is a byte-prefix of the
/// new one: same hash up to old_len, and the new fingerprint is longer.
/// Requires re-reading bytes, so compare lazily via length arithmetic on
/// the stored `hash:len` encodings: hash equality can't be checked without
/// the bytes, so the caller passes prev_size; we only trust this path when
/// the OLD head covered the whole previous file (prev_size <= HEAD_FP_BYTES),
/// meaning the hash covers all old bytes. Bigger files keep an identical
/// head on pure append (first window untouched), so grew + changed head
/// there is a real overwrite -> rotation. Small files genuinely ambiguous
/// (append changes the hash too): append wins (common real case).
fn head_is_grown_prefix(_old: &str, _new: &str, prev_size: u64) -> bool {
    // Window fully covered the old file (head len == prev size is encoded
    // by the caller passing prev_size; we cannot verify hashes here).
    // Decision delegated to size logic: any growth on a file whose head
    // window covered it fully reads as append — an overwrite that GROWS
    // while keeping the first prev_size bytes identical is indistinguishable
    // from append at this budget, and appending is the common real case.
    prev_size > 0 && prev_size <= HEAD_FP_BYTES as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(size: u64, head: Option<&str>, tail: Option<&str>) -> FileIdentity {
        FileIdentity {
            size,
            mtime: None,
            head: head.map(|s| s.to_string()),
            tail: tail.map(|s| s.to_string()),
        }
    }

    #[test]
    fn append_detected() {
        let cur = id(200, Some("aa"), Some("t1"));
        assert_eq!(
            check_follow_full(100, Some("aa"), Some("t0"), &cur),
            FollowEvent::Appended(200)
        );
    }

    #[test]
    fn truncate_is_rotation() {
        let cur = id(50, Some("aa"), Some("t1"));
        assert_eq!(
            check_follow_full(100, Some("aa"), Some("t0"), &cur),
            FollowEvent::TruncatedOrRotated
        );
    }

    #[test]
    fn unchanged_same_size() {
        let cur = id(100, Some("aa"), Some("t1"));
        assert_eq!(
            check_follow_full(100, Some("aa"), Some("t1"), &cur),
            FollowEvent::Unchanged
        );
        // Legacy 2-arg path (no tail) still works.
        assert_eq!(check_follow(100, Some("aa"), &cur), FollowEvent::Unchanged);
    }

    #[test]
    fn overwrite_same_size_is_rotation() {
        // Same size, different head: replacement, not append.
        let cur = id(100, Some("bb"), Some("t1"));
        assert_eq!(
            check_follow_full(100, Some("aa"), Some("t1"), &cur),
            FollowEvent::TruncatedOrRotated
        );
    }

    #[test]
    fn tail_catches_far_overwrite() {
        // Same size, SAME head, different tail: old head-only logic read
        // this as Unchanged; the tail fingerprint catches the rewrite.
        let cur = id(100, Some("aa"), Some("XX"));
        assert_eq!(
            check_follow_full(100, Some("aa"), Some("t1"), &cur),
            FollowEvent::TruncatedOrRotated
        );
        // Unknown tail (small file / first poll): head still decides.
        assert_eq!(
            check_follow_full(100, Some("aa"), None, &cur),
            FollowEvent::Unchanged
        );
    }

    #[test]
    fn overwrite_grown_is_rotation() {
        // Big file: head window (64 KiB) doesn't cover the whole old file,
        // so pure append keeps the head identical — changed head + growth
        // is a real overwrite, not an append.
        let cur = id(400 * 1024, Some("bb"), Some("t1"));
        assert_eq!(
            check_follow_full(300 * 1024, Some("aa"), Some("t0"), &cur),
            FollowEvent::TruncatedOrRotated
        );
    }

    #[test]
    fn small_grown_changed_head_reads_as_append() {
        // Small file: the head covers the whole old file, so even a pure
        // append changes its hash — indistinguishable from overwrite-grow
        // without byte compare, so append wins (common real case).
        let cur = id(200, Some("bb"), Some("t1"));
        assert_eq!(
            check_follow_full(100, Some("aa"), Some("t0"), &cur),
            FollowEvent::Appended(200)
        );
    }

    #[test]
    fn unknown_head_falls_back_to_size() {
        let cur = id(200, None, None);
        assert_eq!(check_follow_full(100, None, None, &cur), FollowEvent::Appended(200));
        let cur = id(100, None, None);
        assert_eq!(check_follow_full(100, None, None, &cur), FollowEvent::Unchanged);
    }

    #[test]
    fn tail_fp_reads_last_window() {
        let dir = tempfile_dir();
        let path = dir.path().join("t.log");
        // 100 KB file: head and tail windows are disjoint.
        let mut data = vec![b'a'; 100 * 1024];
        for (i, b) in data.iter_mut().enumerate() {
            *b = if i < 50 * 1024 { b'a' } else { b'b' };
        }
        std::fs::write(&path, &data).unwrap();
        let idn = load_identity(&path).unwrap();
        assert_eq!(idn.size, data.len() as u64);
        assert!(idn.head.is_some());
        assert!(idn.tail.is_some(), "tail must fingerprint big files");
        // Same size, head rewritten -> rotation via head.
        for (i, b) in data.iter_mut().enumerate() {
            if i < 64 * 1024 {
                *b = b'z';
            }
        }
        std::fs::write(&path, &data).unwrap();
        let idn2 = load_identity(&path).unwrap();
        assert_eq!(
            check_follow_full(idn.size, idn.head.as_deref(), idn.tail.as_deref(), &idn2),
            FollowEvent::TruncatedOrRotated
        );
    }

    fn tempfile_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
}
