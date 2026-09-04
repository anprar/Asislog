// English comments: persistent bookmarks in a per-file JSON sidecar.
// Validates head fingerprint so growth (append) is accepted but
// replacement is flagged.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Bookmark, BookmarkColor};

const SIDECAR_VERSION: u32 = 1;
const FINGER_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
struct Sidecar {
    version: u32,
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    fingerprint: String,
    bookmarks: Vec<MarkRec>,
}

#[derive(Serialize, Deserialize, Clone)]
struct MarkRec {
    line: u64,
    byte: u64,
    label: String,
    color: String,
}

/// Sidecar path: `<file>.asislog.json` (beside the log, deletable).
pub fn sidecar_path(log_path: &Path) -> PathBuf {
    let mut s = log_path.as_os_str().to_owned();
    s.push(".asislog.json");
    PathBuf::from(s)
}

/// FNV-1a 64 over head 64 KiB + tail 64 KiB + size.
/// Cheap fingerprint: append-only growth keeps the head identical.
pub fn fingerprint(path: &Path, size: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |b: &[u8]| {
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    mix(&size.to_le_bytes());
    let mut buf = vec![0u8; FINGER_BYTES];
    let n = f.read(&mut buf).ok()?;
    mix(&buf[..n]);
    if size > FINGER_BYTES as u64 && f.seek(SeekFrom::End(-(FINGER_BYTES as i64))).is_ok() {
        let n = f.read(&mut buf).unwrap_or(0);
        mix(&buf[..n]);
    }
    Some(format!("{:016x}", h))
}

fn mtime_key(path: &Path) -> (u64, u32) {
    match std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    {
        Some(d) => (d.as_secs(), d.subsec_nanos()),
        None => (0, 0),
    }
}

/// Save bookmarks. Best effort; errors returned as Indonesian text.
pub fn save(path: &Path, bookmarks: &[Bookmark]) -> Result<(), String> {
    let md = std::fs::metadata(path)
        .map_err(|e| format!("Gagal membaca file untuk sidecar: {}", e))?;
    let size = md.len();
    let (s, n) = mtime_key(path);
    let fp = fingerprint(path, size).unwrap_or_default();
    let sc = Sidecar {
        version: SIDECAR_VERSION,
        size,
        mtime_secs: s,
        mtime_nanos: n,
        fingerprint: fp,
        bookmarks: bookmarks
            .iter()
            .map(|b| MarkRec {
                line: b.line,
                byte: b.byte,
                label: b.label.clone(),
                color: b.color.key().to_string(),
            })
            .collect(),
    };
    let text = serde_json::to_string_pretty(&sc)
        .map_err(|e| format!("Gagal menyusun sidecar: {}", e))?;
    std::fs::write(sidecar_path(path), text)
        .map_err(|e| format!("Gagal menulis sidecar: {}", e))
}

/// Load result: (bookmarks, optional Indonesian warning).
/// Missing sidecar → empty without warning. Head mismatch → loaded + warning.
pub fn load(path: &Path) -> (Vec<Bookmark>, Option<String>) {
    let sc_path = sidecar_path(path);
    let text = match std::fs::read_to_string(&sc_path) {
        Ok(t) => t,
        Err(_) => return (Vec::new(), None),
    };
    let sc: Sidecar = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(_) => {
            return (
                Vec::new(),
                Some(String::from("Sidecar penanda rusak; diabaikan.")),
            )
        }
    };
    if sc.version != SIDECAR_VERSION {
        return (
            Vec::new(),
            Some(String::from("Versi sidecar penanda beda; diabaikan.")),
        );
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let fp = fingerprint(path, size).unwrap_or_default();
    let mut warn = None;
    if !sc.fingerprint.is_empty() && sc.fingerprint != fp {
        warn = Some(String::from(
            "File sumber berubah; nomor baris penanda mungkin tidak valid.",
        ));
    } else if sc.size != size {
        warn = Some(String::from(
            "File bertambah/berubah ukuran; penanda dipertahankan.",
        ));
    }
    let marks = sc
        .bookmarks
        .into_iter()
        .map(|r| Bookmark {
            line: r.line,
            byte: r.byte,
            label: r.label,
            color: BookmarkColor::from_key(&r.color),
        })
        .collect();
    (marks, warn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("asislog-marks-{}-{}.log", tag, std::process::id()));
        p
    }

    #[test]
    fn roundtrip_and_growth_ok() {
        let p = tmp_path("a");
        std::fs::write(&p, "line1\nline2\n").unwrap();
        let marks = vec![Bookmark {
            line: 2,
            byte: 6,
            label: "dua".into(),
            color: BookmarkColor::Red,
        }];
        save(&p, &marks).unwrap();
        // Append growth keeps head fingerprint: no hard warning about invalidity.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            writeln!(f, "line3").unwrap();
        }
        let (loaded, _warn) = load(&p);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].label, "dua");
        assert_eq!(loaded[0].color, BookmarkColor::Red);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(sidecar_path(&p));
    }

    #[test]
    fn replacement_warns() {
        let p = tmp_path("b");
        std::fs::write(&p, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n").unwrap();
        save(
            &p,
            &[Bookmark {
                line: 1,
                byte: 0,
                label: "x".into(),
                color: BookmarkColor::Default,
            }],
        )
        .unwrap();
        // Replace head content entirely.
        std::fs::write(&p, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n").unwrap();
        let (loaded, warn) = load(&p);
        assert_eq!(loaded.len(), 1);
        assert!(warn.is_some());
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(sidecar_path(&p));
    }

    #[test]
    fn missing_sidecar_is_empty() {
        let p = tmp_path("nope-missing");
        let _ = std::fs::remove_file(sidecar_path(&p));
        let (loaded, warn) = load(&p);
        assert!(loaded.is_empty());
        assert!(warn.is_none());
    }
}
