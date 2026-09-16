// English comments: optional version check + previous-crash discovery.
// No vendor SDK, no account, no telemetry: the check fetches a plain-text
// version file from a user-configured URL (empty = off), and crash reports
// are the local files the panic hook already writes. Both are best-effort
// background threads that never block the UI.

use std::path::PathBuf;
use std::sync::mpsc;

/// Outcome of one version check, delivered to the UI thread.
#[derive(Clone, Debug)]
pub enum UpdateOutcome {
    /// A newer version exists upstream.
    Newer(String),
    /// Already on the newest known version.
    Current(String),
    /// Check failed (offline, bad URL, unparsable body).
    Failed(String),
}

/// True when `latest` is a newer release than `current`.
/// Accepts `v` prefix, unequal part counts (missing = 0), and pre-release
/// suffixes (`-rc1`, `+build`): release > prerelease of the same core.
/// Unparsable cores compare as NOT newer (never nag on unknown schemes).
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    fn split(s: &str) -> Option<(Vec<u64>, Option<String>)> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        if s.is_empty() {
            return None;
        }
        // Cut build metadata, then pre-release.
        let s = s.split('+').next().unwrap_or("");
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (s, None),
        };
        if core.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for p in core.split('.') {
            match p.parse::<u64>() {
                Ok(n) => parts.push(n),
                Err(_) => return None,
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some((parts, pre))
    }
    let (Some((a, a_pre)), Some((b, b_pre))) = (split(current), split(latest)) else {
        return false;
    };
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        if y != x {
            return y > x;
        }
    }
    // Same core: release beats prerelease; prerelease-vs-prerelease by label.
    match (a_pre, b_pre) {
        (Some(_), None) => true, // rc -> release upstream: update available
        (None, None) => false,   // identical versions
        (None, Some(_)) => false, // release installed, rc upstream: stay
        (Some(ap), Some(bp)) => bp > ap,
    }
}

/// Blocking fetch of the version text (http/https only, 64 KiB cap).
/// The file is plain text; the first whitespace token is the version
/// (e.g. `v0.2.0` or `0.2.0 # notes...`). Times out via the caller's
/// `recv_timeout` — this function itself just does one request.
pub fn fetch_version_text(url: &str) -> Result<String, String> {
    let t = url.trim();
    if !(t.starts_with("http://") || t.starts_with("https://")) {
        return Err(String::from("URL harus http:// atau https://."));
    }
    let resp = ureq::get(t)
        .call()
        .map_err(|e| format!("Gagal cek versi: {}", e))?;
    let body = resp.into_body().into_reader();
    use std::io::Read;
    let mut s = String::new();
    body.take(64 * 1024).read_to_string(&mut s)
        .map_err(|e| format!("Gagal cek versi: {}", e))?;
    let ver = s.split_whitespace().next().unwrap_or("").to_string();
    if ver.is_empty() {
        return Err(String::from("Berkas versi kosong."));
    }
    Ok(ver)
}

/// Background version check: fetch, compare against `current`, report.
pub fn spawn_update_check(url: String, current: String, tx: mpsc::Sender<UpdateOutcome>) {
    std::thread::spawn(move || {
        let out = match fetch_version_text(&url) {
            Ok(latest) => {
                if is_newer_version(&current, &latest) {
                    UpdateOutcome::Newer(latest)
                } else {
                    UpdateOutcome::Current(latest)
                }
            }
            Err(e) => UpdateOutcome::Failed(e),
        };
        let _ = tx.send(out);
    });
}

/// Previous-crash reports left by the panic hook (`asislog-crash-*.log`
/// in the temp dir), newest first, capped. Empty = no crash on record.
pub fn crash_reports() -> Vec<PathBuf> {
    let dir = std::env::temp_dir();
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("asislog-crash-") || !name.ends_with(".log") {
            continue;
        }
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        v.push((mtime, e.path()));
    }
    v.sort_by_key(|a| std::cmp::Reverse(a.0));
    v.truncate(10);
    v.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_table() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("0.1", "0.1.1"));
        assert!(is_newer_version("0.1.1", "0.2"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
        assert!(!is_newer_version("1.10.0", "1.9.0"));
        // Pre-release < release of the same core.
        assert!(is_newer_version("0.2.0-rc1", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.2.0-rc1"));
        assert!(is_newer_version("0.2.0-rc1", "0.2.0-rc2"));
        assert!(!is_newer_version("0.2.0-rc2", "0.2.0-rc1"));
        // Garbage never nags.
        assert!(!is_newer_version("0.1.0", "latest"));
        assert!(!is_newer_version("abc", "0.2.0"));
        assert!(!is_newer_version("", "0.2.0"));
        assert!(!is_newer_version("0.1.0", ""));
    }

    #[test]
    fn version_url_must_be_http() {
        assert!(fetch_version_text("ftp://x/VERSION").is_err());
        assert!(fetch_version_text("").is_err());
    }

    #[test]
    fn crash_scan_finds_hook_files() {
        // Unique probe name so parallel runs / real crashes can't collide.
        let name = format!("asislog-crash-probe-{}.log", std::process::id());
        let p = std::env::temp_dir().join(&name);
        std::fs::write(&p, "probe").unwrap();
        let found = crash_reports();
        assert!(found.iter().any(|f| f == &p), "probe must be listed");
        std::fs::remove_file(&p).ok();
    }
}
