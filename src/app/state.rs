// English comments: AsisLogApp state: config, session, workspace, open-file (split from app.rs; behavior unchanged).
#![allow(unused_imports)]
// English comments: AsisLog egui app (tabs, shortcuts, background jobs).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::engine::decode::Encoding;
use crate::engine::filter::{parse_filter, ParsedFilter};
use crate::engine::follow::{check_follow, load_identity, FollowEvent};
use crate::engine::index::{self, SparseIndex};
use crate::engine::search::{self, CacheKey, FileRev, Hit, SearchCache};
use crate::engine::{format_count, format_size, BlockKind, BookmarkColor, Doc};
use crate::store::{HighlightRule, HighlightSet, HistEntry, Preset};
use crate::ui::{
    dialogs::parse_goto,
    dialogs::GotoTarget,
    icons::{icon_button, Icon},
    results::result_row,
    theme::Tema,
    viewer::{self, CompiledRule},
};
use super::*;


// ---------- app ----------

pub struct AsisLogApp {
    pub(crate) tabs: Vec<TabState>,
    pub(crate) current: usize,
    pub(crate) global_error: Option<String>,
    pub(crate) global_status: String,
    pub(crate) tema: Tema,
    /// (Pilihan, dark) yang sedang diterapkan ke egui.
    pub(crate) tema_state: (Tema, bool),
    /// Preset pencarian milik user (bawaan dari store::builtin_presets).
    pub(crate) presets: Vec<Preset>,
    /// Set highlighter bernama; aturan efektif = set aktif.
    pub(crate) sets: Vec<HighlightSet>,
    pub(crate) active_set: Option<String>,
    /// Aturan terkompilasi untuk render viewport (dibangun ulang bila kotor).
    pub(crate) hl_compiled: Vec<CompiledRule>,
    pub(crate) hl_dirty: bool,
    // Dialog simpan preset.
    pub(crate) preset_save_open: bool,
    pub(crate) preset_save_name: String,
    /// True bila config global perlu ditulis (di-flush di luar pinjam tab).
    pub(crate) cfg_dirty: bool,
    // Jendela aturan sorotan + form tambah.
    pub(crate) hl_open: bool,
    pub(crate) hl_name: String,
    pub(crate) hl_set_name: String,
    pub(crate) hl_pattern: String,
    pub(crate) hl_regex: bool,
    pub(crate) hl_case: bool,
    pub(crate) hl_color_idx: usize,
    pub(crate) hl_whole: bool,
    // Dialog ubah label penanda.
    pub(crate) rename_open: bool,
    pub(crate) rename_line: u64,
    pub(crate) rename_label: String,
    pub(crate) rename_color: BookmarkColor,
    // Dialog rentang waktu.
    pub(crate) range_open: bool,
    pub(crate) range_start: String,
    pub(crate) range_end: String,
    pub(crate) range_msg: String,
    /// Riwayat file yang pernah dibuka (path string, terbaru dulu).
    pub(crate) recent: Vec<String>,
    /// File favorit (disematkan di menu Riwayat).
    pub(crate) favorites: Vec<String>,
    /// Jendela daftar pintasan (F1).
    pub(crate) shortcuts_open: bool,
    /// History pola pencarian global (config, maks 30).
    pub(crate) history: Vec<HistEntry>,
    /// True bila sesi perlu ditulis (debounce 10 dtk).
    pub(crate) session_dirty: bool,
    pub(crate) last_session_save: Instant,
    /// Zoom UI (1.0 normal). Diterapkan ke text style + tinggi baris.
    pub(crate) zoom: f32,
    /// Jendela scratchpad + isi + pesan.
    pub(crate) scratch_open: bool,
    pub(crate) scratch_text: String,
    pub(crate) scratch_msg: String,
    /// Dialog buka URL + unduhan berjalan.
    pub(crate) url_open: bool,
    pub(crate) url_text: String,
    pub(crate) dl_rx: Option<mpsc::Receiver<DlMsg>>,
    /// Dialog tempel teks.
    pub(crate) paste_open: bool,
    pub(crate) paste_text: String,
    /// Goto dialog: terapkan ke semua tab.
    pub(crate) goto_all: bool,
}

impl AsisLogApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Config global (preset, sorotan, tema, riwayat); abaikan bila belum ada.
        let mut cfg = crate::store::load();
        crate::store::migrate_sets(&mut cfg);
        let tema = cfg
            .tema
            .as_deref()
            .map(Tema::from_key)
            .unwrap_or_default();
        let zoom = cfg.zoom.clamp(0.7, 1.8);
        if zoom.is_finite() {
            Self::apply_zoom(&cc.egui_ctx, zoom);
        } else {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
        }
        let mut app = Self {
            tabs: Vec::new(),
            current: 0,
            global_error: None,
            global_status: String::from("Siap. Buka file log untuk mulai."),
            tema,
            // Paksa terapkan sekali pada frame pertama (termasuk baca tema OS).
            tema_state: (Tema::Gelap, true),
            presets: cfg.presets,
            sets: cfg.sets,
            active_set: cfg.active_set,
            hl_compiled: Vec::new(),
            hl_dirty: true,
            preset_save_open: false,
            preset_save_name: String::new(),
            cfg_dirty: false,
            hl_open: false,
            hl_name: String::new(),
            hl_set_name: String::new(),
            hl_pattern: String::new(),
            hl_regex: false,
            hl_case: false,
            hl_color_idx: 0,
            hl_whole: false,
            rename_open: false,
            rename_line: 1,
            rename_label: String::new(),
            rename_color: BookmarkColor::Default,
            range_open: false,
            range_start: String::new(),
            range_end: String::new(),
            range_msg: String::new(),
            recent: cfg.recent,
            favorites: cfg.favorites,
            history: cfg.history,
            session_dirty: false,
            last_session_save: Instant::now(),
            shortcuts_open: false,
            zoom,
            scratch_open: false,
            scratch_text: cfg.scratch.clone(),
            scratch_msg: String::new(),
            url_open: false,
            url_text: String::new(),
            dl_rx: None,
            paste_open: false,
            paste_text: String::new(),
            goto_all: false,
        };
        app.rebuild_highlights();
        app.restore_session();
        app
    }

    /// Terapkan zoom ke text style egui (visuals milik tema, tak disentuh).
    pub(crate) fn apply_zoom(ctx: &egui::Context, zoom: f32) {
        let z = zoom.clamp(0.7, 1.8);
        let mut style = (*ctx.style()).clone();
        let px = |base: f32| base * z;
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(px(14.0), egui::FontFamily::Monospace),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(px(14.0), egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(px(14.0), egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(px(11.0), egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(px(18.0), egui::FontFamily::Proportional),
        );
        ctx.set_style(style);
    }

    /// Tinggi baris viewport mengikuti zoom.
    pub(crate) fn row_h(&self) -> f32 {
        (viewer::ROW_H * self.zoom).round().max(14.0)
    }

    pub(crate) fn bump_zoom(&mut self, ctx: &egui::Context, next: f32) {
        self.zoom = next.clamp(0.7, 1.8);
        Self::apply_zoom(ctx, self.zoom);
        self.cfg_dirty = true;
        self.global_status = format!("Zoom {}%.", (self.zoom * 100.0).round() as u32);
    }

    /// Tulis config global. Galat di status; scratch dipotong 64 KB.
    pub(crate) fn save_config(&mut self) {
        let mut scratch = self.scratch_text.clone();
        if scratch.len() > 65536 {
            scratch.truncate(65536);
        }
        let cfg = crate::store::Config {
            presets: self.presets.clone(),
            highlights: Vec::new(),
            sets: self.sets.clone(),
            active_set: self.active_set.clone(),
            tema: Some(self.tema.key().to_string()),
            recent: self.recent.clone(),
            favorites: self.favorites.clone(),
            history: self.history.clone(),
            zoom: self.zoom,
            scratch,
        };
        if let Err(e) = crate::store::save(&cfg) {
            self.global_status = e;
        }
        self.cfg_dirty = false;
    }

    /// Aturan efektif = aturan enabled milik set aktif.
    pub(crate) fn active_rules(&self) -> Vec<HighlightRule> {
        self.active_set
            .as_ref()
            .and_then(|n| self.sets.iter().find(|s| &s.name == n))
            .map(|s| s.rules.clone())
            .unwrap_or_default()
    }

    /// Tulis sesi workspace (tab + posisi + filter + follow).
    pub(crate) fn save_session(&mut self) {
        use crate::store::{Session, SessionTab};
        let tabs = self
            .tabs
            .iter()
            .map(|t| SessionTab {
                path: t.doc.path.display().to_string(),
                top_line: t.row_to_line(t.top_row).unwrap_or(1),
                selected_line: t.selected_line,
                search_text: t.search_text.clone(),
                regex_on: t.regex_on,
                case_sensitive: t.case_sensitive,
                filter_text: t.filter_text.clone(),
                follow: t.doc.follow,
                encoding: t.doc.encoding_override.map(|e| e.key().to_string()),
                scope: t.scope,
            })
            .collect();
        let s = Session { tabs, current: self.current, tema: Some(self.tema.key().to_string()) };
        if let Err(e) = crate::store::save_session(&s) {
            self.global_status = e;
        }
        self.session_dirty = false;
        self.last_session_save = Instant::now();
    }

    /// Simpan workspace produk: tab + filter/range tab aktif + set sorotan aktif.
    pub(crate) fn save_workspace_to(&mut self, path: &std::path::Path) {
        use crate::store::{Workspace, WorkspaceFile};
        if self.tabs.is_empty() {
            self.global_status = String::from("Tidak ada tab untuk disimpan.");
            return;
        }
        let cur = &self.tabs[self.current.min(self.tabs.len() - 1)];
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("Workspace"));
        let ws = Workspace {
            version: 1,
            name,
            files: self
                .tabs
                .iter()
                .map(|t| WorkspaceFile {
                    path: t.doc.path.display().to_string(),
                    top_line: t.row_to_line(t.top_row).unwrap_or(1),
                    selected_line: t.selected_line,
                })
                .collect(),
            filter: cur.filter_text.clone(),
            range: cur.range_applied.clone(),
            highlighter: self
                .active_set
                .as_ref()
                .and_then(|n| self.sets.iter().find(|s| &s.name == n))
                .cloned(),
        };
        match crate::store::save_workspace(path, &ws) {
            Ok(()) => {
                self.global_status =
                    format!("Workspace disimpan ke {}.", path.display());
            }
            Err(e) => self.global_status = e,
        }
    }

    /// Buka workspace: N log + filter/range bersama + set sorotan.
    /// File hilang dilewati dengan catatan; set di-upsert lalu diaktifkan.
    pub(crate) fn open_workspace(&mut self, ws: crate::store::Workspace) {
        let base = self.tabs.len();
        let mut opened = 0;
        let mut missing = 0;
        for f in &ws.files {
            let p = PathBuf::from(&f.path);
            if !p.exists() {
                missing += 1;
                continue;
            }
            match Doc::open(p.clone()) {
                Ok(mut doc) => {
                    let (marks, warn) = crate::engine::marks::load(&doc.path);
                    if !marks.is_empty() {
                        doc.bookmarks = marks;
                    }
                    if let Some(w) = warn {
                        doc.status = w;
                    }
                    let mut tab = TabState::new(doc);
                    tab.selected_line = f.selected_line.max(1);
                    tab.top_row = f.top_line.saturating_sub(1);
                    tab.saved_top = tab.top_row;
                    self.tabs.push(tab);
                    crate::store::push_recent(
                        &mut self.recent,
                        &p.display().to_string(),
                    );
                    opened += 1;
                }
                Err(_) => missing += 1,
            }
        }
        if let Some(set) = ws.highlighter {
            let name = set.name.clone();
            if !self.sets.iter().any(|s| s.name == name) {
                self.sets.push(set);
            }
            self.active_set = Some(name);
            self.hl_dirty = true;
            self.cfg_dirty = true;
        }
        let range = ws.range.clone();
        let filter = ws.filter.clone();
        for t in &mut self.tabs[base..] {
            if let Some((a, b)) = range.clone() {
                match apply_time_range(t, &a, &b) {
                    Ok(msg) => {
                        t.doc.status = msg;
                        t.range_applied = Some((a, b));
                    }
                    Err(e) => t.doc.status = e,
                }
            } else if !filter.trim().is_empty() {
                t.filter_text = filter.clone();
                let q = filter.clone();
                t.start_filter(q);
            }
        }
        if opened > 0 {
            self.current = base;
        }
        self.global_status = format!(
            "Workspace '{}': {} dibuka{}.",
            ws.name,
            opened,
            if missing > 0 {
                format!(", {} file hilang, dilewati", missing)
            } else {
                String::new()
            }
        );
        self.session_dirty = true;
    }

    /// Pulihkan sesi saat start: buka ulang tab yang filenya masih ada.
    pub(crate) fn restore_session(&mut self) {        use crate::engine::decode::Encoding;
        let sess = crate::store::load_session();
        if sess.tabs.is_empty() {
            return;
        }
        let mut opened = 0;
        let mut missing = 0;
        for st in sess.tabs {
            let p = PathBuf::from(&st.path);
            if !p.exists() {
                missing += 1;
                continue;
            }
            match Doc::open(p) {
                Ok(mut doc) => {
                    let (marks, warn) = crate::engine::marks::load(&doc.path);
                    if !marks.is_empty() {
                        doc.bookmarks = marks;
                    }
                    if let Some(w) = warn {
                        doc.status = w;
                    }
                    if let Some(key) = st.encoding.as_deref() {
                        if let Some(enc) = Encoding::from_key(key) {
                            doc.set_encoding_override(Some(enc));
                        }
                    }
                    let mut tab = TabState::new(doc);
                    tab.search_text = st.search_text;
                    tab.regex_on = st.regex_on;
                    tab.case_sensitive = st.case_sensitive;
                    tab.filter_text = st.filter_text.clone();
                    tab.doc.follow = st.follow;
                    tab.doc.stick_bottom = st.follow;
                    tab.selected_line = st.selected_line.max(1);
                    tab.top_row = st.top_line.saturating_sub(1);
                    tab.saved_top = tab.top_row;
                    tab.scope = st.scope;
                    if let Some((a, b)) = st.scope {
                        tab.scope_a = a.to_string();
                        tab.scope_b = b.to_string();
                    }
                    if !tab.search_text.trim().is_empty() {
                        tab.debounce_at = Some(Instant::now() + Duration::from_millis(400));
                    }
                    if !st.filter_text.trim().is_empty() {
                        let q = st.filter_text.clone();
                        tab.start_filter(q);
                    }
                    self.tabs.push(tab);
                    opened += 1;
                }
                Err(_) => missing += 1,
            }
        }
        if !self.tabs.is_empty() {
            self.current = sess.current.min(self.tabs.len() - 1);
        }
        if opened > 0 {
            self.global_status = format!(
                "Sesi dipulihkan: {} tab{}.",
                opened,
                if missing > 0 {
                    format!(", {} file tak ditemukan, dilewati", missing)
                } else {
                    String::new()
                }
            );
        }
    }

    /// Toggle label warna cepat (tombol 1-9) dari query aktif di tab kini.
    /// Sama query + tombol sama = hapus lagi. Butuh set aktif (dibuat bila kosong).
    pub(crate) fn toggle_label(&mut self, idx: usize) {
        const COLORS: [&str; 9] = crate::store::LABEL_COLORS;
        let cur = self.current;
        let (q, cs) = match self.tabs.get(cur) {
            Some(t) => (t.search_text.trim().to_string(), t.case_sensitive),
            None => return,
        };
        if q.is_empty() {
            if let Some(t) = self.tabs.get_mut(cur) {
                t.doc.status =
                    String::from("Ketik query dulu, lalu tekan 1-9 untuk label warna.");
            }
            return;
        }
        if self.active_set.is_none() {
            self.sets.push(HighlightSet { name: "Cepat".to_string(), rules: Vec::new() });
            self.active_set = Some("Cepat".to_string());
        }
        let short: String = q.chars().take(40).collect();
        let name = format!("Kunci {}: {}", idx + 1, short);
        let aname = self.active_set.clone().unwrap_or_default();
        let mut msg = String::new();
        if let Some(s) = self.sets.iter_mut().find(|s| s.name == aname) {
            if let Some(pos) = s.rules.iter().position(|r| r.name == name) {
                s.rules.remove(pos);
                msg = format!("Label {} dihapus.", idx + 1);
            } else if s.rules.len() >= 50 {
                msg = String::from("Set penuh (50 aturan). Hapus dulu yang tak perlu.");
            } else {
                let cname = crate::store::highlight_color_names()
                    .iter()
                    .find(|(k, _)| *k == COLORS[idx])
                    .map(|(_, n)| *n)
                    .unwrap_or("?");
                s.rules.push(HighlightRule {
                    name: name.clone(),
                    pattern: q,
                    regex: false,
                    case_sensitive: cs,
                    color: COLORS[idx].to_string(),
                    whole_line: false,
                    enabled: true,
                });
                msg = format!("Label {}: \"{}\" ({}). Tekan lagi untuk hapus.", idx + 1, short, cname);
            }
            self.hl_dirty = true;
            self.save_config();
        }
        if let Some(t) = self.tabs.get_mut(cur) {
            t.doc.status = msg;
        }
    }

    /// Bangun ulang cache aturan terkompilasi.
    pub(crate) fn rebuild_highlights(&mut self) {        self.hl_compiled = self
            .active_rules()
            .iter()
            .filter(|r| r.enabled)
            .filter_map(|r| {
                CompiledRule::compile(&r.pattern, r.regex, r.case_sensitive, &r.color, r.whole_line)
            })
            .collect();
        self.hl_dirty = false;
    }

    /// Simpan penanda tab aktif ke sidecar. Galat tampil di status tab.
    pub(crate) fn save_marks(idx: usize, tabs: &mut [TabState]) {
        let Some(tab) = tabs.get_mut(idx) else { return };
        match crate::engine::marks::save(&tab.doc.path, &tab.doc.bookmarks) {
            Ok(()) => {}
            Err(e) => tab.doc.status = e,
        }
    }

    /// Open files passed on the command line (public for the binary crate).
    /// Missing files become a status message, never a panic or dialog.
    pub fn open_files(&mut self, files: Vec<PathBuf>) {
        for p in files {
            if p.exists() {
                self.open_file(p);
            } else {
                self.global_status = format!("File tidak ditemukan: {}", p.display());
            }
        }
    }

    pub(crate) fn open_file(&mut self, path: PathBuf) {
        // Arsip (zip/tar/gz): ekstrak entri teks terbaik ke temp dulu.
        let opened = match crate::engine::archive::open_maybe_archive(&path) {
            Ok(o) => o,
            Err(e) => {
                self.global_error = Some(e);
                return;
            }
        };
        let note = opened.note.clone();
        let temp = opened.temp.clone();
        match Doc::open(opened.path) {
            Ok(mut doc) => {
                // Muat penanda persisten (sidecar JSON + validasi sidik).
                let (marks, warn) = crate::engine::marks::load(&path);
                if !marks.is_empty() {
                    doc.bookmarks = marks;
                }
                if let Some(w) = warn {
                    doc.status = w;
                }
                if !note.is_empty() {
                    doc.status = note;
                }
                let mut tab = TabState::new(doc);
                tab.temp_path = temp;
                self.tabs.push(tab);
                self.current = self.tabs.len() - 1;
                self.global_status = format!("Membuka {}.", path.display());
                // Catat ke riwayat file (tetap, tersimpan di config).
                crate::store::push_recent(&mut self.recent, &path.display().to_string());
                self.save_config();
                self.session_dirty = true;
            }
            Err(e) => {
                self.global_error = Some(e);
            }
        }
    }

    pub(crate) fn open_dialog(&mut self) {
        let f = rfd::FileDialog::new()
            .add_filter("Log", &["log", "txt", "out", "err"])
            .add_filter("Arsip", &["zip", "tgz", "gz", "tar"])
            .add_filter("Semua", &["*"])
            .pick_file();
        if let Some(p) = f {
            self.open_file(p);
        }
    }

    pub(crate) fn current_tab_mut(&mut self) -> Option<&mut TabState> {
        self.tabs.get_mut(self.current)
    }
}
