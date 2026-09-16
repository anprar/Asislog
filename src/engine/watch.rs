// English comments: native file watch (OS events) with polling fallback.
// Watches parent directories of open logs (rotation-safe: rename+create
// still lands in the same dir). Events only HASTEN the existing follow poll
// (fingerprint decision stays in `follow`); if notify fails or misses, the
// interval poll catches up. Never panics, never blocks the UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Relevant notify kinds: content/metadata writes, create/rename/remove.
/// Attribute-only noise (atime) is still forwarded — the follow poll is
/// cheap (size+mtime fast path) and dedups it.
fn event_is_relevant(ev: &notify::Event) -> bool {
    use notify::EventKind::*;
    match &ev.kind {
        Modify(_) | Create(_) | Remove(_) => true,
        Any | Other | Access(_) => false,
    }
}

/// True when an OS event path may belong to an open log file.
/// Strategy: canonical compare first (handles symlinks/8.3 paths), else
/// parent-dir + file-name compare (handles rotation: rename+create where
/// the event path is the NEW file with the same name).
pub fn watch_matches(open: &Path, event: &Path) -> bool {
    if open == event {
        return true;
    }
    // Canonical compare (best effort; files may vanish mid-rotation).
    if let (Ok(a), Ok(b)) = (open.canonicalize(), event.canonicalize()) {
        if a == b {
            return true;
        }
    }
    // Fallback: same parent dir + same file name.
    match (open.parent(), open.file_name(), event.parent(), event.file_name()) {
        (Some(ap), Some(an), Some(bp), Some(bn)) => an == bn && ap == bp,
        _ => false,
    }
}

/// Directory watcher for open log files (non-recursive, parents only).
pub struct DirWatch {
    watcher: notify::RecommendedWatcher,
    watched_dirs: HashSet<PathBuf>,
}

impl DirWatch {
    /// Spawn a watcher forwarding relevant event paths to `tx`.
    /// The callback runs on notify's thread: only cheap send, no I/O.
    pub fn spawn(tx: mpsc::Sender<PathBuf>) -> Result<Self, String> {
        use notify::{Config, RecommendedWatcher, Watcher};
        let mut watcher =
            RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| {
                    let Ok(ev) = res else { return };
                    if !event_is_relevant(&ev) {
                        return;
                    }
                    for p in ev.paths {
                        let _ = tx.send(p);
                    }
                },
                Config::default(),
            )
            .map_err(|e| format!("watch init: {}", e))?;
        // Reborrow for the initial state (no dirs yet).
        let _ = &mut watcher;
        Ok(Self { watcher, watched_dirs: HashSet::new() })
    }

    /// Ensure the parent dir of `file` is watched (best effort).
    pub fn watch_file(&mut self, file: &Path) {
        use notify::Watcher;
        let Some(dir) = file.parent() else { return };
        if self.watched_dirs.contains(dir) {
            return;
        }
        // Canonicalize the dir when possible so later comparisons are stable,
        // but always remember the literal too (deleted dirs still match).
        let key = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if self.watched_dirs.contains(&key) {
            return;
        }
        if self
            .watcher
            .watch(dir, notify::RecursiveMode::NonRecursive)
            .is_ok()
        {
            self.watched_dirs.insert(dir.to_path_buf());
            self.watched_dirs.insert(key);
        }
    }

    /// Drop dirs no open file needs anymore (keeps the watch set small).
    pub fn sync(&mut self, open_files: &[PathBuf]) {
        use notify::Watcher;
        let mut need: HashSet<PathBuf> = HashSet::new();
        for f in open_files {
            if let Some(d) = f.parent() {
                need.insert(d.to_path_buf());
                if let Ok(c) = d.canonicalize() {
                    need.insert(c);
                }
            }
        }
        let stale: Vec<PathBuf> = self
            .watched_dirs
            .iter()
            .filter(|d| !need.contains(*d))
            .cloned()
            .collect();
        for d in stale {
            let _ = self.watcher.unwatch(&d);
            self.watched_dirs.remove(&d);
        }
        for f in open_files {
            self.watch_file(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_matches() {
        let p = Path::new(if cfg!(windows) { "C:/x/a.log" } else { "/tmp/a.log" });
        assert!(watch_matches(p, p));
    }

    #[test]
    fn sibling_name_does_not_match() {
        let a = Path::new(if cfg!(windows) { "C:/x/a.log" } else { "/tmp/a.log" });
        let b = Path::new(if cfg!(windows) { "C:/x/b.log" } else { "/tmp/b.log" });
        assert!(!watch_matches(a, b));
    }

    #[test]
    fn parent_plus_name_fallback_matches_unresolved() {
        // Non-existent paths: canonicalize fails on both, fallback decides.
        let a = Path::new(if cfg!(windows) {
            "C:/definitely-missing-asislog/a.log"
        } else {
            "/definitely-missing-asislog/a.log"
        });
        let b = Path::new(if cfg!(windows) {
            "C:/definitely-missing-asislog/a.log"
        } else {
            "/definitely-missing-asislog/a.log"
        });
        assert!(watch_matches(a, b));
    }
}
