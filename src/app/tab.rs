// English comments: ViewMode + TabState (one tab: search/filter/follow/nav state) (split from app.rs; behavior unchanged).
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


/// Mode tampil viewport: semua baris, hanya hasil, atau hanya penanda.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    All,
    Hits,
    Marks,
}

impl ViewMode {
    pub fn nama(self) -> &'static str {
        match self {
            ViewMode::All => "Semua",
            ViewMode::Hits => "Hasil",
            ViewMode::Marks => "Penanda",
        }
    }

    pub fn semua() -> &'static [ViewMode] {
        &[ViewMode::All, ViewMode::Hits, ViewMode::Marks]
    }
}

// ---------- one tab ----------

pub(crate) struct TabState {
    pub(crate) doc: Doc,
    pub(crate) search_text: String,
    pub(crate) last_searched: String,
    pub(crate) debounce_at: Option<Instant>,
    pub(crate) case_sensitive: bool,
    pub(crate) regex_on: bool,
    pub(crate) current_hit: Option<usize>,
    /// Progress pindaian pencarian (diperbarui per batch).
    pub(crate) search_scanned: u64,
    pub(crate) search_total: u64,
    /// Cache hasil lengkap per query+revisi file (pola ulang = instan).
    pub(crate) search_cache: SearchCache,
    /// Kunci cache untuk pencarian yang sedang berjalan.
    pub(crate) pending_key: Option<CacheKey>,
    pub(crate) filter_text: String,
    pub(crate) top_row: u64,
    pub(crate) selected_line: u64,
    pub(crate) hover_line: Option<u64>,
    /// File temp hasil ekstrak arsip (dihapus saat tab ditutup).
    pub(crate) temp_path: Option<PathBuf>,
    /// Mode tampil viewport + peta barisnya.
    pub(crate) view_mode: ViewMode,
    pub(crate) mode_lines: Vec<u64>,
    /// Cakupan pencarian (baris lo..=hi) + teks dialog + buka dialog.
    pub(crate) scope: Option<(u64, u64)>,
    pub(crate) scope_a: String,
    pub(crate) scope_b: String,
    pub(crate) scope_open: bool,
    pub(crate) scope_msg: String,
    pub(crate) show_bookmarks: bool,
    /// Saringan ketik di panel penanda.
    pub(crate) mark_query: String,
    /// Rentang waktu yang sedang diterapkan sebagai filter (start, end).
    pub(crate) range_applied: Option<(String, String)>,
    /// Panel hasil diciutkan (header saja) agar viewport log lega.
    pub(crate) results_collapsed: bool,
    pub(crate) goto_open: bool,
    pub(crate) goto_input: String,
    pub(crate) goto_msg: String,
    pub(crate) export_open: bool,
    pub(crate) export_context: usize,
    pub(crate) index_rx: Option<mpsc::Receiver<IndexUpdate>>,
    pub(crate) search_rx: Option<mpsc::Receiver<SearchBatchMsg>>,
    pub(crate) filter_rx: Option<mpsc::Receiver<(Vec<u64>, bool)>>,
    /// Peta bucket ERROR/WARN (512 byte) + ukuran file saat dipindai.
    pub(crate) marker_bits: Option<Vec<u8>>,
    pub(crate) marker_size: u64,
    pub(crate) marker_rx: Option<mpsc::Receiver<(Vec<u8>, u64, Option<TimeHist>)>>,
    /// Histogram ERROR per menit (shading strip).
    pub(crate) time_hist: Option<TimeHist>,
    /// Catatan follow segar, mis. `+128 baris baru` (+ waktu).
    pub(crate) follow_note: Option<(String, Instant)>,
    /// Jumlah baris terlihat terakhir (untuk posisi lompat 40% viewport).
    pub(crate) last_visible: u64,
    /// History navigasi (nomor baris) + posisi kini. Maks 200.
    pub(crate) hist: Vec<u64>,
    pub(crate) hist_pos: usize,
    /// True bila penanda berubah dan sidecar perlu ditulis ulang.
    pub(crate) marks_dirty: bool,
    /// top_row terakhir yang tercatat untuk sesi (hemat tulis).
    pub(crate) saved_top: u64,
    pub(crate) gen_shared: Arc<AtomicU64>,
    pub(crate) index_cancel: Arc<AtomicBool>,
    pub(crate) search_cancel: Arc<AtomicBool>,
    pub(crate) last_follow_poll: Instant,
    /// Sidik head terakhir untuk follow (None = belum diketahui).
    pub(crate) follow_fp: Option<String>,
    pub(crate) pending_search_error: Option<String>,
}

impl TabState {
    pub(crate) fn new(doc: Doc) -> Self {
        let gen_shared = Arc::new(AtomicU64::new(0));
        let index_cancel = Arc::new(AtomicBool::new(false));
        let (index_tx, index_rx) = mpsc::channel();
        spawn_indexer(
            doc.path.clone(),
            doc.encoding(),
            doc.bom_len,
            doc.size,
            index_tx,
            index_cancel.clone(),
        );
        Self {
            doc,
            search_text: String::new(),
            last_searched: String::new(),
            debounce_at: None,
            case_sensitive: false,
            regex_on: false,
            current_hit: None,
            search_scanned: 0,
            search_total: 0,
            search_cache: SearchCache::new(8),
            pending_key: None,
            filter_text: String::new(),
            top_row: 0,
            selected_line: 1,
            hover_line: None,
            temp_path: None,
            view_mode: ViewMode::All,
            mode_lines: Vec::new(),
            scope: None,
            scope_a: String::new(),
            scope_b: String::new(),
            scope_open: false,
            scope_msg: String::new(),
            show_bookmarks: false,
            mark_query: String::new(),
            range_applied: None,
            results_collapsed: true,
            goto_open: false,
            goto_input: String::new(),
            goto_msg: String::new(),
            export_open: false,
            export_context: 10,
            index_rx: Some(index_rx),
            search_rx: None,
            filter_rx: None,
            marker_bits: None,
            marker_size: 0,
            marker_rx: None,
            time_hist: None,
            follow_note: None,
            last_visible: 30,
            hist: Vec::new(),
            hist_pos: 0,
            marks_dirty: false,
            saved_top: 0,
            gen_shared,
            index_cancel,
            search_cancel: Arc::new(AtomicBool::new(false)),
            last_follow_poll: Instant::now(),
            follow_fp: None,
            pending_search_error: None,
        }
    }

    pub(crate) fn total_view_rows(&self) -> u64 {
        if self.view_mode != ViewMode::All {
            self.mode_lines.len().max(1) as u64
        } else {
            self.doc.view_row_count().max(1)
        }
    }

    /// View-row -> nomor baris asli (hormati mode tampil dulu, lalu filter).
    pub(crate) fn row_to_line(&self, row: u64) -> Option<u64> {
        if self.view_mode != ViewMode::All {
            self.mode_lines.get(row as usize).copied()
        } else {
            self.doc.view_row_to_line(row)
        }
    }

    /// Bangun ulang peta mode tampil dari hits/penanda kini.
    pub(crate) fn refresh_mode_map(&mut self) {
        self.mode_lines = match self.view_mode {
            ViewMode::All => Vec::new(),
            ViewMode::Hits => self.doc.hits.iter().map(|h| h.line).collect(),
            ViewMode::Marks => self.doc.bookmarks.iter().map(|b| b.line).collect(),
        };
        if self.view_mode != ViewMode::All {
            self.top_row = 0;
        }
    }

    /// Revisi file untuk kunci cache (ukuran + mtime).
    pub(crate) fn file_rev(&self) -> FileRev {
        let (s, n) = match self.doc.mtime {
            Some(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs(), d.subsec_nanos()),
                Err(_) => (0, 0),
            },
            None => (0, 0),
        };
        FileRev { size: self.doc.size, mtime_s: s, mtime_n: n }
    }

    pub(crate) fn start_search(&mut self, history: &mut Vec<HistEntry>) {
        use crate::engine::query;
        let q = self.search_text.clone();
        self.last_searched = q.clone();
        self.debounce_at = None;
        if q.trim().is_empty() {
            self.doc.hits.clear();
            self.doc.search_error = None;
            self.doc.search_in_progress = false;
            self.current_hit = None;
            self.results_collapsed = true;
            self.refresh_mode_map();
            return;
        }
        // Catat ke history global (dedupe + batas di dalam).
        crate::store::push_history(
            history,
            HistEntry {
                query: q.clone(),
                regex: self.regex_on,
                case_sensitive: self.case_sensitive,
            },
        );
        self.doc.search_gen += 1;
        let gen = self.doc.search_gen;
        self.gen_shared.store(gen, Ordering::Relaxed);
        self.search_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.search_rx = Some(rx);
        self.doc.hits.clear();
        self.refresh_mode_map();
        self.doc.search_truncated = false;
        self.doc.search_error = None;
        self.doc.search_in_progress = true;
        self.current_hit = None;
        self.search_scanned = 0;
        self.search_total = self.doc.size;
        // Cache: pola sama + revisi file sama = instan.
        let bool_mode = !self.regex_on && query::is_boolean_query(&q);
        // Cakupan baris -> interval byte via checkpoint (None bila belum petakan).
        let scope_bytes = self.scope.and_then(|(a, b)| {
            let (s, _) = self.doc.line_byte_range(a)?;
            let (_, e) = self.doc.line_byte_range(b)?;
            Some((s, e))
        });
        let key = CacheKey {
            query: q.clone(),
            regex: self.regex_on,
            case_sensitive: self.case_sensitive,
            boolean: bool_mode,
            scope: self.scope,
            rev: self.file_rev(),
        };
        if let Some(hits) = self.search_cache.get(&key) {
            self.doc.hits = hits;
            self.doc.search_in_progress = false;
            self.search_scanned = self.doc.size;
            self.current_hit = Some(0);
            self.results_collapsed = false;
            self.doc.status = String::from("Hasil dari cache (pola sama).");
            if !self.doc.hits.is_empty() {
                let ln = self.doc.hits[0].line;
                self.selected_line = ln;
                self.center_on_line(ln);
            }
            return;
        }
        self.pending_key = Some(key);
        if bool_mode {
            match query::parse_query(&q) {
                Ok(ast) => spawn_bool_search(
                    self.doc.path.clone(),
                    ast,
                    self.doc.encoding(),
                    self.doc.bom_len,
                    self.case_sensitive,
                    self.scope,
                    gen,
                    self.gen_shared.clone(),
                    tx,
                    cancel,
                ),
                Err(e) => {
                    self.doc.search_error = Some(e);
                    self.doc.search_in_progress = false;
                }
            }
            return;
        }
        spawn_search(
            self.doc.path.clone(),
            q,
            self.regex_on,
            self.case_sensitive,
            scope_bytes,
            gen,
            self.gen_shared.clone(),
            tx,
            cancel,
        );
    }

    pub(crate) fn start_filter(&mut self, query: String) {
        let f = parse_filter(&query, self.case_sensitive);
        self.doc.filter = f.clone();
        self.range_applied = None;
        if f.is_empty() {
            self.doc.filter_active = false;
            self.doc.filter_map.clear();
            self.top_row = 0;
            self.doc.status = String::from("Filter dikosongkan — menampilkan semua baris.");
            return;
        }
        self.doc.filter_active = false; // aktif setelah selesai
        self.doc.status = String::from("Memfilter…");
        let (tx, rx) = mpsc::channel();
        self.filter_rx = Some(rx);
        spawn_filter(
            self.doc.path.clone(),
            self.doc.encoding(),
            self.doc.bom_len,
            f,
            tx,
        );
        self.top_row = 0;
    }

    pub(crate) fn poll_channels(&mut self) {
        // Index updates
        if let Some(rx) = &self.index_rx {
            while let Ok(u) = rx.try_recv() {
                self.doc.apply_index(u.index.clone());
                if u.index.complete {
                    // selesai: putuskan channel
                }
            }
            if self.doc.index.complete {
                self.index_rx = None;
            }
        }
        // Search batches
        if let Some(rx) = &self.search_rx {
            let mut done = false;
            let mut any = false;
            while let Ok(m) = rx.try_recv() {
                any = true;
                if m.gen != self.doc.search_gen {
                    continue; // hasil lama, buang
                }
                if let Some(e) = m.error {
                    self.doc.search_error = Some(e);
                    self.doc.search_in_progress = false;
                    done = true;
                    break;
                }
                self.search_scanned = m.scanned;
                self.search_total = m.total;
                let was_empty = self.doc.hits.is_empty();
                self.doc.hits.extend(m.batch);
                // Hit pertama masuk: buka panel hasil langsung (tak perlu tunggu selesai).
                if was_empty && !self.doc.hits.is_empty() {
                    self.results_collapsed = false;
                }
                if m.truncated {
                    self.doc.search_truncated = true;
                }
                if m.done {
                    self.doc.search_in_progress = false;
                    done = true;
                }
            }
            let _ = any;
            if done {
                self.search_rx = None;
                // Simpan hasil lengkap ke cache (pola ulang = instan).
                if self.doc.search_error.is_none() && !self.doc.search_truncated {
                    if let Some(key) = self.pending_key.take() {
                        self.search_cache.put(key, self.doc.hits.clone());
                    }
                } else {
                    self.pending_key = None;
                }
                self.refresh_mode_map();
                if self.doc.search_error.is_none() && !self.doc.hits.is_empty() {
                    self.current_hit = Some(0);
                    // Ada hasil: buka panel hasil otomatis.
                    self.results_collapsed = false;
                    // lompat ke hasil pertama
                    let ln = self.doc.hits[0].line;
                    self.selected_line = ln;
                    self.center_on_line(ln);
                }
            }
        }
        // Filter result
        if let Some(rx) = &self.filter_rx {
            if let Ok((map, _trunc)) = rx.try_recv() {
                self.doc.filter_map = map;
                self.doc.filter_active = true;
                self.top_row = 0;
                self.doc.status = format!(
                    "Filter aktif: {} baris cocok.",
                    format_count(self.doc.filter_map.len() as u64)
                );
                self.filter_rx = None;
            }
        }
        // Marker ERROR/WARN + histogram selesai.
        if let Some(rx) = &self.marker_rx {
            while let Ok((bits, size, hist)) = rx.try_recv() {
                self.marker_bits = Some(bits);
                self.marker_size = size;
                self.time_hist = hist;
            }
            if self.marker_bits.is_some() {
                self.marker_rx = None;
            }
        }
        // Indeks selesai & peta marker usang/belum ada -> pindai bucket latar.
        // (Hanya bila selisih ukuran > 64 MB agar append kecil tak memicu pindaian ulang.)
        if self.doc.index.complete && self.marker_rx.is_none() {
            let stale = match &self.marker_bits {
                None => true,
                Some(_) => self.doc.size.saturating_sub(self.marker_size) > 64 * 1024 * 1024,
            };
            if stale {
                let (tx, rx) = mpsc::channel();
                self.marker_rx = Some(rx);
                spawn_marker_scan(self.doc.path.clone(), self.doc.size, tx);
            }
        }
    }

    pub(crate) fn center_on_line(&mut self, line: u64) {
        // Posisikan baris ~40% dari atas viewport (bukan paling atas)
        // agar konteks sebelum match ikut terbaca.
        let row = self.view_row_of_line(line).unwrap_or(0);
        let off = (self.last_visible.max(10) * 2 / 5).max(3);
        self.top_row = row.saturating_sub(off);
    }

    pub(crate) fn view_row_of_line(&self, line: u64) -> Option<u64> {
        if self.view_mode != ViewMode::All {
            return self
                .mode_lines
                .iter()
                .position(|&l| l == line)
                .map(|i| i as u64);
        }
        if self.doc.filter_active {
            self.doc.filter_map.iter().position(|&l| l == line).map(|i| i as u64)
        } else {
            if line < 1 {
                return None;
            }
            Some(line - 1)
        }
    }

    pub(crate) fn jump_to_hit(&mut self, idx: usize) {
        if idx >= self.doc.hits.len() {
            return;
        }
        self.current_hit = Some(idx);
        let ln = self.doc.hits[idx].line;
        self.selected_line = ln;
        self.center_on_line(ln);
        self.record_nav(ln);
    }

    /// Catat lokasi ke history (maks 200; memotong cabang maju).
    pub(crate) fn record_nav(&mut self, line: u64) {
        if self.hist.last() == Some(&line) {
            return;
        }
        self.hist.truncate(self.hist_pos);
        self.hist.push(line);
        if self.hist.len() > 200 {
            self.hist.remove(0);
        }
        self.hist_pos = self.hist.len();
    }

    /// Lompat tercatat (goto, hasil, penanda, strip): pilih + pusatkan + catat.
    pub(crate) fn nav_to(&mut self, line: u64) {
        self.selected_line = line;
        self.center_on_line(line);
        self.record_nav(line);
    }

    /// History maju/mundur tanpa mencatat (Alt+Left/Right).
    pub(crate) fn go_hist(&mut self, back: bool) -> bool {
        if self.hist.is_empty() {
            return false;
        }
        if back {
            if self.hist_pos <= 1 {
                return false;
            }
            self.hist_pos -= 1;
        } else if self.hist_pos < self.hist.len() {
            self.hist_pos += 1;
        } else {
            return false;
        }
        let ln = self.hist[self.hist_pos - 1];
        self.selected_line = ln;
        self.center_on_line(ln);
        true
    }

    /// Penanda sebelumnya/berikutnya (Alt+Up/Down).
    pub(crate) fn go_mark(&mut self, prev: bool) -> bool {
        if self.doc.bookmarks.is_empty() {
            return false;
        }
        let cur = self.selected_line;
        let target = if prev {
            self.doc
                .bookmarks
                .iter()
                .rev()
                .find(|b| b.line < cur)
                .or_else(|| self.doc.bookmarks.iter().last())
        } else {
            self.doc
                .bookmarks
                .iter()
                .find(|b| b.line > cur)
                .or_else(|| self.doc.bookmarks.first())
        };
        if let Some(b) = target {
            let ln = b.line;
            self.nav_to(ln);
            true
        } else {
            false
        }
    }

    pub(crate) fn poll_follow(&mut self) {
        if !self.doc.follow {
            return;
        }
        if self.last_follow_poll.elapsed() < Duration::from_millis(400) {
            return;
        }
        self.last_follow_poll = Instant::now();
        let cur = match load_identity(&self.doc.path) {
            Ok(c) => c,
            Err(e) => {
                self.doc.status = format!("Gagal memantau file: {}", e);
                return;
            }
        };
        match check_follow(self.doc.size, self.follow_fp.as_deref(), &cur) {
            FollowEvent::Unchanged => {
                // Adopsi sidik (poll pertama setelah buka).
                if self.follow_fp.is_none() {
                    self.follow_fp = cur.head.clone();
                }
            }
            FollowEvent::Appended(new_size) => {
                let old_bytes = self.doc.size;
                let old_lines = if self.doc.index.complete {
                    self.doc.index.total_lines
                } else {
                    0
                };
                match self.doc.remap() {
                    Ok(()) => {
                        // Revisi file berubah -> cache pencarian gugur.
                        self.search_cache.clear();
                        if self.doc.index.complete && old_lines > 0 {
                            self.doc.index_tail(old_bytes, old_lines);
                            let added = self.doc.index.total_lines.saturating_sub(old_lines);
                            if added > 0 {
                                self.follow_note = Some((
                                    format!("+{} baris baru", format_count(added)),
                                    Instant::now(),
                                ));
                            }
                        } else {
                            // indeks belum selesai: biarkan indexer latar menyelesaikannya;
                            // mulai ulang indexer dari posisi baru bila perlu.
                            let (tx, rx) = mpsc::channel();
                            self.index_cancel.store(true, Ordering::Relaxed);
                            let cancel = Arc::new(AtomicBool::new(false));
                            self.index_cancel = cancel.clone();
                            spawn_indexer(
                                self.doc.path.clone(),
                                self.doc.encoding(),
                                self.doc.bom_len,
                                self.doc.size,
                                tx,
                                cancel,
                            );
                            self.index_rx = Some(rx);
                        }
                        self.doc.size = new_size;
                        self.doc.mtime = cur.mtime;
                        self.follow_fp = cur.head.clone();
                        if self.doc.stick_bottom {
                            self.top_row = self.total_view_rows().saturating_sub(60);
                        }
                    }
                    Err(e) => self.doc.status = e,
                }
            }
            FollowEvent::TruncatedOrRotated => {
                if let Err(e) = self.doc.reopen_after_rotate() {
                    self.doc.status = e;
                } else {
                    // Revisi file berubah -> cache pencarian gugur.
                    self.search_cache.clear();
                    // Sidik baru diadopsi; peta marker dibangun ulang.
                    self.follow_fp = load_identity(&self.doc.path)
                        .ok()
                        .and_then(|id| id.head);
                    self.marker_bits = None;
                    self.marker_size = 0;
                    self.time_hist = None;
                    // indeks ulang
                    let (tx, rx) = mpsc::channel();
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.index_cancel = cancel.clone();
                    spawn_indexer(
                        self.doc.path.clone(),
                        self.doc.encoding(),
                        self.doc.bom_len,
                        self.doc.size,
                        tx,
                        cancel,
                    );
                    self.index_rx = Some(rx);
                    self.top_row = 0;
                }
            }
        }
    }
}
