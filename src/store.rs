// English comments: global config (presets, highlight rules, theme).
// Stored as JSON outside the log directory so it survives file moves.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One saved search preset.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Preset {
    pub name: String,
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

/// One custom highlight rule (viewport only).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HighlightRule {
    pub name: String,
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Warna: red orange green blue purple yellow.
    pub color: String,
    /// True = whole line, false = match span only.
    pub whole_line: bool,
    pub enabled: bool,
}

impl HighlightRule {
    /// Validate pattern (regex must compile). Indonesian error, no panic.
    pub fn validate(&self) -> Result<(), String> {
        if self.pattern.is_empty() {
            return Err(String::from("Pola sorotan tidak boleh kosong."));
        }
        if self.regex {
            regex::Regex::new(&self.pattern)
                .map(|_| ())
                .map_err(|e| format!("Regex sorotan tidak valid: {}", e))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// Legacy: aturan lepas sebelum era set (dimigrasi ke `sets` saat load).
    #[serde(default)]
    pub highlights: Vec<HighlightRule>,
    /// Set highlighter bernama; yang aktif ditunjuk `active_set`.
    #[serde(default)]
    pub sets: Vec<HighlightSet>,
    #[serde(default)]
    pub active_set: Option<String>,
    /// Tema: system dark light light_contrast contrast dusk solar_dark solar_light monokai.
    #[serde(default)]
    pub tema: Option<String>,
    /// Riwayat file dibuka (path string, terbaru dulu, maks 12).
    #[serde(default)]
    pub recent: Vec<String>,
    /// File favorit (path string, disematkan di menu Riwayat).
    #[serde(default)]
    pub favorites: Vec<String>,
    /// History pola pencarian global (terbaru dulu, maks 30).
    #[serde(default)]
    pub history: Vec<HistEntry>,
    /// Zoom UI (1.0 = normal, 0.7-1.8).
    #[serde(default = "default_zoom")]
    pub zoom: f32,
    /// Isi scratchpad (dipotong 64 KB saat simpan).
    #[serde(default)]
    pub scratch: String,
}

fn default_zoom() -> f32 {
    1.0
}

/// Satu set highlighter bernama per produk; bisa diekspor/diimpor via file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HighlightSet {
    pub name: String,
    #[serde(default)]
    pub rules: Vec<HighlightRule>,
}

/// Migrasi config lama: aturan lepas -> set "Bawaan".
pub fn migrate_sets(cfg: &mut Config) {
    if cfg.sets.is_empty() && !cfg.highlights.is_empty() {
        cfg.sets.push(HighlightSet {
            name: "Bawaan".to_string(),
            rules: std::mem::take(&mut cfg.highlights),
        });
        cfg.active_set = Some("Bawaan".to_string());
    }
    if cfg.active_set.is_none() {
        cfg.active_set = cfg.sets.first().map(|s| s.name.clone());
    }
}

/// Workspace per produk: N log + filter + highlighter + rentang waktu.
/// Satu file JSON, cocok dishare via git (path boleh relatif).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceFile {
    pub path: String,
    #[serde(default)]
    pub top_line: u64,
    #[serde(default)]
    pub selected_line: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub files: Vec<WorkspaceFile>,
    /// Filter token (diabaikan bila `range` ada).
    #[serde(default)]
    pub filter: String,
    /// Rentang waktu (start, end) bila filter berasal dari sana.
    #[serde(default)]
    pub range: Option<(String, String)>,
    /// Salinan set highlighter aktif agar workspace mandiri.
    #[serde(default)]
    pub highlighter: Option<HighlightSet>,
}

impl Workspace {
    pub fn new(name: String) -> Self {
        Self {
            version: 1,
            name,
            files: Vec::new(),
            filter: String::new(),
            range: None,
            highlighter: None,
        }
    }
}

pub fn load_workspace(path: &std::path::Path) -> Result<Workspace, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Gagal membaca workspace: {}", e))?;
    serde_json::from_str(&text).map_err(|e| format!("Workspace tidak valid: {}", e))
}

pub fn save_workspace(path: &std::path::Path, ws: &Workspace) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(ws).map_err(|e| format!("Gagal menyusun workspace: {}", e))?;
    std::fs::write(path, text).map_err(|e| format!("Gagal menulis workspace: {}", e))
}

/// Catat path ke depan riwayat (dedupe, maks 12).
pub fn push_recent(recent: &mut Vec<String>, path: &str) {
    recent.retain(|p| p != path);
    recent.insert(0, path.to_string());
    recent.truncate(12);
}

/// One global search-history entry (query + modes).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistEntry {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

/// Push to front (dedupe on query+modes, cap 30).
pub fn push_history(hist: &mut Vec<HistEntry>, e: HistEntry) {
    hist.retain(|h| !(h.query == e.query && h.regex == e.regex && h.case_sensitive == e.case_sensitive));
    hist.insert(0, e);
    hist.truncate(30);
}

/// Subsequence fuzzy score (lower = better). None when not a subsequence.
/// Matches case-insensitively on ASCII.
pub fn fuzzy_score(pattern: &str, text: &str) -> Option<usize> {
    if pattern.is_empty() {
        return Some(0);
    }
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let mut pi = 0;
    let mut gaps = 0usize;
    let mut last = 0usize;
    let mut first = None::<usize>;
    for (ti, tc) in t.iter().enumerate() {
        if pi < p.len() && *tc == p[pi] {
            if first.is_none() {
                first = Some(ti);
            } else {
                gaps += ti - last - 1;
            }
            last = ti;
            pi += 1;
        }
    }
    if pi != p.len() {
        return None;
    }
    Some(first.unwrap_or(0) * 2 + gaps + t.len() / 64)
}

/// Up to `n` history entries fuzzy-matching `pattern`, best first.
pub fn suggest_history<'a>(hist: &'a [HistEntry], pattern: &str, n: usize) -> Vec<&'a HistEntry> {
    let mut scored: Vec<(usize, usize, &'a HistEntry)> = hist
        .iter()
        .enumerate()
        .filter_map(|(i, h)| fuzzy_score(pattern, &h.query).map(|s| (s, i, h)))
        .collect();
    scored.sort_by_key(|(s, i, _)| (*s, *i));
    scored.into_iter().take(n).map(|(_, _, h)| h).collect()
}

/// One restored tab in a saved session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionTab {
    pub path: String,
    pub top_line: u64,
    pub selected_line: u64,
    pub search_text: String,
    pub regex_on: bool,
    pub case_sensitive: bool,
    pub filter_text: String,
    pub follow: bool,
    /// Encoding override key (decode::Encoding::key), None = automatic.
    pub encoding: Option<String>,
    /// Search scope lines (lo, hi).
    #[serde(default)]
    pub scope: Option<(u64, u64)>,
}

/// Full workspace session (tabs + theme + active tab).
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Session {
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    #[serde(default)]
    pub current: usize,
    #[serde(default)]
    pub tema: Option<String>,
}

pub fn session_path() -> Option<PathBuf> {
    config_path().map(|p| {
        p.parent()
            .map(|d| d.join("session.json"))
            .unwrap_or(PathBuf::from("session.json"))
    })
}

pub fn load_session() -> Session {
    let Some(p) = session_path() else {
        return Session::default();
    };
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_session(s: &Session) -> Result<(), String> {
    let Some(p) = session_path() else {
        return Err(String::from("Direktori config tidak ditemukan."));
    };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Gagal membuat direktori config: {}", e))?;
    }
    let text =
        serde_json::to_string_pretty(s).map_err(|e| format!("Gagal menyusun sesi: {}", e))?;
    std::fs::write(&p, text).map_err(|e| format!("Gagal menulis sesi: {}", e))
}

/// Builtin search presets (Indonesian names).
pub fn builtin_presets() -> Vec<Preset> {
    vec![
        Preset { name: "Java: ERROR".into(), query: "ERROR".into(), regex: false, case_sensitive: false },
        Preset { name: "Java: FATAL".into(), query: "FATAL".into(), regex: false, case_sensitive: false },
        Preset { name: "Java: Exception & cause".into(), query: "Exception|Caused by:".into(), regex: true, case_sensitive: false },
        Preset { name: "Java: OutOfMemory".into(), query: "OutOfMemoryError".into(), regex: false, case_sensitive: false },
        Preset { name: "Java: Connection refused".into(), query: "Connection refused".into(), regex: false, case_sensitive: false },
        Preset { name: "Java: timeout".into(), query: "timeout".into(), regex: false, case_sensitive: false },
        Preset { name: "SQL: Checkpoint".into(), query: "CHECKPOINT".into(), regex: false, case_sensitive: false },
        Preset { name: "SQL: Transaksi".into(), query: "TRANSACTION".into(), regex: false, case_sensitive: false },
        Preset { name: "SQL: Rollback".into(), query: "ROLLBACK".into(), regex: false, case_sensitive: false },
        Preset { name: "SQL: Commit".into(), query: "COMMIT".into(), regex: false, case_sensitive: false },
        Preset { name: "SQL: INSERT INTO".into(), query: "INSERT INTO".into(), regex: false, case_sensitive: false },
    ]
}

/// Highlight color keys with (dark, light) RGB.
pub fn highlight_palette(key: &str) -> ((u8, u8, u8), (u8, u8, u8)) {
    match key {
        "red" => ((255, 110, 110), (180, 30, 30)),
        "orange" => ((255, 180, 90), (170, 100, 0)),
        "green" => ((120, 220, 130), (20, 130, 40)),
        "blue" => ((120, 180, 255), (30, 90, 180)),
        "purple" => ((200, 150, 255), (110, 60, 170)),
        "teal" => ((90, 220, 210), (0, 130, 130)),
        "pink" => ((255, 150, 200), (180, 40, 120)),
        "brown" => ((205, 155, 105), (120, 70, 20)),
        _ => ((255, 220, 100), (150, 110, 0)), // yellow
    }
}

pub fn highlight_color_names() -> &'static [(&'static str, &'static str)] {
    &[
        ("red", "Merah"),
        ("orange", "Oranye"),
        ("green", "Hijau"),
        ("blue", "Biru"),
        ("purple", "Ungu"),
        ("yellow", "Kuning"),
        ("teal", "Teal"),
        ("pink", "Pink"),
        ("brown", "Cokelat"),
    ]
}

/// Sembilan warna label cepat (tombol 1-9).
pub const LABEL_COLORS: [&str; 9] =
    ["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "brown"];

/// Global config path: %APPDATA%/AsisLog/config.json (Windows)
/// or ~/.config/asislog/config.json (other).
pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|a| PathBuf::from(a).join("AsisLog").join("config.json"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config").join("asislog").join("config.json"))
    }
}

pub fn load() -> Config {
    let Some(p) = config_path() else {
        return Config::default();
    };
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => return Config::default(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let Some(p) = config_path() else {
        return Err(String::from("Direktori config tidak ditemukan."));
    };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Gagal membuat direktori config: {}", e))?;
    }
    let text =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("Gagal menyusun config: {}", e))?;
    std::fs::write(&p, text).map_err(|e| format!("Gagal menulis config: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_have_queries() {
        let b = builtin_presets();
        assert!(b.len() >= 10);
        assert!(b.iter().all(|p| !p.name.is_empty() && !p.query.is_empty()));
    }

    #[test]
    fn rule_validation() {
        let ok = HighlightRule {
            name: "x".into(),
            pattern: "ROLLBACK".into(),
            regex: false,
            case_sensitive: false,
            color: "red".into(),
            whole_line: false,
            enabled: true,
        };
        assert!(ok.validate().is_ok());
        let bad = HighlightRule { pattern: "([a".into(), regex: true, ..ok.clone() };
        assert!(bad.validate().is_err());
        let empty = HighlightRule { pattern: "".into(), ..ok };
        assert!(empty.validate().is_err());
    }

    #[test]
    fn config_roundtrip_json() {
        let cfg = Config {
            presets: vec![Preset { name: "a".into(), query: "b".into(), regex: true, case_sensitive: false }],
            highlights: vec![],
            sets: vec![],
            active_set: None,
            tema: Some("dark".into()),
            recent: vec!["c:/x.log".into()],
            favorites: vec!["c:/fav.log".into()],
            history: vec![HistEntry { query: "q".into(), regex: false, case_sensitive: false }],
            zoom: 1.2,
            scratch: String::new(),
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.presets, cfg.presets);
        assert_eq!(back.tema, cfg.tema);
        assert_eq!(back.recent, cfg.recent);
        assert_eq!(back.history, cfg.history);
        assert_eq!(back.favorites, cfg.favorites);
        assert_eq!(back.zoom, 1.2);
    }

    #[test]
    fn recent_dedupe_cap() {
        let mut r = Vec::new();
        for i in 0..15 {
            push_recent(&mut r, &format!("f{}.log", i));
        }
        assert_eq!(r.len(), 12);
        assert_eq!(r[0], "f14.log");
        push_recent(&mut r, "f10.log");
        assert_eq!(r[0], "f10.log");
        assert_eq!(r.len(), 12);
    }

    #[test]
    fn history_dedupe_and_suggest() {
        let mut h = Vec::new();
        push_history(&mut h, HistEntry { query: "ERROR".into(), regex: false, case_sensitive: false });
        push_history(&mut h, HistEntry { query: "Exception".into(), regex: false, case_sensitive: false });
        push_history(&mut h, HistEntry { query: "ERROR".into(), regex: false, case_sensitive: false });
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].query, "ERROR");
        let s = suggest_history(&h, "err", 6);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].query, "ERROR");
        // Prefix beats scattered subsequence.
        push_history(&mut h, HistEntry { query: "xExxxRxxxRxxx".into(), regex: false, case_sensitive: false });
        let s = suggest_history(&h, "err", 6);
        assert_eq!(s[0].query, "ERROR");
        assert!(suggest_history(&h, "zzz", 6).is_empty());
    }

    #[test]
    fn session_roundtrip() {        let s = Session {
            tabs: vec![SessionTab {
                path: "c:/a.log".into(),
                top_line: 10,
                selected_line: 12,
                search_text: "ERROR".into(),
                regex_on: false,
                case_sensitive: false,
            filter_text: String::new(),
            follow: true,
            encoding: Some("utf8".into()),
            scope: None,
            }],
            current: 0,
            tema: Some("dark".into()),
        };
        let text = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn migrate_legacy_highlights() {
        let mut cfg = Config {
            highlights: vec![HighlightRule {
                name: "r".into(),
                pattern: "x".into(),
                regex: false,
                case_sensitive: false,
                color: "red".into(),
                whole_line: false,
                enabled: true,
            }],
            ..Config::default()
        };
        migrate_sets(&mut cfg);
        assert_eq!(cfg.sets.len(), 1);
        assert_eq!(cfg.sets[0].name, "Bawaan");
        assert_eq!(cfg.active_set.as_deref(), Some("Bawaan"));
        assert!(cfg.highlights.is_empty());
    }

    #[test]
    fn workspace_roundtrip() {
        let mut ws = Workspace::new("ERP malam".to_string());
        ws.files.push(WorkspaceFile {
            path: "logs/a.log".to_string(),
            top_line: 100,
            selected_line: 120,
        });
        ws.filter = "ERROR -DEBUG".to_string();
        ws.range = Some(("2026-08-24 13:00:00".to_string(), "2026-08-24 14:00:00".to_string()));
        ws.highlighter = Some(HighlightSet {
            name: "SQL".to_string(),
            rules: vec![HighlightRule {
                name: "rb".into(),
                pattern: "ROLLBACK".into(),
                regex: false,
                case_sensitive: false,
                color: "red".into(),
                whole_line: false,
                enabled: true,
            }],
        });
        let text = serde_json::to_string(&ws).unwrap();
        let back: Workspace = serde_json::from_str(&text).unwrap();
        assert_eq!(back, ws);
    }
}
