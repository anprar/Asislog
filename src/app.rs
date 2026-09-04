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

// ---------- background messages ----------

#[derive(Clone, Debug)]
struct IndexUpdate {
    index: SparseIndex,
}

#[derive(Clone, Debug)]
struct SearchBatchMsg {
    gen: u64,
    batch: Vec<Hit>,
    done: bool,
    truncated: bool,
    error: Option<String>,
    /// Bytes scanned so far (for progress: `Mencari… x / y`).
    scanned: u64,
    /// Total file bytes at search start.
    total: u64,
}

/// Gutter + text responses for one log row (for distinct click targets).
struct RowResp {
    g: egui::Response,
    t: egui::Response,
}

/// Hasil unduhan URL di thread latar.
enum DlMsg {
    Done(PathBuf),
    Failed(String),
}

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

struct TabState {
    doc: Doc,
    search_text: String,
    last_searched: String,
    debounce_at: Option<Instant>,
    case_sensitive: bool,
    regex_on: bool,
    current_hit: Option<usize>,
    /// Progress pindaian pencarian (diperbarui per batch).
    search_scanned: u64,
    search_total: u64,
    /// Cache hasil lengkap per query+revisi file (pola ulang = instan).
    search_cache: SearchCache,
    /// Kunci cache untuk pencarian yang sedang berjalan.
    pending_key: Option<CacheKey>,
    filter_text: String,
    top_row: u64,
    selected_line: u64,
    hover_line: Option<u64>,
    /// File temp hasil ekstrak arsip (dihapus saat tab ditutup).
    temp_path: Option<PathBuf>,
    /// Mode tampil viewport + peta barisnya.
    view_mode: ViewMode,
    mode_lines: Vec<u64>,
    /// Cakupan pencarian (baris lo..=hi) + teks dialog + buka dialog.
    scope: Option<(u64, u64)>,
    scope_a: String,
    scope_b: String,
    scope_open: bool,
    scope_msg: String,    show_bookmarks: bool,
    /// Saringan ketik di panel penanda.
    mark_query: String,
    /// Rentang waktu yang sedang diterapkan sebagai filter (start, end).
    range_applied: Option<(String, String)>,
    /// Panel hasil diciutkan (header saja) agar viewport log lega.
    results_collapsed: bool,
    goto_open: bool,
    goto_input: String,
    goto_msg: String,
    export_open: bool,
    export_context: usize,
    index_rx: Option<mpsc::Receiver<IndexUpdate>>,
    search_rx: Option<mpsc::Receiver<SearchBatchMsg>>,
    filter_rx: Option<mpsc::Receiver<(Vec<u64>, bool)>>,
    /// Peta bucket ERROR/WARN (512 byte) + ukuran file saat dipindai.
    marker_bits: Option<Vec<u8>>,
    marker_size: u64,
    marker_rx: Option<mpsc::Receiver<(Vec<u8>, u64, Option<TimeHist>)>>,
    /// Histogram ERROR per menit (shading strip).
    time_hist: Option<TimeHist>,
    /// Catatan follow segar, mis. `+128 baris baru` (+ waktu).
    follow_note: Option<(String, Instant)>,
    /// Jumlah baris terlihat terakhir (untuk posisi lompat 40% viewport).
    last_visible: u64,
    /// History navigasi (nomor baris) + posisi kini. Maks 200.
    hist: Vec<u64>,
    hist_pos: usize,
    /// True bila penanda berubah dan sidecar perlu ditulis ulang.
    marks_dirty: bool,
    /// top_row terakhir yang tercatat untuk sesi (hemat tulis).
    saved_top: u64,
    gen_shared: Arc<AtomicU64>,
    index_cancel: Arc<AtomicBool>,
    search_cancel: Arc<AtomicBool>,
    last_follow_poll: Instant,
    /// Sidik head terakhir untuk follow (None = belum diketahui).
    follow_fp: Option<String>,
    pending_search_error: Option<String>,
}

impl TabState {
    fn new(doc: Doc) -> Self {
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

    fn total_view_rows(&self) -> u64 {
        if self.view_mode != ViewMode::All {
            self.mode_lines.len().max(1) as u64
        } else {
            self.doc.view_row_count().max(1)
        }
    }

    /// View-row -> nomor baris asli (hormati mode tampil dulu, lalu filter).
    fn row_to_line(&self, row: u64) -> Option<u64> {
        if self.view_mode != ViewMode::All {
            self.mode_lines.get(row as usize).copied()
        } else {
            self.doc.view_row_to_line(row)
        }
    }

    /// Bangun ulang peta mode tampil dari hits/penanda kini.
    fn refresh_mode_map(&mut self) {
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
    fn file_rev(&self) -> FileRev {
        let (s, n) = match self.doc.mtime {
            Some(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs(), d.subsec_nanos()),
                Err(_) => (0, 0),
            },
            None => (0, 0),
        };
        FileRev { size: self.doc.size, mtime_s: s, mtime_n: n }
    }

    fn start_search(&mut self, history: &mut Vec<HistEntry>) {
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

    fn start_filter(&mut self, query: String) {
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

    fn poll_channels(&mut self) {
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

    fn center_on_line(&mut self, line: u64) {
        // Posisikan baris ~40% dari atas viewport (bukan paling atas)
        // agar konteks sebelum match ikut terbaca.
        let row = self.view_row_of_line(line).unwrap_or(0);
        let off = (self.last_visible.max(10) * 2 / 5).max(3);
        self.top_row = row.saturating_sub(off);
    }

    fn view_row_of_line(&self, line: u64) -> Option<u64> {
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

    fn jump_to_hit(&mut self, idx: usize) {
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
    fn record_nav(&mut self, line: u64) {
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
    fn nav_to(&mut self, line: u64) {
        self.selected_line = line;
        self.center_on_line(line);
        self.record_nav(line);
    }

    /// History maju/mundur tanpa mencatat (Alt+Left/Right).
    fn go_hist(&mut self, back: bool) -> bool {
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
    fn go_mark(&mut self, prev: bool) -> bool {
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

    fn poll_follow(&mut self) {
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

// ---------- background workers (file-based, bounded RAM) ----------

fn spawn_indexer(
    path: PathBuf,
    encoding: Encoding,
    bom_len: usize,
    total_size: u64,
    tx: mpsc::Sender<IndexUpdate>,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        use std::io::Read;
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        let mut checkpoints: Vec<(u64, u64)> = vec![(1, bom_len as u64)];
        let mut line: u64 = 1;
        let mut byte: u64 = bom_len as u64;
        let mut last_cp_line = 1u64;
        let mut last_cp_byte = bom_len as u64;
        let mut buf = vec![0u8; 1024 * 1024];
        let mut last_send = Instant::now();
        let wide = encoding.is_wide();
        let le = encoding == Encoding::Utf16Le;
        // Skip BOM
        let mut skipped = 0usize;
        // For wide, align reads to even boundaries (approx: assume bom 2).
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            let mut start = 0usize;
            if skipped < bom_len {
                let skip = (bom_len - skipped).min(n);
                start = skip;
                skipped += skip;
                byte += skip as u64;
            }
            if wide {
                let mut i = start;
                if (byte % 2) == 1 && i < n {
                    i += 1;
                    byte += 1;
                }
                while i + 1 < n {
                    let is_nl = if le {
                        buf[i] == 0x0A && buf[i + 1] == 0x00
                    } else {
                        buf[i] == 0x00 && buf[i + 1] == 0x0A
                    };
                    byte += 2;
                    if is_nl {
                        line += 1;
                        if line - last_cp_line >= index::CHECKPOINT_LINES
                            || byte - last_cp_byte >= index::CHECKPOINT_BYTES
                        {
                            checkpoints.push((line, byte));
                            last_cp_line = line;
                            last_cp_byte = byte;
                        }
                    }
                    i += 2;
                }
                // odd tail byte ignored (carried roughly; wide huge files are rare)
            } else {
                for &b in &buf[start..n] {
                    byte += 1;
                    if b == b'\n' {
                        line += 1;
                        if byte < total_size
                            && (line - last_cp_line >= index::CHECKPOINT_LINES
                                || byte - last_cp_byte >= index::CHECKPOINT_BYTES)
                        {
                            checkpoints.push((line, byte));
                            last_cp_line = line;
                            last_cp_byte = byte;
                        }
                    }
                }
            }
            if last_send.elapsed() > Duration::from_millis(50) {
                last_send = Instant::now();
                let progress = if total_size > 0 {
                    (byte as f32 / total_size as f32).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let total_lines = line;
                let _ = tx.send(IndexUpdate {
                    index: SparseIndex {
                        checkpoints: checkpoints.clone(),
                        total_lines,
                        total_bytes: total_size,
                        complete: false,
                        progress,
                    },
                });
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // Trim phantom trailing line (narrow): count newlines vs lines.
        // Rebuild precisely is O(n) again; instead adjust: if file ends with \n,
        // last increment created empty line past EOF -> subtract.
        let mut total_lines = line;
        // Check last byte cheaply:
        if let Ok(md) = std::fs::metadata(&path) {
            if md.len() > 0 && !wide {
                use std::io::{Seek, SeekFrom};
                if let Ok(mut f) = std::fs::File::open(&path) {
                    if f.seek(SeekFrom::End(-1)).is_ok() {
                        let mut last = [0u8; 1];
                        if f.read_exact(&mut last).is_ok() && last[0] == b'\n' {
                            total_lines = total_lines.saturating_sub(1).max(1);
                        }
                    }
                }
            }
        }
        let _ = tx.send(IndexUpdate {
            index: SparseIndex {
                checkpoints,
                total_lines,
                total_bytes: total_size,
                complete: true,
                progress: 1.0,
            },
        });
    });
}

fn spawn_search(
    path: PathBuf,
    query: String,
    regex_on: bool,
    case_sensitive: bool,
    scope: Option<(u64, u64)>,
    gen: u64,
    gen_shared: Arc<AtomicU64>,
    tx: mpsc::Sender<SearchBatchMsg>,
    _cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        use std::io::Read;
        if query.trim().is_empty() {
            let _ = tx.send(SearchBatchMsg {
                gen,
                batch: Vec::new(),
                done: true,
                truncated: false,
                error: None,
                scanned: 0,
                total: 0,
            });
            return;
        }
        // Compile regex once (bytes).
        let re: Option<regex::bytes::Regex> = if regex_on {
            let mut b = regex::bytes::RegexBuilder::new(&query);
            b.case_insensitive(!case_sensitive);
            match b.build() {
                Ok(r) => Some(r),
                Err(e) => {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: true,
                        truncated: false,
                        error: Some(format!("Regex tidak valid: {}", e)),
                        scanned: 0,
                        total: 0,
                    });
                    return;
                }
            }
        } else {
            None
        };
        let needle_cmp: Vec<u8> = if !regex_on {
            if case_sensitive {
                query.as_bytes().to_vec()
            } else {
                query.as_bytes().to_ascii_lowercase()
            }
        } else {
            Vec::new()
        };
        let finder = if !regex_on {
            Some(memchr::memmem::Finder::new(&needle_cmp))
        } else {
            None
        };
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(SearchBatchMsg {
                    gen,
                    batch: Vec::new(),
                    done: true,
                    truncated: false,
                    error: Some(format!("Gagal membaca file: {}", e)),
                    scanned: 0,
                    total: 0,
                });
                return;
            }
        };
        let total_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut reader = std::io::BufReader::with_capacity(4 * 1024 * 1024, file);
        let chunk_size = search::SEARCH_CHUNK;
        let mut buf = vec![0u8; chunk_size + 16 * 1024];
        let mut carry: Vec<u8> = Vec::new(); // overlap tail
        let mut global_offset: u64 = 0;
        let mut line_no: u64 = 1;
        let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
        let mut total_found: usize = 0;
        let mut truncated = false;
        // Track whether previous chunk ended mid-line to fix line numbers:
        // line_no always counts lines started. carry holds tail bytes of prev chunk
        // (up to overlap) that may contain a partial line; we handle by scanning
        // combined = carry + chunk for matches but only report matches starting
        // at >= carry_len - overlap_guard? Simplify: report matches in combined
        // whose start >= carry.len() except first chunk, plus handle cross-boundary
        // by overlap = needle.len().
        let overlap = if !regex_on {
            needle_cmp.len().min(16 * 1024)
        } else {
            8 * 1024
        };
        loop {
            if gen_shared.load(Ordering::Relaxed) != gen {
                return; // dibatalkan (search_gen)
            }
            // Build combined buffer: carry + new bytes
            let mut combined = std::mem::take(&mut carry);
            let combined_base = global_offset.saturating_sub(combined.len() as u64);
            // read next chunk after carry
            let mut tmp = vec![0u8; chunk_size];
            let n = match reader.read(&mut tmp) {
                Ok(0) => 0,
                Ok(n) => n,
                Err(e) => {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: true,
                        truncated,
                        error: Some(format!("Gagal membaca file: {}", e)),
                        scanned: global_offset,
                        total: total_size,
                    });
                    return;
                }
            };
            if n == 0 && combined.is_empty() {
                break;
            }
            tmp.truncate(n);
            // line number at combined start:
            // combined_base corresponds to (global_offset - carry_len). line_no tracks
            // lines up to global_offset. Rewind line_no by newlines in carry? We kept
            // carry as tail of previous combined that was already counted. To avoid
            // double counting, recompute: newlines in carry were already counted when
            // advancing line_no past them? We set carry from tail AFTER counting, so
            // line_no currently points past carry. So start_line = line_no - nl(carry).
            let carry_nl = memchr::memchr_iter(b'\n', &combined).count() as u64;
            let mut cur_line = line_no.saturating_sub(carry_nl);
            let carry_len = combined.len();
            combined.extend_from_slice(&tmp);
            let is_last = n < chunk_size;
            // Cakupan byte: lewati bongkah di luar interval (tetap hitung
            // baris/carry agar penomoran persis; cocok tepi difilter per-match).
            if let Some((ss, ee)) = scope {
                let cs = combined_base;
                let ce = combined_base + combined.len() as u64;
                if ce <= ss || cs >= ee {
                    line_no += memchr::memchr_iter(b'\n', &tmp).count() as u64;
                    global_offset += n as u64;
                    let tail = overlap.min(combined.len());
                    carry = combined[combined.len() - tail..].to_vec();
                    if is_last {
                        break;
                    }
                    continue;
                }
            }
            // Search in combined (lowercased view if needed)
            if !regex_on {
                let hay_owned;
                let hay: &[u8] = if case_sensitive {
                    &combined
                } else {
                    hay_owned = combined.to_ascii_lowercase();
                    // search within this scope using owned copy; positions identical
                    // length, so we can proceed with a block:
                    let f = finder.as_ref().unwrap();
                    // line starts relative
                    let mut rel_line = cur_line;
                    let mut line_start_rel = 0usize;
                    // If combined_base > 0 and first byte continues a line, cur_line is
                    // already correct (continuation). Find first newline to set starts.
                    // Iterate matches:
                    for m in f.find_iter(&hay_owned) {
                        if gen_shared.load(Ordering::Relaxed) != gen {
                            return;
                        }
                        if m < carry_len.saturating_sub(overlap) && global_offset != 0 {
                            // already reported in previous chunk
                            continue;
                        }
                        // count newlines between line_start tracking? Simpler: count
                        // newlines from previous match? For batch sizes this O(n*m)
                        // could be slow; instead precompute line starts once below.
                        let _ = (rel_line, line_start_rel);
                        let _ = m;
                    }
                    // Fall through to unified path below using hay_owned:
                    // To keep code simple, replace combined with lowered copy:
                    drop(combined);
                    combined = hay_owned;
                    // recompute below with case_sensitive=true semantics
                    let _ = &mut rel_line;
                    let _ = &mut line_start_rel;
                    &combined
                };
                // Precompute relative line starts for combined.
                let starts_mid = combined_base > 0;
                let _ = starts_mid;
                let mut rel_starts: Vec<usize> = Vec::new();
                // Determine if combined starts mid-line: check byte before combined_base.
                // We don't have it; approximate: if global_offset != 0 then the first
                // line is a continuation UNLESS carry was empty and previous chunk ended
                // exactly at newline. Track via flag: previous chunk end char.
                // Simplify: if carry_len > 0, first line is continuation (no new start).
                let cont = carry_len > 0;
                if !cont {
                    rel_starts.push(0);
                }
                for i in memchr::memchr_iter(b'\n', hay) {
                    if i + 1 < hay.len() {
                        rel_starts.push(i + 1);
                    }
                }
                let f = finder.as_ref().unwrap();
                for m in f.find_iter(hay) {
                    if gen_shared.load(Ordering::Relaxed) != gen {
                        return;
                    }
                    if global_offset != 0 && m < carry_len.saturating_sub(overlap) {
                        continue;
                    }
                    // line lookup
                    let li = match rel_starts.binary_search(&m) {
                        Ok(i) => i,
                        Err(i) => {
                            if i == 0 {
                                // inside continuation line
                                // line = cur_line, col = distance to combined start
                                // + need global line start for byte: scan back in file?
                                // Approximate byte as combined_base (line started earlier).
                                // For byte field, find previous newline in hay before m,
                                // else use combined_base.
                                let mut p = m;
                                while p > 0 && hay[p - 1] != b'\n' {
                                    p -= 1;
                                }
                                let gstart = if p == 0 && cont {
                                    // true start is before combined; estimate
                                    combined_base
                                } else {
                                    combined_base + p as u64
                                };
                                let hit = Hit {
                                    line: cur_line,
                                    byte: gstart,
                                    col_start: (m - p) as u32,
                                    col_end: (m - p + needle_cmp.len()) as u32,
                                };
                                if let Some((ss, ee)) = scope {
                                    let gp = combined_base + m as u64;
                                    if gp < ss || gp >= ee {
                                        continue;
                                    }
                                }
                                pending.push(hit.clone());
                                total_found += 1;
                                if pending.len() >= search::SEARCH_BATCH {
                                    let b = std::mem::take(&mut pending);
                                    let _ = tx.send(SearchBatchMsg {
                                        gen,
                                        batch: b,
                                        done: false,
                                        truncated: false,
                                        error: None,
                                        scanned: global_offset,
                                        total: total_size,
                                    });
                                }
                                if total_found >= search::MAX_STORED_HITS {
                                    truncated = true;
                                }
                                if truncated {
                                    break;
                                }
                                continue;
                            } else {
                                i - 1
                            }
                        }
                    };
                    let ls = rel_starts[li];
                    let gline = cur_line + li as u64 - if cont { 0 } else { 0 };
                    // When cont, rel_starts[0] is second line; li indexes accordingly.
                    // cur_line is line number of combined[0]'s line. If cont, combined[0]
                    // belongs to cur_line, but rel_starts[0] starts at cur_line+1.
                    // Adjust:
                    let gline2 = if cont { cur_line + li as u64 + 1 } else { cur_line + li as u64 };
                    let _ = gline;
                    let hit = Hit {
                        line: gline2,
                        byte: combined_base + ls as u64,
                        col_start: (m - ls) as u32,
                        col_end: (m - ls + needle_cmp.len()) as u32,
                    };
                    if let Some((ss, ee)) = scope {
                        let gp = combined_base + m as u64;
                        if gp < ss || gp >= ee {
                            continue;
                        }
                    }
                    pending.push(hit);
                    total_found += 1;
                    if pending.len() >= search::SEARCH_BATCH {
                        let b = std::mem::take(&mut pending);
                                let _ = tx.send(SearchBatchMsg {
                                    gen,
                                    batch: b,
                                    done: false,
                                    truncated: false,
                                    error: None,
                                    scanned: global_offset,
                                    total: total_size,
                                });
                    }
                    if total_found >= search::MAX_STORED_HITS {
                        truncated = true;
                        break;
                    }
                }
                // advance line_no by newlines in the non-carry part only (avoid double count)
                let new_part = if combined.len() >= carry_len {
                    &combined[carry_len.saturating_sub(overlap.min(carry_len))..]
                } else {
                    &combined[..]
                };
                // Actually simpler: line_no += newlines in tmp (fresh bytes only).
                let _ = new_part;
                let nl_new = memchr::memchr_iter(b'\n', &tmp).count() as u64;
                line_no += nl_new;
                global_offset += n as u64;
                // new carry = tail overlap of combined
                let tail = overlap.min(combined.len());
                carry = combined[combined.len() - tail..].to_vec();
                if !pending.is_empty() && (pending.len() >= 200 || is_last) {
                    let b = std::mem::take(&mut pending);
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: b,
                        done: false,
                        truncated: false,
                        error: None,
                        scanned: global_offset,
                        total: total_size,
                    });
                }
                if truncated {
                    break;
                }
                if is_last {
                    break;
                }
            } else {
                // Regex path: search combined as bytes.
                let re = re.as_ref().unwrap();
                let cont = carry_len > 0;
                let mut rel_starts: Vec<usize> = Vec::new();
                if !cont {
                    rel_starts.push(0);
                }
                for i in memchr::memchr_iter(b'\n', &combined) {
                    if i + 1 < combined.len() {
                        rel_starts.push(i + 1);
                    }
                }
                for m in re.find_iter(&combined) {
                    if gen_shared.load(Ordering::Relaxed) != gen {
                        return;
                    }
                    let (s, e) = (m.start(), m.end());
                    if e == s {
                        continue;
                    }
                    if global_offset != 0 && s < carry_len.saturating_sub(overlap) {
                        continue;
                    }
                    if let Some((ss, ee)) = scope {
                        let gp = combined_base + s as u64;
                        if gp < ss || gp >= ee {
                            continue;
                        }
                    }
                    // line lookup
                    let (gline, ls) = match rel_starts.binary_search(&s) {
                        Ok(i) => {
                            let g = if cont { cur_line + i as u64 + 1 } else { cur_line + i as u64 };
                            (g, rel_starts[i])
                        }
                        Err(0) => (cur_line, {
                            let mut p = s;
                            while p > 0 && combined[p - 1] != b'\n' {
                                p -= 1;
                            }
                            p
                        }),
                        Err(i) => {
                            let g = if cont { cur_line + (i - 1) as u64 + 1 } else { cur_line + (i - 1) as u64 };
                            (g, rel_starts[i - 1])
                        }
                    };
                    let line_end = combined
                        .iter()
                        .skip(s)
                        .position(|&b| b == b'\n')
                        .map(|k| s + k)
                        .unwrap_or(combined.len());
                    let ce = e.min(line_end).saturating_sub(ls) as u32;
                    pending.push(Hit {
                        line: gline,
                        byte: combined_base + ls as u64,
                        col_start: s.saturating_sub(ls) as u32,
                        col_end: ce,
                    });
                    total_found += 1;
                    if pending.len() >= search::SEARCH_BATCH {
                        let b = std::mem::take(&mut pending);
                                let _ = tx.send(SearchBatchMsg {
                                    gen,
                                    batch: b,
                                    done: false,
                                    truncated: false,
                                    error: None,
                                    scanned: global_offset,
                                    total: total_size,
                                });
                    }
                    if total_found >= search::MAX_STORED_HITS {
                        truncated = true;
                        break;
                    }
                }
                let nl_new = memchr::memchr_iter(b'\n', &tmp).count() as u64;
                line_no += nl_new;
                global_offset += n as u64;
                let tail = overlap.min(combined.len());
                carry = combined[combined.len() - tail..].to_vec();
                let _ = cur_line;
                if !pending.is_empty() && (pending.len() >= 200 || is_last) {
                    let b = std::mem::take(&mut pending);
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: b,
                        done: false,
                        truncated: false,
                        error: None,
                        scanned: global_offset,
                        total: total_size,
                    });
                }
                if truncated {
                    break;
                }
                if is_last {
                    break;
                }
            }
        }
        if gen_shared.load(Ordering::Relaxed) != gen {
            return;
        }
        let _ = tx.send(SearchBatchMsg {
            gen,
            batch: std::mem::take(&mut pending),
            done: true,
            truncated,
            error: None,
            scanned: total_size,
            total: total_size,
        });
    });
}

/// Worker pencarian boolean: evaluasi AST per baris terdecode.
/// Batch 500 hit + progres byte, hormati `search_gen` seperti worker literal.
fn spawn_bool_search(
    path: PathBuf,
    ast: crate::engine::query::Query,
    encoding: Encoding,
    bom_len: usize,
    case_sensitive: bool,
    scope_lines: Option<(u64, u64)>,
    gen: u64,
    gen_shared: Arc<AtomicU64>,
    tx: mpsc::Sender<SearchBatchMsg>,
    _cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        if encoding.is_wide() {
            let _ = tx.send(SearchBatchMsg {
                gen,
                batch: Vec::new(),
                done: true,
                truncated: false,
                error: Some(String::from(
                    "Boolean search belum mendukung UTF-16; gunakan literal.",
                )),
                scanned: 0,
                total: 0,
            });
            return;
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(SearchBatchMsg {
                    gen,
                    batch: Vec::new(),
                    done: true,
                    truncated: false,
                    error: Some(format!("Gagal membaca file: {}", e)),
                    scanned: 0,
                    total: 0,
                });
                return;
            }
        };
        let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        let terms: Vec<String> = ast
            .positive_terms()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let term_refs: Vec<&str> = terms.iter().map(|s| s.as_str()).collect();
        let mut line_no: u64 = 1;
        let mut byte_off: u64 = 0;
        let mut scanned: u64 = 0;
        let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
        let mut total_found: usize = 0;
        let mut truncated = false;
        let mut buf: Vec<u8> = Vec::new();
        // Lewati BOM pada baris pertama.
        let mut first = true;
        loop {
            if gen_shared.load(Ordering::Relaxed) != gen {
                return;
            }
            buf.clear();
            let n = match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: true,
                        truncated,
                        error: Some(format!("Gagal membaca file: {}", e)),
                        scanned,
                        total,
                    });
                    return;
                }
            };
            let line_start = byte_off;
            byte_off += n as u64;
            scanned += n as u64;
            let mut lb = &buf[..];
            if lb.ends_with(b"\n") {
                lb = &lb[..lb.len() - 1];
            }
            if first {
                first = false;
                if bom_len > 0 && lb.len() >= bom_len {
                    lb = &lb[bom_len..];
                }
            }
            let mut bytes = lb;
            if !bytes.is_empty() && bytes[bytes.len() - 1] == b'\r' {
                bytes = &bytes[..bytes.len() - 1];
            }
            // Cakupan baris: lewati decode/match di luar interval.
            if let Some((lo, hi)) = scope_lines {
                if line_no < lo || line_no > hi {
                    line_no += 1;
                    continue;
                }
            }
            let text = crate::engine::decode::decode_bytes(bytes, encoding);
            if ast.matches(&text, case_sensitive) {
                let (cs, ce) =
                    crate::engine::query::first_match_span(&text, &term_refs, case_sensitive);
                pending.push(Hit {
                    line: line_no,
                    byte: line_start + if bom_len > 0 && line_no == 1 { bom_len as u64 } else { 0 },
                    col_start: cs,
                    col_end: ce,
                });
                total_found += 1;
                if pending.len() >= search::SEARCH_BATCH {
                    let b = std::mem::take(&mut pending);
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: b,
                        done: false,
                        truncated: false,
                        error: None,
                        scanned,
                        total,
                    });
                }
                if total_found >= search::MAX_STORED_HITS {
                    truncated = true;
                    break;
                }
            }
            line_no += 1;
        }
        if gen_shared.load(Ordering::Relaxed) != gen {
            return;
        }
        let _ = tx.send(SearchBatchMsg {
            gen,
            batch: std::mem::take(&mut pending),
            done: true,
            truncated,
            error: None,
            scanned: total,
            total,
        });
    });
}

fn spawn_filter(    path: PathBuf,
    encoding: Encoding,
    _bom_len: usize,
    f: ParsedFilter,
    tx: mpsc::Sender<(Vec<u64>, bool)>,
) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let file = match std::fs::File::open(&path) {
            Ok(x) => x,
            Err(_) => {
                let _ = tx.send((Vec::new(), false));
                return;
            }
        };
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        let mut map: Vec<u64> = Vec::new();
        let mut line_no: u64 = 1;
        let mut buf: Vec<u8> = Vec::new();
        let cap = 5_000_000usize;
        let mut truncated = false;
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            // strip newline
            let mut line_bytes = &buf[..];
            if line_bytes.ends_with(b"\n") {
                line_bytes = &line_bytes[..line_bytes.len() - 1];
            }
            let ok = if encoding.is_wide() {
                // wide: decode raw (includes \r\n as units); rough but functional
                let s = crate::engine::decode::decode_bytes(line_bytes, encoding);
                crate::engine::filter::line_matches(&s, &f)
            } else {
                crate::engine::filter::line_bytes_match(line_bytes, encoding, &f)
            };
            if ok {
                map.push(line_no);
                if map.len() >= cap {
                    truncated = true;
                    break;
                }
            }
            line_no += 1;
        }
        let _ = tx.send((map, truncated));
    });
}

/// Jumlah bucket peta kepadatan ERROR/WARN (512 byte per file).
pub const MARKER_BUCKETS: usize = 512;

/// Histogram kepadatan ERROR per menit untuk shading strip
/// (klik strip tetap lompat posisi byte).
#[derive(Clone, Debug, Default)]
pub struct TimeHist {
    pub start_min: i64,
    pub step_min: i64,
    pub counts: Vec<u32>,
    pub first_line: Vec<u64>,
    pub partial: bool,
}

/// Kuantisasi sampel (epoch-menit, baris) menjadi bin adaptif (maks `max_bins`).
/// None bila data tak cukup untuk sumbu waktu (0/1 menit).
pub fn build_time_hist(samples: &[(i64, u64)], max_bins: usize) -> Option<TimeHist> {
    if samples.len() < 2 {
        return None;
    }
    let min = samples.iter().map(|(m, _)| *m).min()?;
    let max = samples.iter().map(|(m, _)| *m).max()?;
    if max <= min {
        return None;
    }
    let span = (max - min + 1) as usize;
    let nb = span.min(max_bins.max(1));
    let step = ((span + nb - 1) / nb).max(1) as i64;
    let n = ((span as i64 + step - 1) / step) as usize;
    let mut counts = vec![0u32; n];
    let mut first = vec![u64::MAX; n];
    for (m, ln) in samples {
        let b = ((m - min) / step) as usize;
        if b < n {
            counts[b] += 1;
            if *ln < first[b] {
                first[b] = *ln;
            }
        }
    }
    Some(TimeHist { start_min: min, step_min: step, counts, first_line: first, partial: false })
}

/// Pindai latar: tandai bucket byte yang mengandung ERROR/FATAL/Exception
/// (bit 0) atau WARN (bit 1). Satu pass memchr, ~detik untuk 2 GB.
/// Kasar per ±4 MB pada file 2 GB — cukup sebagai "peta masalah" strip.
fn spawn_marker_scan(
    path: PathBuf,
    total: u64,
    tx: mpsc::Sender<(Vec<u8>, u64, Option<TimeHist>)>,
) {
    std::thread::spawn(move || {
        use std::io::{BufRead, Read};
        let mut bits = vec![0u8; MARKER_BUCKETS];
        if total == 0 {
            let _ = tx.send((bits, total, None));
            return;
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => {
                let _ = tx.send((bits, total, None));
                return;
            }
        };
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        let err_fs = [
            memchr::memmem::Finder::new(b"ERROR"),
            memchr::memmem::Finder::new(b"FATAL"),
            memchr::memmem::Finder::new(b"Exception"),
            memchr::memmem::Finder::new(b"Caused by:"),
        ];
        let warn_f = memchr::memmem::Finder::new(b"WARN");
        let mut buf = vec![0u8; 1024 * 1024];
        let mut offset: u64 = 0;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            let has_err = err_fs.iter().any(|f| f.find(chunk).is_some());
            let has_warn = warn_f.find(chunk).is_some();
            if has_err || has_warn {
                let b0 = ((offset * MARKER_BUCKETS as u64) / total.max(1)) as usize;
                let b1 =
                    (((offset + n as u64) * MARKER_BUCKETS as u64) / total.max(1)) as usize;
                for b in b0..=b1.min(MARKER_BUCKETS - 1) {
                    if has_err {
                        bits[b] |= 0x01;
                    }
                    if has_warn {
                        bits[b] |= 0x02;
                    }
                }
            }
            offset += n as u64;
        }
        // Pass kedua: histogram ERROR per menit (garis ber-cap waktu saja).
        // Satu baca sekuensial tambahan; page cache masih hangat.
        let hist = time_hist_pass(&path);
        let _ = tx.send((bits, total, hist));
    });
}

/// Pass garis: kumpulkan (epoch-menit, baris) garis ERROR (maks 200 rb),
/// lalu kuantisasi menjadi histogram. None bila < 2 menit berbeda.
fn time_hist_pass(path: &PathBuf) -> Option<TimeHist> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
    let err_fs = [
        memchr::memmem::Finder::new(b"ERROR"),
        memchr::memmem::Finder::new(b"FATAL"),
        memchr::memmem::Finder::new(b"Exception"),
    ];
    let mut samples: Vec<(i64, u64)> = Vec::new();
    let mut line_no: u64 = 1;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if err_fs.iter().any(|f| f.find(&buf).is_some()) {
            let text = String::from_utf8_lossy(&buf);
            if let Some(ts) = Doc::parse_timestamp_prefix(text.trim_start()) {
                if samples.len() < 200_000 {
                    samples.push((ts / 60, line_no));
                }
            }
        }
        line_no += 1;
    }
    let mut h = build_time_hist(&samples, 1024)?;
    h.partial = samples.len() >= 200_000;
    Some(h)
}

// ---------- app ----------

pub struct AsisLogApp {
    tabs: Vec<TabState>,
    current: usize,
    global_error: Option<String>,
    global_status: String,
    tema: Tema,
    /// (Pilihan, dark) yang sedang diterapkan ke egui.
    tema_state: (Tema, bool),
    /// Preset pencarian milik user (bawaan dari store::builtin_presets).
    presets: Vec<Preset>,
    /// Set highlighter bernama; aturan efektif = set aktif.
    sets: Vec<HighlightSet>,
    active_set: Option<String>,
    /// Aturan terkompilasi untuk render viewport (dibangun ulang bila kotor).
    hl_compiled: Vec<CompiledRule>,
    hl_dirty: bool,
    // Dialog simpan preset.
    preset_save_open: bool,
    preset_save_name: String,
    /// True bila config global perlu ditulis (di-flush di luar pinjam tab).
    cfg_dirty: bool,
    // Jendela aturan sorotan + form tambah.
    hl_open: bool,
    hl_name: String,
    hl_set_name: String,
    hl_pattern: String,
    hl_regex: bool,
    hl_case: bool,
    hl_color_idx: usize,
    hl_whole: bool,
    // Dialog ubah label penanda.
    rename_open: bool,
    rename_line: u64,
    rename_label: String,
    rename_color: BookmarkColor,
    // Dialog rentang waktu.
    range_open: bool,
    range_start: String,
    range_end: String,
    range_msg: String,
    /// Riwayat file yang pernah dibuka (path string, terbaru dulu).
    recent: Vec<String>,
    /// File favorit (disematkan di menu Riwayat).
    favorites: Vec<String>,
    /// Jendela daftar pintasan (F1).
    shortcuts_open: bool,
    /// History pola pencarian global (config, maks 30).
    history: Vec<HistEntry>,
    /// True bila sesi perlu ditulis (debounce 10 dtk).
    session_dirty: bool,
    last_session_save: Instant,
    /// Zoom UI (1.0 normal). Diterapkan ke text style + tinggi baris.
    zoom: f32,
    /// Jendela scratchpad + isi + pesan.
    scratch_open: bool,
    scratch_text: String,
    scratch_msg: String,
    /// Dialog buka URL + unduhan berjalan.
    url_open: bool,
    url_text: String,
    dl_rx: Option<mpsc::Receiver<DlMsg>>,
    /// Dialog tempel teks.
    paste_open: bool,
    paste_text: String,
    /// Goto dialog: terapkan ke semua tab.
    goto_all: bool,
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
    fn apply_zoom(ctx: &egui::Context, zoom: f32) {
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
    fn row_h(&self) -> f32 {
        (viewer::ROW_H * self.zoom).round().max(14.0)
    }

    fn bump_zoom(&mut self, ctx: &egui::Context, next: f32) {
        self.zoom = next.clamp(0.7, 1.8);
        Self::apply_zoom(ctx, self.zoom);
        self.cfg_dirty = true;
        self.global_status = format!("Zoom {}%.", (self.zoom * 100.0).round() as u32);
    }

    /// Tulis config global. Galat di status; scratch dipotong 64 KB.
    fn save_config(&mut self) {
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

    /// Set aktif (mutable) untuk jendela sorotan.
    fn active_set_mut(&mut self) -> Option<&mut HighlightSet> {
        let name = self.active_set.clone()?;
        self.sets.iter_mut().find(|s| s.name == name)
    }

    /// Aturan efektif = aturan enabled milik set aktif.
    fn active_rules(&self) -> Vec<HighlightRule> {
        self.active_set
            .as_ref()
            .and_then(|n| self.sets.iter().find(|s| &s.name == n))
            .map(|s| s.rules.clone())
            .unwrap_or_default()
    }

    /// Tulis sesi workspace (tab + posisi + filter + follow).
    fn save_session(&mut self) {
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
    fn save_workspace_to(&mut self, path: &PathBuf) {
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
    fn open_workspace(&mut self, ws: crate::store::Workspace) {
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
    fn restore_session(&mut self) {        use crate::engine::decode::Encoding;
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
    fn toggle_label(&mut self, idx: usize) {
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
    fn rebuild_highlights(&mut self) {        self.hl_compiled = self
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
    fn save_marks(idx: usize, tabs: &mut [TabState]) {
        let Some(tab) = tabs.get_mut(idx) else { return };
        match crate::engine::marks::save(&tab.doc.path, &tab.doc.bookmarks) {
            Ok(()) => {}
            Err(e) => tab.doc.status = e,
        }
    }

    fn open_file(&mut self, path: PathBuf) {
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

    fn open_dialog(&mut self) {
        let f = rfd::FileDialog::new()
            .add_filter("Log", &["log", "txt", "out", "err"])
            .add_filter("Arsip", &["zip", "tgz", "gz", "tar"])
            .add_filter("Semua", &["*"])
            .pick_file();
        if let Some(p) = f {
            self.open_file(p);
        }
    }

    fn current_tab_mut(&mut self) -> Option<&mut TabState> {
        self.tabs.get_mut(self.current)
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = ctx.input(|i| i.modifiers.ctrl);
        let shift = ctx.input(|i| i.modifiers.shift);
        // Ctrl+O
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::O)) {
            self.open_dialog();
        }
        // F1: jendela daftar pintasan (berlaku tanpa tab).
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.shortcuts_open = true;
        }
        // Angka & n/N milik editor saat mengetik di kolom teks / dialog terbuka.
        // (Dihitung SEBELUM pinjam tab: current_tab_mut meminjam seluruh self.)
        let typing = {
            let typing_field = ["cari", "saring", "tandai-saring"]
                .iter()
                .any(|id| ctx.memory(|m| m.focused() == Some(egui::Id::new(*id))));
            let dialog_open = self.hl_open
                || self.preset_save_open
                || self.rename_open
                || self.range_open
                || self.shortcuts_open
                || self.url_open
                || self.paste_open
                || self.scratch_open
                || self
                    .tabs
                    .get(self.current)
                    .map(|t| t.goto_open || t.export_open || t.scope_open)
                    .unwrap_or(false);
            typing_field || dialog_open
        };
        let typing_field = typing;
        let Some(tab) = self.current_tab_mut() else { return };
        let mut sess_touch = false;
        // Ctrl+F fokus cari: kami tandai lewat status (fokus widget di bawah via id)
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("cari")));
        }
        // F3 / Shift+F3
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            if !tab.doc.hits.is_empty() {
                let n = tab.doc.hits.len();
                let cur = tab.current_hit.unwrap_or(0);
                let nxt = if shift {
                    cur.saturating_sub(1).min(n - 1)
                } else {
                    (cur + 1).min(n - 1)
                };
                tab.jump_to_hit(nxt);
            }
        }
        // Ctrl+G
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::G)) {
            tab.goto_open = true;
        }
        // Ctrl+E: dialog ekspor hasil
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::E)) {
            tab.export_open = true;
        }
        // Ctrl+Home / Ctrl+End
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::Home)) {
            tab.top_row = 0;
            tab.selected_line = tab.row_to_line(0).unwrap_or(1);
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::End)) {
            let total = tab.total_view_rows();
            tab.top_row = total.saturating_sub(60);
            tab.doc.stick_bottom = true;
        }
        // Ctrl+Shift+F toggle ikuti
        if ctrl && shift && ctx.input(|i| i.key_pressed(egui::Key::F)) {
            tab.doc.follow = !tab.doc.follow;
            tab.doc.stick_bottom = tab.doc.follow;
            sess_touch = true;
        }
        // Alt+Left / Alt+Right: history navigasi
        let alt = ctx.input(|i| i.modifiers.alt);
        if alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            if !tab.go_hist(true) {
                tab.doc.status = String::from("Tidak ada lokasi sebelumnya.");
            }
        }
        if alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            if !tab.go_hist(false) {
                tab.doc.status = String::from("Tidak ada lokasi berikutnya.");
            }
        }
        // Alt+Up / Alt+Down: penanda sebelumnya/berikutnya
        if alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            if !tab.go_mark(true) {
                tab.doc.status = String::from("Belum ada penanda.");
            }
        }
        if alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            if !tab.go_mark(false) {
                tab.doc.status = String::from("Belum ada penanda.");
            }
        }
        // Esc batal pencarian
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            tab.search_text.clear();
            tab.last_searched.clear();
            tab.doc.hits.clear();
            tab.doc.search_error = None;
            tab.doc.search_in_progress = false;
            tab.doc.search_gen += 1;
            tab.gen_shared.store(tab.doc.search_gen, Ordering::Relaxed);
            tab.current_hit = None;
            tab.results_collapsed = true;
            tab.goto_open = false;
            tab.export_open = false;
            tab.scope_open = false;
        }
        drop(tab);
        if sess_touch {
            self.session_dirty = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.preset_save_open = false;
            self.hl_open = false;
            self.rename_open = false;
            self.range_open = false;
        }
        // Ctrl+B: toggle penanda di baris aktif (+ simpan sidecar)
        if ctrl && !shift && ctx.input(|i| i.key_pressed(egui::Key::B)) {
            let idx = self.current;
            if let Some(t) = self.tabs.get_mut(idx) {
                let ln = t.selected_line;
                t.doc.toggle_bookmark(ln);
                t.refresh_mode_map();
            }
            Self::save_marks(idx, &mut self.tabs);
        }
        // Ctrl+Shift+B: buka/tutup panel penanda
        if ctrl && shift && ctx.input(|i| i.key_pressed(egui::Key::B)) {
            if let Some(t) = self.tabs.get_mut(self.current) {
                t.show_bookmarks = !t.show_bookmarks;
            }
        }
        // F2: ubah label penanda di baris aktif
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            let idx = self.current;
            let found = self.tabs.get(idx).and_then(|t| {
                let ln = t.selected_line;
                t.doc.bookmarks.iter().find(|b| b.line == ln).map(|b| {
                    (ln, b.label.clone(), b.color)
                })
            });
            match found {
                Some((ln, label, color)) => {
                    self.rename_line = ln;
                    self.rename_label = label;
                    self.rename_color = color;
                    self.rename_open = true;
                }
                None => {
                    if let Some(t) = self.tabs.get_mut(idx) {
                        t.doc.status = String::from(
                            "Tidak ada penanda di baris aktif. Tekan Ctrl+B dulu.",
                        );
                    }
                }
            }
        }
        // Ctrl+Tab / Ctrl+Shift+Tab: pindah tab (bukan saat mengetik).
        if ctrl && !alt && !typing_field && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let n = self.tabs.len();
            if n > 1 {
                if shift {
                    self.current = (self.current + n - 1) % n;
                } else {
                    self.current = (self.current + 1) % n;
                }
                self.session_dirty = true;
            }
        }
        // Zoom Ctrl+= / Ctrl+- / Ctrl+0.
        if ctrl && !shift && !alt {
            if ctx.input(|i| i.key_pressed(egui::Key::Equals)) {
                self.bump_zoom(ctx, self.zoom + 0.1);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.bump_zoom(ctx, self.zoom - 0.1);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
                self.bump_zoom(ctx, 1.0);
            }
        }
        // Label warna 1-9 dari query aktif (bukan saat mengetik).
        if !typing && !ctrl && !alt {
            const NUMS: [egui::Key; 9] = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ];
            for (i, k) in NUMS.iter().enumerate() {
                if ctx.input(|ii| ii.key_pressed(*k)) {
                    self.toggle_label(i);
                    break;
                }
            }
            // n / N: hasil berikut/sebelum tanpa panel.
            if ctx.input(|i| i.key_pressed(egui::Key::N)) {
                if let Some(t) = self.tabs.get_mut(self.current) {
                    if !t.doc.hits.is_empty() {
                        let n = t.doc.hits.len();
                        let c = t.current_hit.unwrap_or(0);
                        if shift {
                            t.jump_to_hit(c.saturating_sub(1).min(n - 1));
                        } else {
                            t.jump_to_hit((c + 1).min(n - 1));
                        }
                    }
                }
            }
        }
    }

    fn goto_execute(&mut self, tab_idx: usize) {
        let input = self.tabs[tab_idx].goto_input.clone();
        let all = self.goto_all && self.tabs.len() > 1;
        // Terapkan ke tab kini + (opsional) semua tab lain untuk korelasi.
        let targets: Vec<usize> = if all {
            (0..self.tabs.len()).collect()
        } else {
            vec![tab_idx]
        };
        let mut ok = 0;
        let mut fail = 0;
        for i in targets {
            let tab = &mut self.tabs[i];
            let r: Result<(), String> = match parse_goto(&input) {
                Ok(GotoTarget::Line(n)) => {
                    let max = if tab.doc.index.complete {
                        tab.doc.index.total_lines
                    } else {
                        tab.doc.line_count_estimate()
                    };
                    // `akhir` dikirim sebagai u64::MAX: selesaikan ke baris terakhir.
                    let n = if n == u64::MAX { max.max(1) } else { n };
                    if n > max && tab.doc.index.complete {
                        Err(format!("Baris melebihi {} baris.", format_count(max)))
                    } else {
                        tab.nav_to(n);
                        Ok(())
                    }
                }
                Ok(GotoTarget::Percent(p)) => {
                    let ln = tab.doc.goto_percent_line(p);
                    tab.nav_to(ln);
                    Ok(())
                }
                Ok(GotoTarget::Timestamp(ts)) => match goto_timestamp(tab, &ts) {
                    Ok(ln) => {
                        tab.nav_to(ln);
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            match r {
                Ok(()) => {
                    tab.goto_msg.clear();
                    tab.goto_open = false;
                    ok += 1;
                }
                Err(e) => {
                    // Pada mode semua-tab, tab gagal tetap lanjut; pesan di tab kini.
                    tab.goto_open = !all;
                    if i == tab_idx {
                        tab.goto_msg = e;
                    }
                    fail += 1;
                }
            }
        }
        if all {
            self.global_status = format!("Lompat ke semua tab: {} ok, {} gagal.", ok, fail);
        }
    }
}

/// Timestamp jump: binary search bila monotonik, else linear dengan timeout.
fn goto_timestamp(tab: &mut TabState, ts: &str) -> Result<u64, String> {
    // Parse target as unix seconds; if input is not full prefix, try to find
    // comparable by scanning? We require parseable prefix.
    let target = Doc::parse_timestamp_prefix(ts).or_else(|| {
        // Coba tempel tanggal? minimal: "13:41:02" tidak cukup -> tolak.
        None
    });
    let target = match target {
        Some(t) => t,
        None => {
            // Fallback: cari substring linear (timeout 3 dtk).
            return goto_substring_linear(tab, ts);
        }
    };
    let total = if tab.doc.index.complete {
        tab.doc.index.total_lines
    } else {
        tab.doc.line_count_estimate().min(2_000_000)
    };
    if total == 0 {
        return Err(String::from("File kosong."));
    }
    // Sample first/last parsed times.
    let first = (1..=total.min(200))
        .filter_map(|ln| tab.doc.get_line_text(ln))
        .filter_map(|s| Doc::parse_timestamp_prefix(&s))
        .next();
    let last = (total.saturating_sub(200).max(1)..=total)
        .rev()
        .filter_map(|ln| tab.doc.get_line_text(ln))
        .filter_map(|s| Doc::parse_timestamp_prefix(&s))
        .next();
    let monotonic = match (first, last) {
        (Some(a), Some(b)) => a <= b,
        _ => false,
    };
    if monotonic {
        // binary search lines
        let mut lo = 1u64;
        let mut hi = total;
        let mut best: Option<u64> = None;
        let start = Instant::now();
        while lo <= hi {
            if start.elapsed() > Duration::from_secs(5) {
                break;
            }
            let mid = lo + (hi - lo) / 2;
            let t = tab
                .doc
                .get_line_text(mid)
                .and_then(|s| Doc::parse_timestamp_prefix(&s));
            match t {
                Some(v) if v < target => lo = mid + 1,
                Some(v) if v > target => {
                    best = Some(mid);
                    hi = mid.saturating_sub(1);
                    if hi == 0 {
                        break;
                    }
                }
                Some(_) => return Ok(mid),
                None => {
                    // baris tanpa cap waktu: cari tetangga terdekat (langkah kecil)
                    // Sederhana: geser lo maju.
                    lo = mid + 1;
                    if lo > total {
                        break;
                    }
                }
            }
        }
        best.ok_or_else(|| String::from("Cap waktu tidak ditemukan."))
    } else {
        // linear dengan timeout
        let start = Instant::now();
        let mut ln = 1u64;
        while ln <= total {
            if start.elapsed() > Duration::from_secs(3) {
                return Err(String::from(
                    "Pencarian waktu kehabisan waktu — hasil sebagian tidak ditemukan.",
                ));
            }
            if let Some(s) = tab.doc.get_line_text(ln) {
                if let Some(v) = Doc::parse_timestamp_prefix(&s) {
                    if v >= target {
                        return Ok(ln);
                    }
                }
            }
            ln += 1;
            if ln > 500_000 {
                break;
            }
        }
        Err(String::from("Cap waktu tidak ditemukan."))
    }
}

fn goto_substring_linear(tab: &mut TabState, needle: &str) -> Result<u64, String> {
    let start = Instant::now();
    let total = if tab.doc.index.complete {
        tab.doc.index.total_lines.min(1_000_000)
    } else {
        200_000
    };
    for ln in 1..=total {
        if start.elapsed() > Duration::from_secs(3) {
            return Err(String::from("Pencarian kehabisan waktu."));
        }
        if let Some(s) = tab.doc.get_line_text(ln) {
            if s.contains(needle) {
                return Ok(ln);
            }
        }
    }
    Err(String::from("Tidak ditemukan."))
}

/// Selesaikan blok pada baris aktif: 0 = SQL, 1 = transaksi, 2 = checkpoint.
fn resolve_block(tab: &mut TabState, which: u8) -> Result<(u64, u64, String), String> {
    match which {
        0 => tab
            .doc
            .sql_block_range(tab.selected_line)
            .map(|(a, b)| {
                (
                    a,
                    b,
                    format!("Blok SQL baris {}–{}", format_count(a), format_count(b)),
                )
            }),
        1 => tab
            .doc
            .block_range(tab.selected_line, BlockKind::Transaction),
        _ => tab
            .doc
            .block_range(tab.selected_line, BlockKind::Checkpoint),
    }
}

fn copy_block(tab: &mut TabState, ctx: &egui::Context, which: u8) {
    match resolve_block(tab, which) {
        Ok((a, b, desc)) => match tab.doc.copy_range_text(a, b) {
            Ok(s) => {
                ctx.copy_text(s);
                tab.doc.status = format!("{} disalin.", desc);
            }
            Err(e) => tab.doc.status = e,
        },
        Err(e) => tab.doc.status = e,
    }
}

fn export_block(tab: &mut TabState, which: u8, fname: &str) {    let (a, b, desc) = match resolve_block(tab, which) {
        Ok(v) => v,
        Err(e) => {
            tab.doc.status = e;
            return;
        }
    };
    let Some(p) = rfd::FileDialog::new().set_file_name(fname).save_file() else {
        return;
    };
    match tab.doc.export_range_to_file(&p, a, b) {
        Ok(n) => {
            tab.doc.status = format!("{} diekspor ({} baris) ke {}.", desc, n, p.display())
        }
        Err(e) => tab.doc.status = e,
    }
}

/// Salin N baris dari baris aktif dengan nomor baris ("ln: teks").
fn copy_numbered(tab: &mut TabState, ctx: &egui::Context, count: u64) {
    let a = tab.selected_line;
    let mut out = String::new();
    let mut over = false;
    for ln in a..a.saturating_add(count.max(1)) {
        let Some(t) = tab.doc.get_line_text(ln) else { break };
        let row = format!("{}: {}\n", ln, t);
        if out.len() + row.len() > crate::engine::COPY_CAP_BYTES {
            over = true;
            break;
        }
        out.push_str(&row);
    }
    if out.is_empty() {
        tab.doc.status = String::from("Tidak ada baris untuk disalin.");
    } else {
        ctx.copy_text(out);
        tab.doc.status = if over {
            String::from("Disalin sebagian (16 MB).")
        } else {
            String::from("Disalin dengan nomor baris.")
        };
    }
}

/// Unduh URL ke temp di thread latar (lapor via channel).
fn spawn_download(url: String, tx: mpsc::Sender<DlMsg>) {
    std::thread::spawn(move || {
        let t = url.trim();
        if !(t.starts_with("http://") || t.starts_with("https://")) {
            let _ = tx.send(DlMsg::Failed(String::from("URL harus http:// atau https://.")));
            return;
        }
        let leaf = t
            .rsplit(['/', '?'])
            .next()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        let leaf: String = leaf
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .take(80)
            .collect();
        let leaf = if leaf.is_empty() { String::from("unduhan.log") } else { leaf };
        let mut dir = std::env::temp_dir();
        dir.push("asislog-dl");
        if std::fs::create_dir_all(&dir).is_err() {
            let _ = tx.send(DlMsg::Failed(String::from("Gagal membuat direktori temp.")));
            return;
        }
        let out = dir.join(leaf);
        let resp = match ureq::get(t).call() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(DlMsg::Failed(format!("Gagal mengunduh: {}", e)));
                return;
            }
        };
        let mut body = resp.into_body().into_reader();
        let mut f = match std::fs::File::create(&out) {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(DlMsg::Failed(format!("Gagal menulis temp: {}", e)));
                return;
            }
        };
        match std::io::copy(&mut body, &mut f) {
            Ok(_) => {
                let _ = tx.send(DlMsg::Done(out));
            }
            Err(e) => {
                let _ = tx.send(DlMsg::Failed(format!("Gagal mengunduh: {}", e)));
            }
        }
    });
}

/// Warna dot penanda, sadar-tema.
fn mark_color(c: BookmarkColor, dark: bool) -> egui::Color32 {
    match c {
        BookmarkColor::Default => viewer::gutter_color(dark),
        BookmarkColor::Blue => egui::Color32::from_rgb(90, 160, 255),
        BookmarkColor::Green => egui::Color32::from_rgb(90, 220, 120),
        BookmarkColor::Yellow => egui::Color32::from_rgb(255, 200, 60),
        BookmarkColor::Red => egui::Color32::from_rgb(255, 110, 110),
        BookmarkColor::Purple => egui::Color32::from_rgb(200, 150, 255),
    }
}

fn highlight_keys() -> &'static [(&'static str, &'static str)] {
    crate::store::highlight_color_names()
}
fn key_index(k: &str) -> usize {
    highlight_keys()
        .iter()
        .position(|(kk, _)| *kk == k)
        .unwrap_or(0)
}

fn color_name_id(key: &str) -> &'static str {
    highlight_keys()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, n)| *n)
        .unwrap_or("Kuning")
}

/// Nama file aman dari nama set (ASCII saja, maks 40 char).
fn sanitize_name(s: &str) -> String {
    let out: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == ' ')
        .take(40)
        .collect();
    let out = out.trim().replace(' ', "-").to_lowercase();
    if out.is_empty() {
        String::from("highlight")
    } else {
        out
    }
}

/// Terapkan rentang waktu sebagai filter baris [lo, hi] (maks 2 juta baris).
/// Mengembalikan pesan status Indonesia. Baris tanpa timestamp dilewati
/// oleh pencarian biner/linear di `goto_timestamp`.
fn apply_time_range(tab: &mut TabState, start: &str, end: &str) -> Result<String, String> {
    let t0 = Doc::parse_timestamp_prefix(start.trim()).ok_or_else(|| {
        String::from("Waktu awal tidak valid. Contoh: 2026-08-24 13:00:00")
    })?;
    let t1 = Doc::parse_timestamp_prefix(end.trim())
        .ok_or_else(|| String::from("Waktu akhir tidak valid."))?;
    if t1 < t0 {
        return Err(String::from("Waktu akhir harus setelah waktu awal."));
    }
    let total = if tab.doc.index.complete {
        tab.doc.index.total_lines
    } else {
        tab.doc.line_count_estimate().min(2_000_000)
    };
    if total == 0 {
        return Err(String::from("File kosong."));
    }
    let lo = goto_timestamp(tab, start.trim())?;
    // Batas atas: baris pertama >= t1 (atau total bila tak ketemu).
    let mut hi = match goto_timestamp(tab, end.trim()) {
        Ok(l) => {
            // Samakan presisi: bila baris l tepat == t1, ikutkan; bila sudah
            // melewati t1, mundur satu baris.
            match tab
                .doc
                .get_line_text(l)
                .and_then(|s| Doc::parse_timestamp_prefix(&s))
            {
                Some(v) if v > t1 => l.saturating_sub(1).max(lo),
                _ => l,
            }
        }
        Err(_) => total,
    };
    hi = hi.min(total);
    if hi < lo {
        return Err(String::from("Rentang kosong pada file ini."));
    }
    const CAP: u64 = 2_000_000;
    let mut truncated = false;
    if hi - lo + 1 > CAP {
        hi = lo + CAP - 1;
        truncated = true;
    }
    tab.doc.filter_map = (lo..=hi).collect();
    tab.doc.filter_active = true;
    tab.doc.filter = parse_filter("", tab.case_sensitive);
    tab.doc.filter.raw = format!("waktu {} s.d. {}", start.trim(), end.trim());
    tab.top_row = 0;
    tab.nav_to(lo);
    Ok(format!(
        "Rentang waktu: {} baris{}.",
        format_count(hi - lo + 1),
        if truncated { " (dibatasi 2 jt)" } else { "" }
    ))
}

impl eframe::App for AsisLogApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Tulis sesi + config saat keluar (best effort, tanpa panel).
        self.save_session();
        if self.cfg_dirty {
            self.save_config();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drag-drop
        let dropped: Vec<PathBuf> = ctx
            .input(|i| i.raw.dropped_files.clone())
            .into_iter()
            .filter_map(|f| f.path)
            .collect();
        for p in dropped {
            self.open_file(p);
        }

        self.handle_shortcuts(ctx);

        // Terapkan tema pilihan (hanya saat berubah agar hemat).
        // `Sistem` dibaca dari OS tiap frame sehingga mengikuti perubahan OS.
        let (vis, dark_now) = self.tema.resolve(ctx);
        if (self.tema, dark_now) != self.tema_state {
            ctx.set_visuals(vis);
            self.tema_state = (self.tema, dark_now);
        }
        // Bangun ulang cache sorotan bila aturan berubah.
        if self.hl_dirty {
            self.rebuild_highlights();
        }

        // Poll background per tab (sebelum pinjam mut untuk UI)
        // History dipinjam terpisah (field lain) agar start_search bisa mencatat.
        let history = &mut self.history;
        let mut session_touched = false;
        for t in self.tabs.iter_mut() {
            t.poll_channels();
            t.poll_follow();
            // Penanda berubah -> tulis sidecar (best effort).
            if t.marks_dirty {
                if let Err(e) = crate::engine::marks::save(&t.doc.path, &t.doc.bookmarks) {
                    t.doc.status = e;
                }
                t.marks_dirty = false;
            }
            // debounce pencarian 150 ms
            if let Some(at) = t.debounce_at {
                if Instant::now() >= at {
                    t.start_search(history);
                    session_touched = true;
                }
            }
            // Posisi gulir berubah -> sesi perlu disimpan ulang (debounce).
            if t.top_row != t.saved_top {
                t.saved_top = t.top_row;
                session_touched = true;
            }
        }
        if session_touched {
            self.session_dirty = true;
        }
        // Flush sesi (debounce 10 dtk) + config kotor.
        if self.session_dirty && self.last_session_save.elapsed() > Duration::from_secs(10) {
            self.save_session();
        }
        if self.cfg_dirty {
            self.save_config();
        }
        // Unduhan URL selesai? Kumpulkan dulu agar tak pinjam saat open_file.
        let dl_done: Vec<DlMsg> = if let Some(rx) = &self.dl_rx {
            rx.try_iter().collect()
        } else {
            Vec::new()
        };
        if !dl_done.is_empty() {
            self.dl_rx = None;
            for m in dl_done {
                match m {
                    DlMsg::Done(p) => self.open_file(p),
                    DlMsg::Failed(e) => self.global_status = e,
                }
            }
        }

        // ---- bar bilah atas: sesi file ----
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Buka").clicked() {
                    self.open_dialog();
                }
                // Riwayat file + favorit.
                ui.menu_button("Riwayat v", |ui| {
                    let mut open: Option<PathBuf> = None;
                    let mut missing_recent: Option<String> = None;
                    let mut fav_toggle: Option<String> = None;
                    if !self.favorites.is_empty() {
                        ui.label("Favorit:");
                        for r in self.favorites.clone() {
                            ui.horizontal(|ui| {
                                let name = PathBuf::from(&r)
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| r.clone());
                                if ui.button(name).on_hover_text(r.clone()).clicked() {
                                    let p = PathBuf::from(&r);
                                    if p.exists() {
                                        open = Some(p);
                                    } else {
                                        self.global_status = format!(
                                            "File favorit tak ditemukan: {}",
                                            r
                                        );
                                    }
                                    ui.close();
                                }
                                if ui
                                    .small_button("F")
                                    .on_hover_text("Lepas favorit")
                                    .clicked()
                                {
                                    fav_toggle = Some(r);
                                }
                            });
                        }
                        ui.separator();
                    }
                    ui.label("Terakhir dibuka:");
                    if self.recent.is_empty() {
                        ui.label("Belum ada riwayat.");
                    }
                    for r in self.recent.clone() {
                        ui.horizontal(|ui| {
                            let name = PathBuf::from(&r)
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| r.clone());
                            if ui.button(name).on_hover_text(r.clone()).clicked() {
                                let p = PathBuf::from(&r);
                                if p.exists() {
                                    open = Some(p);
                                } else {
                                    missing_recent = Some(r.clone());
                                }
                                ui.close();
                            }
                            let is_fav = self.favorites.iter().any(|f| f == &r);
                            if ui
                                .small_button(if is_fav { "[F]" } else { "F" })
                                .on_hover_text("Jadikan favorit")
                                .clicked()
                            {
                                fav_toggle = Some(r);
                            }
                        });
                    }
                    if let Some(m) = missing_recent {
                        self.recent.retain(|p| p != &m);
                        self.save_config();
                        self.global_status =
                            format!("File tidak ditemukan, dihapus dari riwayat: {}", m);
                    }
                    if let Some(f) = fav_toggle {
                        if self.favorites.iter().any(|x| x == &f) {
                            self.favorites.retain(|x| x != &f);
                        } else {
                            self.favorites.insert(0, f);
                            self.favorites.truncate(20);
                        }
                        self.save_config();
                    }
                    ui.separator();
                    if ui.button("Bersihkan riwayat").clicked() {
                        self.recent.clear();
                        self.save_config();
                        ui.close();
                    }
                    if let Some(p) = open {
                        self.open_file(p);
                    }
                });
                // Buka dari URL / tempel teks (pekerjaan support).
                ui.menu_button("URL/teks v", |ui| {
                    if ui
                        .button("Buka URL…")
                        .on_hover_text("Unduh http(s) ke temp lalu buka")
                        .clicked()
                    {
                        self.url_open = true;
                        ui.close();
                    }
                    if ui
                        .button("Tempel teks…")
                        .on_hover_text("Tempel teks (Ctrl+V) lalu buka sebagai file")
                        .clicked()
                    {
                        self.paste_open = true;
                        ui.close();
                    }
                });
                // Workspace produk: N log + filter + set sorotan + rentang waktu.
                ui.menu_button("Workspace v", |ui| {
                    if ui
                        .button("Simpan workspace…")
                        .on_hover_text("Simpan tab + filter + set sorotan ke 1 file JSON")
                        .clicked()
                    {
                        let def = "workspace-asislog.json".to_string();
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .set_file_name(def)
                            .save_file()
                        {
                            self.save_workspace_to(&p);
                        }
                        ui.close();
                    }
                    if ui
                        .button("Buka workspace…")
                        .on_hover_text("Buka N log + filter + set sorotan dari file")
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                        {
                            match crate::store::load_workspace(&p) {
                                Ok(ws) => self.open_workspace(ws),
                                Err(e) => self.global_status = e,
                            }
                        }
                        ui.close();
                    }
                });
                if self.tabs.is_empty() {
                    ui.label("Belum ada file. Seret .log / .txt ke sini atau tekan Buka.");
                }
                // Tema + bantuan di kanan baris sesi.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("?").on_hover_text("Daftar pintasan (F1)").clicked() {
                        self.shortcuts_open = true;
                    }
                    egui::ComboBox::from_label("Tema")
                        .selected_text(self.tema.nama())
                        .show_ui(ui, |ui| {
                            for t in Tema::semua() {
                                if ui.selectable_label(self.tema == *t, t.nama()).clicked() {
                                    self.tema = *t;
                                    self.cfg_dirty = true;
                                }
                            }
                        });
                });
            });
            // Baris tab (gulir mendatar bila banyak, tidak terpotong).
            if !self.tabs.is_empty() {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut close_idx: Option<usize> = None;
                            for (i, t) in self.tabs.iter().enumerate() {
                                let sel = i == self.current;
                                let name = if sel {
                                    format!("* {}", t.doc.file_name)
                                } else {
                                    t.doc.file_name.clone()
                                };
                                if ui.selectable_label(sel, name).clicked() {
                                    self.current = i;
                                }
                                if ui.small_button("×").clicked() {
                                    close_idx = Some(i);
                                }
                            }
                            if let Some(i) = close_idx {
                                self.tabs[i].index_cancel.store(true, Ordering::Relaxed);
                                self.tabs[i].search_cancel.store(true, Ordering::Relaxed);
                                let tab = self.tabs.remove(i);
                                // Bersihkan temp ekstrak arsip (best effort).
                                if let Some(t) = tab.temp_path {
                                    let _ = std::fs::remove_file(&t);
                                    if let Some(dir) = t.parent() {
                                        let _ = std::fs::remove_dir(dir);
                                    }
                                }
                                if self.current >= self.tabs.len() && !self.tabs.is_empty() {
                                    self.current = self.tabs.len() - 1;
                                }
                                self.session_dirty = true;
                            }
                        });
                    });
            }
        });

        if self.tabs.is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading("AsisLog");
                    ui.label("Buka file log…");
                    ui.label("Penampil portabel untuk file .log / .txt / .out yang sangat besar.");
                    ui.add_space(12.0);
                    if ui.button("Buka file log…").clicked() {
                        self.open_dialog();
                    }
                    if let Some(e) = &self.global_error {
                        ui.colored_label(egui::Color32::RED, e);
                    }
                    ui.label(&self.global_status);
                });
            });
            ctx.request_repaint_after(Duration::from_millis(400));
            return;
        }

        let cur_idx = self.current.min(self.tabs.len() - 1);
        // Toolbar konteks tab aktif (pinjam singkat)
        let (search_text_clone, filter_text_clone) = {
            let t = &self.tabs[cur_idx];
            (t.search_text.clone(), t.filter_text.clone())
        };
        let _ = (search_text_clone, filter_text_clone);

        // ---- Baris 1: file & navigasi ----
        let mut sess_touch = false;
        egui::TopBottomPanel::top("tools").show(ctx, |ui| {
            let tab = &mut self.tabs[cur_idx];
            ui.horizontal_wrapped(|ui| {
                // Encoding override (tanpa label teks: kotaknya menjelaskan sendiri).
                egui::ComboBox::from_id_salt("enc")
                    .selected_text(tab.doc.encoding().label())
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(tab.doc.encoding_override.is_none(), "Otomatis")
                            .clicked()
                        {
                            tab.doc.set_encoding_override(None);
                            tab.search_cache.clear();
                            sess_touch = true;
                        }
                        for e in Encoding::all() {
                            if ui
                                .selectable_label(tab.doc.encoding_override == Some(*e), e.label())
                                .clicked()
                            {
                                tab.doc.set_encoding_override(Some(*e));
                                tab.search_cache.clear();
                                sess_touch = true;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Encoding file (Otomatis = deteksi BOM + sampel)");
                if ui
                    .button("Ke baris…")
                    .on_hover_text("Ke nomor baris, persen, akhir, atau cap waktu (Ctrl+G)")
                    .clicked()
                {
                    tab.goto_open = true;
                }
                if icon_button(ui, Icon::ChevronLeft, "Kembali ke lokasi sebelumnya (Alt+Left)")
                    .clicked()
                {
                    if !tab.go_hist(true) {
                        tab.doc.status = String::from("Tidak ada lokasi sebelumnya.");
                    }
                }
                if icon_button(ui, Icon::ChevronRight, "Maju ke lokasi berikutnya (Alt+Right)")
                    .clicked()
                {
                    if !tab.go_hist(false) {
                        tab.doc.status = String::from("Tidak ada lokasi berikutnya.");
                    }
                }
                if ui.button("Penanda").clicked() {
                    tab.show_bookmarks = !tab.show_bookmarks;
                }
                if ui
                    .button("Ekspor hasil…")
                    .on_hover_text("Simpan hasil pencarian ke file baru")
                    .clicked()
                {
                    tab.export_open = true;
                }
                if ui
                    .button("Sorotan…")
                    .on_hover_text("Aturan highlight kustom (hanya viewport)")
                    .clicked()
                {
                    self.hl_open = true;
                }
                if ui
                    .button("Catatan")
                    .on_hover_text("Scratchpad: catatan + base64/JWT/JSON/SQL")
                    .clicked()
                {
                    self.scratch_open = true;
                }
                // Toggle LIVE (mode) di kanan baris navigasi.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let live = tab.doc.follow;
                    if ui
                        .add(egui::Button::selectable(
                            live,
                            if live { "LIVE" } else { "Ikuti akhir file" },
                        ))
                        .on_hover_text("Pantau akhir file / tail (Ctrl+Shift+F)")
                        .clicked()
                    {
                        tab.doc.follow = !live;
                        tab.doc.stick_bottom = tab.doc.follow;
                        tab.last_follow_poll = Instant::now();
                        sess_touch = true;
                    }
                });
            });
        });
        if sess_touch {
            self.session_dirty = true;
        }

        // ---- Baris 2: pencarian (pusat UI, full-width) ----
        let mut open_range = false;
        let mut filter_all = false;
        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            // Pinjam terpisah agar closure menu tak konflik.
            let tab = &mut self.tabs[cur_idx];
            let presets = &mut self.presets;
            let history = &mut self.history;
            let cfg_dirty = &mut self.cfg_dirty;
            let preset_save_open = &mut self.preset_save_open;
            let preset_save_name = &mut self.preset_save_name;
            let mut search_focused = false;
            ui.horizontal(|ui| {
                // Menu preset pencarian (bawaan + simpanan user).
                ui.menu_button("Preset v", |ui| {                    ui.label("Bawaan:");
                    for p in crate::store::builtin_presets() {
                        if ui.button(p.name.clone()).clicked() {
                            tab.search_text = p.query.clone();
                            tab.regex_on = p.regex;
                            tab.case_sensitive = p.case_sensitive;
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if presets.is_empty() {
                        ui.label("Belum ada simpanan.");
                    }
                    let mut del: Option<usize> = None;
                    let mut apply: Option<Preset> = None;
                    for (i, p) in presets.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button(p.name.clone()).clicked() {
                                apply = Some(p.clone());
                                ui.close();
                            }
                            if ui.small_button("×").on_hover_text("Hapus preset").clicked() {
                                del = Some(i);
                            }
                        });
                    }
                    if let Some(i) = del {
                        presets.remove(i);
                        *cfg_dirty = true;
                    }
                    if let Some(p) = apply {
                        tab.search_text = p.query.clone();
                        tab.regex_on = p.regex;
                        tab.case_sensitive = p.case_sensitive;
                        tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                    }
                    ui.separator();
                    if ui.button("Simpan pencarian saat ini…").clicked() {
                        *preset_save_name = tab.search_text.clone();
                        *preset_save_open = true;
                        ui.close();
                    }
                });
                let clear_w = 30.0;
                let w = (ui.available_width() - clear_w - 8.0).max(120.0);
                let resp = ui.add_sized(
                    egui::vec2(w, 0.0),
                    egui::TextEdit::singleline(&mut tab.search_text)
                        .id_source("cari")
                        .hint_text("Cari teks, exception, request ID, atau regex…"),
                );
                search_focused = resp.has_focus();
                if resp.changed() {
                    tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                }
                if ui
                    .add_sized(egui::vec2(clear_w, 0.0), egui::Button::new("×"))
                    .on_hover_text("Bersihkan pencarian (Esc)")
                    .clicked()
                {
                    tab.search_text.clear();
                    tab.last_searched.clear();
                    tab.doc.hits.clear();
                    tab.doc.search_error = None;
                    tab.doc.search_in_progress = false;
                    tab.current_hit = None;
                    tab.results_collapsed = true;
                }
            });
            // Baris chip boolean: tampil bila query butuh logika AND/OR/NOT.
            // Hapus chip = buang token itu dari query, lalu cari ulang.
            if !tab.search_text.trim().is_empty()
                && !tab.regex_on
                && crate::engine::query::is_boolean_query(&tab.search_text)
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Logika:");
                    match crate::engine::query::top_spans(&tab.search_text) {
                        Some(spans) => {
                            let mut remove: Option<(usize, usize)> = None;
                            for (txt, a, b) in spans {
                                ui.label(format!("[{}]", txt));
                                if ui
                                    .small_button("×")
                                    .on_hover_text(format!("Hapus \"{}\" dari query", txt))
                                    .clicked()
                                {
                                    remove = Some((a, b));
                                }
                            }
                            if let Some((a, b)) = remove {
                                let q = tab.search_text.clone();
                                let rest = format!(
                                    "{} {}",
                                    q[..a].trim_end(),
                                    q[b..].trim_start()
                                );
                                tab.search_text = rest.trim().to_string();
                                tab.debounce_at =
                                    Some(Instant::now() + Duration::from_millis(150));
                            }
                        }
                        None => {
                            ui.label("Ekspresi kompleks (OR/kurung) dievaluasi penuh.");
                        }
                    }
                });
            }
            // Baris autocomplete history: tampil saat kolom fokus + ada yang cocok.
            if search_focused && !tab.search_text.trim().is_empty() {
                let sug = crate::store::suggest_history(history, tab.search_text.trim(), 6);
                if !sug.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Riwayat:");
                        let mut apply: Option<HistEntry> = None;
                        for h in sug {
                            let short: String = h.query.chars().take(40).collect();
                            let tag = if h.regex { " .*" } else { "" };
                            if ui.small_button(format!("{}{}", short, tag)).clicked() {
                                apply = Some(h.clone());
                            }
                        }
                        if let Some(h) = apply {
                            tab.search_text = h.query.clone();
                            tab.regex_on = h.regex;
                            tab.case_sensitive = h.case_sensitive;
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                        }
                    });
                }
            }
            ui.horizontal_wrapped(|ui| {
                // Toggle peka-huruf dan regex dengan state visual tegas.
                let cs = tab.case_sensitive;
                if ui
                    .add(egui::Button::selectable(cs, "Aa"))
                    .on_hover_text("Peka huruf besar/kecil (case sensitive)")
                    .clicked()
                {
                    tab.case_sensitive = !cs;
                }
                let rx = tab.regex_on;
                if ui
                    .add(egui::Button::selectable(rx, ".*"))
                    .on_hover_text("Perlakukan query sebagai regex")
                    .clicked()
                {
                    tab.regex_on = !rx;
                }
                ui.separator();
                if ui
                    .button("‹ Sebelumnya")
                    .on_hover_text("Hasil sebelumnya (Shift+F3)")
                    .clicked()
                {
                    if !tab.doc.hits.is_empty() {
                        let n = tab.doc.hits.len();
                        let c = tab.current_hit.unwrap_or(0);
                        tab.jump_to_hit(c.saturating_sub(1).min(n - 1));
                    }
                }
                if ui
                    .button("Berikutnya ›")
                    .on_hover_text("Hasil berikutnya (F3)")
                    .clicked()
                {
                    if !tab.doc.hits.is_empty() {
                        let n = tab.doc.hits.len();
                        let c = tab.current_hit.unwrap_or(0);
                        tab.jump_to_hit((c + 1).min(n - 1));
                    }
                }
                // Info hasil / progress / 0-hasil yang menjelaskan.
                if tab.doc.search_in_progress {
                    if tab.search_total > 0 {
                        ui.label(format!(
                            "Mencari… {} / {} · {} hasil",
                            format_size(tab.search_scanned),
                            format_size(tab.search_total),
                            format_count(tab.doc.hits.len() as u64),
                        ));
                    } else {
                        ui.label(format!(
                            "Mencari… {} hasil",
                            format_count(tab.doc.hits.len() as u64)
                        ));
                    }
                } else if let Some(e) = &tab.doc.search_error.clone() {
                    ui.colored_label(egui::Color32::RED, e);
                } else if !tab.search_text.trim().is_empty() && tab.doc.hits.is_empty() {
                    let q: String = tab.search_text.chars().take(60).collect();
                    ui.label(format!("Tidak ada kecocokan untuk \"{}\".", q));
                } else {
                    let mode = if tab.regex_on {
                        "regex"
                    } else if crate::engine::query::is_boolean_query(&tab.search_text) {
                        "boolean"
                    } else {
                        "literal"
                    };
                    ui.label(format!(
                        "{} hasil ({})",
                        format_count(tab.doc.hits.len() as u64),
                        mode
                    ));
                }
                if tab.doc.search_truncated {
                    ui.label("(dibatasi 200 rb)");
                }
                // Tombol filter dari pencarian: aktif hanya bila query valid.
                let can_filter = !tab.search_text.trim().is_empty();
                if ui
                    .add_enabled(can_filter, egui::Button::new("Jadikan filter"))
                    .on_hover_text("Tampilkan hanya baris yang cocok di viewport")
                    .clicked()
                {
                    tab.filter_text = tab.search_text.clone();
                    let q = tab.filter_text.clone();
                    tab.start_filter(q);
                }
                // Saat memindai: tombol batal (menaikkan generasi -> thread berhenti).
                if tab.doc.search_in_progress
                    && ui
                        .button("Batalkan pencarian")
                        .on_hover_text("Hentikan pindaian yang berjalan (Esc)")
                        .clicked()
                {
                    tab.doc.search_gen += 1;
                    tab.gen_shared
                        .store(tab.doc.search_gen, Ordering::Relaxed);
                    tab.doc.search_in_progress = false;
                    tab.doc.status = String::from("Pencarian dibatalkan.");
                }
                // Chip cakupan aktif (batasi pencarian ke rentang baris).
                if let Some((a, b)) = tab.scope {
                    ui.label(format!(
                        "Cakupan: {}-{}",
                        format_count(a),
                        format_count(b)
                    ));
                    if ui.small_button("×").on_hover_text("Hapus cakupan").clicked() {
                        tab.scope = None;
                        tab.scope_a.clear();
                        tab.scope_b.clear();
                        if !tab.search_text.trim().is_empty() {
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                        }
                    }
                }
            });
            // ---- Filter + mode tampil + cakupan ----
            ui.horizontal_wrapped(|ui| {
                ui.label("Filter");
                let w = (ui.available_width() - 420.0).clamp(120.0, 420.0);
                ui.add(
                    egui::TextEdit::singleline(&mut tab.filter_text)
                        .id_source("saring")
                        .hint_text("ERROR -DEBUG")
                        .desired_width(w),
                );
                if ui.button("Terapkan").clicked() {
                    let q = tab.filter_text.clone();
                    let cs = tab.case_sensitive;
                    tab.doc.filter = parse_filter(&q, cs);
                    tab.start_filter(q);
                }
                if ui.button("Bersihkan").clicked() {
                    tab.filter_text.clear();
                    tab.start_filter(String::new());
                }
                if ui
                    .button("Ke semua tab")
                    .on_hover_text("Terapkan filter ini ke semua tab (korelasi)")
                    .clicked()
                {
                    filter_all = true;
                }
                egui::ComboBox::from_id_salt("viewmode")
                    .selected_text(format!("Tampil: {}", tab.view_mode.nama()))
                    .show_ui(ui, |ui| {
                        for m in ViewMode::semua() {
                            if ui
                                .selectable_label(tab.view_mode == *m, m.nama())
                                .on_hover_text(match m {
                                    ViewMode::All => "Semua baris (atau hasil filter)",
                                    ViewMode::Hits => "Hanya baris hasil pencarian",
                                    ViewMode::Marks => "Hanya baris penanda",
                                })
                                .clicked()
                            {
                                tab.view_mode = *m;
                                tab.refresh_mode_map();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Mode tampil viewport");
                if ui
                    .button("Cakupan…")
                    .on_hover_text("Batasi pencarian ke rentang baris (hemat untuk file besar)")
                    .clicked()
                {
                    tab.scope_open = true;
                }
                if ui
                    .small_button("?")
                    .on_hover_text(
                        "Filter menyembunyikan baris yang tidak cocok.\n\
                         Token dipisah spasi; semua token inclusions harus ada (AND).\n\
                         Awalan - berarti kecualikan.\n\
                         key=value cocokkan field baris JSON (mis. level=ERROR).\n\n\
                         Contoh:\n  ERROR            hanya baris error\n  ERROR -DEBUG     error tanpa debug\n  level=ERROR      field JSON level\n  OrderService     teks spesifik",
                    )
                    .clicked()
                {
                    tab.doc.status = String::from(
                        "Filter: pisahkan token dengan spasi, awalan - mengecualikan. Contoh: ERROR -DEBUG",
                    );
                }
                if ui
                    .button("Rentang waktu…")
                    .on_hover_text("Tampilkan hanya baris dalam rentang cap waktu")
                    .clicked()
                {
                    open_range = true;
                }
            });
            // Chip tegas saat filter aktif.
            if tab.doc.filter_active {
                ui.horizontal_wrapped(|ui| {
                    let total = if tab.doc.index.complete {
                        tab.doc.index.total_lines
                    } else {
                        tab.doc.line_count_estimate()
                    };
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 200, 255),
                        format!(
                            "Filter aktif: {} · {}/{} baris",
                            tab.doc.filter.raw,
                            format_count(tab.doc.filter_map.len() as u64),
                            format_count(total),
                        ),
                    );
                    if ui.small_button("Hapus").clicked() {
                        tab.filter_text.clear();
                        tab.start_filter(String::new());
                    }
                });
            }
        });

        if open_range {
            self.range_open = true;
        }
        // Korelasi: filter tab aktif ke semua tab.
        if filter_all && !self.tabs.is_empty() {
            let q = self.tabs[cur_idx].filter_text.clone();
            for t in self.tabs.iter_mut() {
                t.filter_text = q.clone();
                let qq = q.clone();
                t.start_filter(qq);
            }
            self.session_dirty = true;
        }

        // Flush config global yang ditandai kotor (di luar pinjam tab).
        if self.cfg_dirty {
            self.save_config();
        }

        // Panel penanda (kiri, bisa diubah lebarnya agar tak menekan viewport)
        if self.tabs[cur_idx].show_bookmarks {
            let dark = self.tema_state.1;
            egui::SidePanel::left("penanda")
                .resizable(true)
                .default_width(260.0)
                .min_width(180.0)
                .max_width(460.0)
                .show(ctx, |ui| {
                    ui.heading("Penanda");
                    ui.label("Ctrl+B tandai - F2 ubah label - Alt+Atas/Bawah pindah");
                    let tab = &mut self.tabs[cur_idx];
                    ui.add(
                        egui::TextEdit::singleline(&mut tab.mark_query)
                            .id_source("tandai-saring")
                            .hint_text("Saring penanda…")
                            .desired_width(f32::INFINITY),
                    );
                    let q = tab.mark_query.to_lowercase();
                    // Kumpulkan agar pinjam berakhir sebelum aksi.
                    let rows: Vec<(u64, String, BookmarkColor)> = tab
                        .doc
                        .bookmarks
                        .iter()
                        .filter(|b| {
                            q.is_empty()
                                || b.label.to_lowercase().contains(&q)
                                || b.line.to_string().contains(&q)
                        })
                        .map(|b| (b.line, b.label.clone(), b.color))
                        .collect();
                    if rows.is_empty() {
                        ui.label("Belum ada. Klik nomor baris / Ctrl+B untuk menandai.");
                    }
                    let mut jump: Option<u64> = None;
                    let mut rename: Option<(u64, String, BookmarkColor)> = None;
                    let mut delete: Option<u64> = None;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (ln, label, color) in rows {
                            ui.horizontal(|ui| {
                                let (dot_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(
                                    dot_rect.center(),
                                    4.0,
                                    mark_color(color, dark),
                                );
                                if ui
                                    .button(format!("{} · {}", format_count(ln), label))
                                    .on_hover_text(format!("Lompat ke baris {}", ln))
                                    .clicked()
                                {
                                    jump = Some(ln);
                                }
                                if ui.small_button("Ubah").on_hover_text("Ubah label (F2)").clicked()
                                {
                                    rename = Some((ln, label.clone(), color));
                                }
                                if ui.small_button("×").on_hover_text("Hapus").clicked() {
                                    delete = Some(ln);
                                }
                            });
                        }
                    });
                    if let Some(ln) = jump {
                        self.tabs[cur_idx].nav_to(ln);
                    }
                    if let Some((ln, label, color)) = rename {
                        self.rename_line = ln;
                        self.rename_label = label;
                        self.rename_color = color;
                        self.rename_open = true;
                    }
                    if let Some(ln) = delete {
                        let t = &mut self.tabs[cur_idx];
                        t.doc.bookmarks.retain(|b| b.line != ln);
                        t.marks_dirty = true;
                        t.refresh_mode_map();
                        t.doc.status = format!("Penanda baris {} dihapus.", ln);
                    }
                });
        }

        // ---- status bawah: grup ringkas, gulir mendatar bila sempit ----
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .show(ctx, |ui| {
                let tab = &self.tabs[cur_idx];
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Keadaan utama (teks berwarna, tanpa glyph simbol).
                            if tab.doc.follow && tab.doc.stick_bottom {
                                ui.colored_label(
                                    egui::Color32::from_rgb(90, 220, 120),
                                    "LIVE",
                                );
                                ui.label("memantau tiap 500 ms");
                                // Info baris baru yang segar (< 6 detik).
                                if let Some((note, at)) = &tab.follow_note {
                                    if at.elapsed() < Duration::from_secs(6) {
                                        ui.label(note.clone());
                                    }
                                }
                            } else if tab.doc.follow {
                                ui.label("LIVE dijeda - kembali ke akhir untuk melanjutkan");
                            } else if !tab.doc.index.complete {
                                ui.label(format!(
                                    "Mengindeks {}% - {} baris terdeteksi",
                                    (tab.doc.index.progress * 100.0).round() as u32,
                                    format_count(tab.doc.index.total_lines),
                                ));
                            } else {
                                ui.label("Siap");
                            }
                            ui.separator();
                            ui.label(format!("File: {}", format_size(tab.doc.size)));
                            ui.separator();
                            let lines = if tab.doc.index.complete {
                                format!("{} baris", format_count(tab.doc.index.total_lines))
                            } else {
                                format!("~{} baris", format_count(tab.doc.line_count_estimate()))
                            };
                            ui.label(lines);
                            ui.separator();
                            ui.label(tab.doc.encoding().label());
                            ui.separator();
                            ui.label(format!(
                                "Cari: {} hasil",
                                format_count(tab.doc.hits.len() as u64)
                            ));
                            ui.separator();
                            ui.label(if tab.doc.filter_active {
                                format!(
                                    "Filter: aktif ({})",
                                    format_count(tab.doc.filter_map.len() as u64)
                                )
                            } else {
                                String::from("Filter: mati")
                            });
                            ui.separator();
                            let total_rows = tab.total_view_rows();
                            let pos = if total_rows > 0 {
                                (tab.top_row as f64 / total_rows as f64 * 100.0).clamp(0.0, 100.0)
                            } else {
                                0.0
                            };
                            ui.label(format!(
                                "Pos: baris {} ({:.0}%)",
                                format_count(
                                    tab.row_to_line(tab.top_row).unwrap_or(1)
                                ),
                                pos,
                            ));
                            if !tab.doc.status.is_empty() {
                                ui.separator();
                                ui.label(tab.doc.status.clone());
                            }
                        });
                    });
            });

        // ---- hasil pencarian: header ramping, collapsible ----
        // Tertutup (header ±28px) saat belum ada query/hasil agar viewport log lega.
        egui::TopBottomPanel::bottom("hasil")
            .resizable(true)
            .default_height(180.0)
            .min_height(30.0)
            .max_height(460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let n = self.tabs[cur_idx].doc.hits.len();
                    ui.strong(format!("Hasil ({})", format_count(n as u64)));
                    if let Some(c) = self.tabs[cur_idx].current_hit {
                        if n > 0 {
                            ui.label(format!("dipilih #{}/{}", c + 1, n));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("×")
                            .on_hover_text("Bersihkan pencarian")
                            .clicked()
                        {
                            let t = &mut self.tabs[cur_idx];
                            t.search_text.clear();
                            t.last_searched.clear();
                            t.doc.hits.clear();
                            t.doc.search_error = None;
                            t.doc.search_in_progress = false;
                            t.current_hit = None;
                            t.results_collapsed = true;
                        }
                        let t = &mut self.tabs[cur_idx];
                        let (icon, tip) = if t.results_collapsed {
                            (Icon::ChevronDown, "Tampilkan panel hasil")
                        } else {
                            (Icon::ChevronUp, "Ciutkan panel hasil")
                        };
                        if icon_button(ui, icon, tip).clicked() {
                            t.results_collapsed = !t.results_collapsed;
                        }
                    });
                });
                // Isi hanya bila dibuka dan relevan (ada query / hasil / progres / galat).
                let show_body = {
                    let t = &self.tabs[cur_idx];
                    !t.results_collapsed
                        && (!t.search_text.trim().is_empty()
                            || !t.doc.hits.is_empty()
                            || t.doc.search_in_progress
                            || t.doc.search_error.is_some())
                };
                if !show_body {
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    // konteks hasil terpilih
                    if ui.button("Tampilkan ±20 baris").clicked() {
                        let t = &mut self.tabs[cur_idx];
                        if let Some(c) = t.current_hit {
                            if let Some(h) = t.doc.hits.get(c) {
                                let ln = h.line;
                                t.selected_line = ln;
                                // top = ln-20 dalam koordinat view
                                let row = t.view_row_of_line(ln).unwrap_or(0);
                                t.top_row = row.saturating_sub(20);
                                t.record_nav(ln);
                            }
                        }
                    }
                    if ui
                        .button("Salin hasil")
                        .on_hover_text("Salin semua hasil (maks 16 MB) ke papan klip")
                        .clicked()
                    {
                        let t = &mut self.tabs[cur_idx];
                        let mut out = String::new();
                        let mut over = false;
                        for h in t.doc.hits.clone() {
                            let txt = t.doc.get_line_text(h.line).unwrap_or_default();
                            let row = format!("{}: {}\n", h.line, txt);
                            if out.len() + row.len() > crate::engine::COPY_CAP_BYTES {
                                over = true;
                                break;
                            }
                            out.push_str(&row);
                        }
                        if out.is_empty() {
                            t.doc.status = String::from("Tidak ada hasil untuk disalin.");
                        } else {
                            ctx.copy_text(out);
                            t.doc.status = if over {
                                String::from(
                                    "Hasil disalin sebagian (16 MB). Gunakan Ekspor untuk sisanya.",
                                )
                            } else {
                                String::from("Hasil disalin ke papan klip.")
                            };
                        }
                    }
                    if ui.button("Ekspor hasil…").clicked() {
                        self.tabs[cur_idx].export_open = true;
                    }
                });
            let hits = self.tabs[cur_idx].doc.hits.clone();
            if hits.is_empty() {
                ui.label("Belum ada hasil. Ketik kata kunci di kolom Cari.");
            } else {
                let row_h = self.row_h();
                let total = hits.len();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show_rows(ui, row_h, total, |ui, range| {
                        // Satu baris hasil = satu baris visual (potong, jangan wrap).
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        // Decode preview per row (cached in Doc).
                        for i in range {
                            let Some(h) = hits.get(i) else { continue };
                            // Ambil teks baris via Doc (pinjam mut singkat per baris).
                            let text = self.tabs[cur_idx]
                                .doc
                                .get_line_text(h.line)
                                .unwrap_or_default();
                            let sel = self.tabs[cur_idx].current_hit == Some(i);
                            let label = result_row(
                                i,
                                h.line,
                                &crate::engine::jsonlog::display_text(&text),
                            );
                            if ui.selectable_label(sel, label).clicked() {
                                self.tabs[cur_idx].jump_to_hit(i);
                            }
                        }
                    });
            }
        });

        // ---- viewport utama: memakai seluruh sisa tinggi CentralPanel ----
        egui::CentralPanel::default().show(ctx, |ui| {
            let rh = self.row_h();
            let rules = &self.hl_compiled;
            let tab = &mut self.tabs[cur_idx];
            let total_rows = tab.total_view_rows();
            // Ukur sekali di awal: tinggi log = sisa panel dikurangi baris aksi.
            // (Jangan ukur ulang di tengah; itulah sumber viewport kerdil.)
            let avail = ui.available_size();
            let action_h = 30.0;
            let log_h = (avail.y - action_h).max(60.0);
            let visible = ((log_h / rh).floor() as u64).clamp(10, 400);
            tab.last_visible = visible;
            // Wheel: gulir per baris
            let delta_y = ctx.input(|i| {
                let a = i.raw_scroll_delta.y;
                let b = i.smooth_scroll_delta.y;
                if a != 0.0 { a } else { b }
            });
            if delta_y != 0.0 {
                let step = ((delta_y.abs() / 20.0).ceil() as u64).max(1).min(50);
                if delta_y < 0.0 {
                    tab.top_row = (tab.top_row + step).min(total_rows.saturating_sub(1));
                    // di dekat bawah -> kunci bawah bila Ikuti
                    if tab.doc.follow
                        && tab.top_row + visible >= total_rows.saturating_sub(2)
                    {
                        tab.doc.stick_bottom = true;
                    }
                } else {
                    tab.top_row = tab.top_row.saturating_sub(step);
                    tab.doc.stick_bottom = false; // gulir ke atas melepas kunci
                }
            }
            // Keyboard atas/bawah PgUp/PgDn
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                tab.top_row = (tab.top_row + 1).min(total_rows.saturating_sub(1));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                tab.top_row = tab.top_row.saturating_sub(1);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::PageDown)) {
                tab.top_row = (tab.top_row + visible).min(total_rows.saturating_sub(1));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
                tab.top_row = tab.top_row.saturating_sub(visible);
            }
            // Stick-to-bottom bila follow
            if tab.doc.follow && tab.doc.stick_bottom {
                tab.top_row = total_rows.saturating_sub(visible);
            }
            tab.top_row = tab.top_row.min(total_rows.saturating_sub(1));
            let dark = self.tema_state.1;

            // Ambil baris viewport (decode hanya yang terlihat).
            let start_row = tab.top_row;
            let mut rows: Vec<(u64, u64, String)> = Vec::new();
            for r in start_row..(start_row + visible).min(total_rows) {
                let Some(ln) = tab.row_to_line(r) else { continue };
                // `None` = baris belum terpetakan (indeks berjalan): tampilkan
                // placeholder agar area tak tampak kosong misterius.
                let txt = tab
                    .doc
                    .get_line_text(ln)
                    .unwrap_or_else(|| String::from("…"));
                rows.push((r, ln, txt));
            }
            let cur_hit_line = tab
                .current_hit
                .and_then(|c| tab.doc.hits.get(c))
                .map(|h| h.line);
            let total_lines = if tab.doc.index.complete {
                tab.doc.index.total_lines
            } else {
                tab.doc.line_count_estimate()
            };
            // Marker peta kepadatan: hasil cari (teal), penanda (biru),
            // aktif (hijau). Merah/kuning khusus bucket ERROR/WARN.
            // Disampling maks ~1200 titik agar murah tiap frame.
            let mut markers: Vec<(f32, egui::Color32)> = Vec::new();
            if total_lines > 0 {
                let stride = (tab.doc.hits.len() / 1200).max(1);
                for (i, h) in tab.doc.hits.iter().enumerate() {
                    if i % stride != 0 {
                        continue;
                    }
                    markers.push((
                        h.line as f32 / total_lines as f32,
                        egui::Color32::from_rgb(70, 210, 200),
                    ));
                }
                for b in &tab.doc.bookmarks {
                    markers.push((
                        b.line as f32 / total_lines as f32,
                        egui::Color32::from_rgb(90, 160, 255),
                    ));
                }
                if let Some(c) = tab.current_hit.and_then(|c| tab.doc.hits.get(c)) {
                    markers.push((c.line as f32 / total_lines as f32, egui::Color32::GREEN));
                }
            }
            // Lebar gutter dinamis mengikuti digit jumlah baris.
            let digits = total_lines.to_string().len().max(4);

            // Area log setinggi log_h persis (kolom daftar + strip + slider).
            ui.allocate_ui_with_layout(
                egui::vec2(avail.x.max(200.0), log_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.horizontal(|ui| {
                        let strip_w = 12.0;
                        let bar_w = 32.0;
                        let list_w = (ui.available_width() - strip_w - bar_w).max(120.0);
                        // Kolom daftar log: gulir mendatar untuk baris panjang.
                        ui.allocate_ui_with_layout(
                            egui::vec2(list_w, log_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.style_mut().wrap_mode =
                                    Some(egui::TextWrapMode::Extend);
                                egui::ScrollArea::horizontal()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.vertical(|ui| {
                                            let hover_old = tab.hover_line;
                                            let mut hover_new = None;
                                            for (_r, ln, txt) in &rows {
                                                let ln = *ln;
                                                // Baris JSON diringkas inline (asli tetap untuk salin).
                                                let disp =
                                                    crate::engine::jsonlog::display_text(txt);
                                                let kind = viewer::classify(&disp);
                                                // SELALU bungkus Frame (isi beda, ukuran sama)
                                                // agar hover tak menggeser layout (anti-flicker).
                                                let bg = if Some(ln) == cur_hit_line {
                                                    viewer::bg_for_current_match_theme(dark)
                                                } else if ln == tab.selected_line {
                                                    viewer::bg_for_selection(dark)
                                                } else if Some(ln) == tab.hover_line {
                                                    viewer::bg_for_hover(dark)
                                                } else {
                                                    egui::Color32::TRANSPARENT
                                                };
                                                let rr = egui::Frame::new().fill(bg).show(
                                                    ui,
                                                    |ui| {
                            ui.horizontal(|ui| {
                                                            // Gutter: nomor asli + penanda ("*" ASCII,
                                                            // bukan glyph bintang agar anti-tofu).
                                                            let mark_col = tab
                                                                .doc
                                                                .bookmarks
                                                                .iter()
                                                                .find(|b| b.line == ln)
                                                                .map(|b| b.color);
                                                            let gutter = format!(
                                                                "{} {:>w$}",
                                                                if mark_col.is_some() {
                                                                    "*"
                                                                } else {
                                                                    " "
                                                                },
                                                                ln,
                                                                w = digits
                                                            );
                                                            let gcolor = match mark_col {
                                                                Some(c) => mark_color(c, dark),
                                                                None => viewer::gutter_color(dark),
                                                            };
                                                            let g = ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(gutter)
                                                                        .monospace()
                                                                        .color(gcolor),
                                                                )
                                                                .sense(egui::Sense::click()),
                                                            );
                                                            ui.separator();
                                                            let t = viewer::render_log_line(
                                                                ui, &disp, kind, dark, rules,
                                                            );
                                                            RowResp { g, t }
                                                        })
                                                        .inner
                                                    },
                                                )
                                                .inner;
                                                if rr.g.clicked() {
                                                    tab.doc.toggle_bookmark(ln);
                                                    tab.selected_line = ln;
                                                    tab.marks_dirty = true;
                                                    tab.refresh_mode_map();
                                                } else if rr.t.clicked() {
                                                    tab.selected_line = ln;
                                                }
                                                if rr.g.hovered() || rr.t.hovered() {
                                                    hover_new = Some(ln);
                                                }
                                            }
                                            tab.hover_line = hover_new;
                                            if hover_new != hover_old {
                                                ctx.request_repaint();
                                            }
                                        });
                                    });
                            },
                        );
                        // Strip peta: klik = lompat ke posisi file.
                        ui.allocate_ui_with_layout(
                            egui::vec2(strip_w, log_h),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(strip_w, log_h),
                                    egui::Sense::click(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(
                                    rect,
                                    0.0,
                                    egui::Color32::from_gray(if dark { 26 } else { 235 }),
                                );
                                // Peta bucket ERROR (merah) / WARN (kuning).
                                if let Some(bits) = tab.marker_bits.as_ref() {
                                    let n = bits.len().max(1) as f32;
                                    for (i, b) in bits.iter().enumerate() {
                                        if *b == 0 {
                                            continue;
                                        }
                                        let y0 = rect.top() + (i as f32 / n) * rect.height();
                                        let y1 = rect.top() + ((i + 1) as f32 / n) * rect.height();
                                        let c = if *b & 0x01 != 0 {
                                            egui::Color32::from_rgb(220, 60, 60)
                                        } else {
                                            egui::Color32::from_rgb(220, 170, 40)
                                        };
                                        painter.rect_filled(
                                            egui::Rect::from_min_max(
                                                egui::pos2(rect.left(), y0),
                                                egui::pos2(rect.right(), y1.max(y0 + 1.0)),
                                            ),
                                            0.0,
                                            c,
                                        );
                                    }
                                }
                                for (f, c) in &markers {
                                    let y = rect.top()
                                        + f.clamp(0.0, 1.0) * rect.height();
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(rect.left(), y - 1.0),
                                            egui::vec2(rect.width(), 2.0),
                                        ),
                                        0.0,
                                        *c,
                                    );
                                }
                                // Shading histogram ERROR/menit di atas bucket.
                                let hist_bins = tab
                                    .time_hist
                                    .as_ref()
                                    .map(|h| h.counts.iter().max().copied().unwrap_or(0))
                                    .unwrap_or(0);
                                if let Some(h) = tab.time_hist.as_ref() {
                                    if hist_bins > 0 && !h.counts.is_empty() {
                                        let n = h.counts.len() as f32;
                                        for (i, c) in h.counts.iter().enumerate() {
                                            if *c == 0 {
                                                continue;
                                            }
                                            let y0 = rect.top() + (i as f32 / n) * rect.height();
                                            let y1 = rect.top()
                                                + ((i + 1) as f32 / n) * rect.height();
                                            let a = (40.0
                                                + 160.0 * (*c as f32 / hist_bins as f32))
                                                as u8;
                                            painter.rect_filled(
                                                egui::Rect::from_min_max(
                                                    egui::pos2(rect.left(), y0),
                                                    egui::pos2(rect.right(), y1.max(y0 + 1.0)),
                                                ),
                                                0.0,
                                                egui::Color32::from_rgba_premultiplied(
                                                    220, 60, 60, a,
                                                ),
                                            );
                                        }
                                    }
                                }
                                // Garis posisi viewport kini.
                                if total_rows > 0 {
                                    let f = tab.top_row as f32 / total_rows as f32;
                                    let y = rect.top() + f.clamp(0.0, 1.0) * rect.height();
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(rect.left(), y - 1.0),
                                            egui::vec2(rect.width(), 2.0),
                                        ),
                                        0.0,
                                        egui::Color32::WHITE,
                                    );
                                }
                                if resp.clicked() {
                                    if let Some(p) = resp.interact_pointer_pos() {
                                        let f = ((p.y - rect.top()) / rect.height())
                                            .clamp(0.0, 1.0);
                                        tab.top_row = ((f * total_rows as f32) as u64)
                                            .min(total_rows.saturating_sub(1));
                                        tab.doc.stick_bottom = false;
                                        if let Some(ln) =
                                            tab.row_to_line(tab.top_row)
                                        {
                                            tab.selected_line = ln;
                                            tab.record_nav(ln);
                                        }
                                    }
                                }
                                // Legenda makna warna strip.
                                {
                                    let bits = tab.marker_bits.as_ref();
                                    let eb = bits
                                        .map(|b| b.iter().filter(|x| *x & 0x01 != 0).count())
                                        .unwrap_or(0);
                                    let wb = bits
                                        .map(|b| b.iter().filter(|x| *x & 0x02 != 0).count())
                                        .unwrap_or(0);
                                    resp.on_hover_text(format!(
                                        "Teal: hasil pencarian ({})\nBiru: penanda ({})\nMerah: bucket ERROR ({} dari 512)\nKuning: bucket WARN ({} dari 512)\nArsir merah: kepadatan ERROR/menit\nHijau: hasil aktif - Putih: posisi viewport\nKlik: lompat ke posisi",
                                        format_count(tab.doc.hits.len() as u64),
                                        format_count(tab.doc.bookmarks.len() as u64),
                                        eb,
                                        wb,
                                    ));
                                }
                            },
                        );
                        // Bilah gulir kustom setinggi viewport: track penuh +
                        // thumb proporsional (posisi pada 38 jt baris terbaca sekilas).
                        ui.allocate_ui_with_layout(
                            egui::vec2(bar_w, log_h),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                let track_h = (log_h - 28.0).max(40.0);
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(bar_w, track_h),
                                    egui::Sense::click_and_drag(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(
                                    rect,
                                    4.0,
                                    if dark {
                                        egui::Color32::from_gray(38)
                                    } else {
                                        egui::Color32::from_gray(208)
                                    },
                                );
                                let total_f = total_rows.max(1) as f32;
                                let th = ((visible as f32 / total_f) * rect.height())
                                    .clamp(10.0, rect.height());
                                let top_f = if total_rows > 1 {
                                    tab.top_row as f32 / (total_rows - 1) as f32
                                } else {
                                    0.0
                                };
                                let y0 = rect.top() + top_f * (rect.height() - th);
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(rect.left() + 2.0, y0),
                                        egui::vec2(rect.width() - 4.0, th),
                                    ),
                                    4.0,
                                    if dark {
                                        egui::Color32::from_gray(120)
                                    } else {
                                        egui::Color32::from_gray(140)
                                    },
                                );
                                if resp.clicked() || resp.dragged() {
                                    if let Some(p) = resp.interact_pointer_pos() {
                                        let f = ((p.y - th / 2.0 - rect.top())
                                            / (rect.height() - th).max(1.0))
                                            .clamp(0.0, 1.0);
                                        tab.top_row = ((f * total_rows.saturating_sub(1) as f32)
                                            as u64)
                                            .min(total_rows.saturating_sub(1));
                                        tab.doc.stick_bottom = false;
                                        if let Some(ln) = tab.row_to_line(tab.top_row)
                                        {
                                            tab.selected_line = ln;
                                        }
                                    }
                                }
                                resp.on_hover_text(format!(
                                    "Baris {} / {} · {:.0}%",
                                    format_count(tab.top_row + 1),
                                    format_count(total_rows),
                                    if total_rows > 0 {
                                        tab.top_row as f64 / total_rows as f64 * 100.0
                                    } else {
                                        0.0
                                    },
                                ));
                                if ui.small_button("Akhir").on_hover_text("Ke akhir file").clicked() {
                                    tab.top_row = total_rows.saturating_sub(visible);
                                    tab.doc.stick_bottom = true;
                                }
                            },
                        );
                    });
                },
            );
            // Baris aksi bawah (setinggi action_h).
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Baris {}", tab.selected_line));
                ui.menu_button("Salin v", |ui| {
                    if ui.button("Salin baris ini").clicked() {
                        match tab.doc.copy_range_text(tab.selected_line, tab.selected_line) {
                            Ok(s) => {
                                ctx.copy_text(s);
                                tab.doc.status = String::from("Baris disalin ke papan klip.");
                            }
                            Err(e) => tab.doc.status = e,
                        }
                        ui.close();
                    }
                    if ui.button("Salin 50 baris").clicked() {
                        match tab.doc.copy_range_text(tab.selected_line, tab.selected_line + 49) {
                            Ok(s) => {
                                ctx.copy_text(s);
                                tab.doc.status = String::from("50 baris disalin.");
                            }
                            Err(e) => tab.doc.status = e,
                        }
                        ui.close();
                    }
                    if ui
                        .button("Salin + nomor (50 baris)")
                        .on_hover_text("Format \"nomor: isi\"")
                        .clicked()
                    {
                        copy_numbered(tab, ctx, 50);
                        ui.close();
                    }
                    if ui
                        .button("Simpan 200 baris ke file…")
                        .on_hover_text("Tulis 200 baris dari posisi ini ke file baru")
                        .clicked()
                    {
                        let a = tab.selected_line;
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name("asislog-pilihan.txt")
                            .save_file()
                        {
                            match tab.doc.export_range_to_file(&p, a, a + 199) {
                                Ok(n) => {
                                    tab.doc.status = format!(
                                        "Disimpan {} baris ke {}.",
                                        n,
                                        p.display()
                                    )
                                }
                                Err(e) => tab.doc.status = e,
                            }
                        }
                        ui.close();
                    }
                    if ui
                        .button("Salin sebagai path")
                        .on_hover_text("Salin \"file:baris\" untuk referensi")
                        .clicked()
                    {
                        ctx.copy_text(format!(
                            "{}:{}",
                            tab.doc.path.display(),
                            tab.selected_line
                        ));
                        tab.doc.status = String::from("Path + baris disalin.");
                        ui.close();
                    }
                });
                ui.menu_button("Salin blok v", |ui| {
                    if ui
                        .button("Salin blok SQL")
                        .on_hover_text("Statement --INSERT-…/INSERT INTO… s.d. go")
                        .clicked()
                    {
                        copy_block(tab, ctx, 0);
                        ui.close();
                    }
                    if ui
                        .button("Salin blok transaksi")
                        .on_hover_text("BEGIN TRANSACTION s.d. COMMIT/ROLLBACK/go")
                        .clicked()
                    {
                        copy_block(tab, ctx, 1);
                        ui.close();
                    }
                    if ui
                        .button("Salin blok checkpoint")
                        .on_hover_text("--START CHECKPOINT s.d. --FINISH CHECKPOINT")
                        .clicked()
                    {
                        copy_block(tab, ctx, 2);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Ekspor blok SQL…").clicked() {
                        export_block(tab, 0, "asislog-blok.sql");
                        ui.close();
                    }
                    if ui.button("Ekspor blok transaksi…").clicked() {
                        export_block(tab, 1, "asislog-transaksi.sql");
                        ui.close();
                    }
                    if ui.button("Ekspor blok checkpoint…").clicked() {
                        export_block(tab, 2, "asislog-checkpoint.txt");
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pos = if total_rows > 0 {
                        (tab.top_row as f64 / total_rows as f64 * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    };
                    ui.label(format!(
                        "{}/{} · {:.0}%",
                        format_count(tab.top_row + 1),
                        format_count(total_rows),
                        pos,
                    ));
                });
            });
        });

        // ---- dialog Ke… ----
        if self.tabs[cur_idx].goto_open {
            let mut do_go = false;
            let mut do_close = false;
            let mut goto_all = self.goto_all;
            egui::Window::new("Ke baris / persen / waktu (Ctrl+G)")
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label("Contoh: 38166903 · 50% · akhir · 2026-09-03 13:41:02");
                    let r = ui.text_edit_singleline(&mut tab.goto_input);
                    // fokus awal
                    if tab.goto_input.is_empty() {
                        r.request_focus();
                    }
                    if !tab.goto_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, &tab.goto_msg);
                    }
                    ui.checkbox(&mut goto_all, "Semua tab (korelasi waktu/baris)");
                    ui.horizontal(|ui| {
                        if ui.button("Pergi").clicked() {
                            do_go = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                    if r.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        do_go = true;
                    }
                });
            self.goto_all = goto_all;
            if do_go {
                self.goto_execute(cur_idx);
            }
            if do_close {
                self.tabs[cur_idx].goto_open = false;
            }
        }

        // ---- dialog Ekspor ----
        if self.tabs[cur_idx].export_open {
            let mut do_export: Option<usize> = None;
            let mut do_ticket: Option<usize> = None;
            let mut do_close = false;
            egui::Window::new("Simpan hasil ke file…")
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label(format!("{} hasil.", tab.doc.hits.len()));
                    ui.horizontal(|ui| {
                        ui.label("Konteks (baris sekitar):");
                        ui.add(egui::DragValue::new(&mut tab.export_context).range(0..=100));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Hanya hasil").clicked() {
                            do_export = Some(0);
                        }
                        if ui.button("Hasil + konteks").clicked() {
                            do_export = Some(tab.export_context);
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button("Tiket Markdown (Jira)")
                            .on_hover_text("Hasil + konteks sebagai Markdown siap paste")
                            .clicked()
                        {
                            do_ticket = Some(tab.export_context);
                        }
                    });
                });
            if let Some(cx) = do_export {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-ekspor.txt")
                    .save_file();
                if let Some(p) = out {
                    let tab = &mut self.tabs[cur_idx];
                    match tab.doc.export_hits_to_file(&p, cx) {
                        Ok(n) => {
                            tab.doc.status =
                                format!("Diekspor {} baris ke {}.", n, p.display());
                            tab.export_open = false;
                        }
                        Err(e) => tab.doc.status = e,
                    }
                }
            }
            if let Some(cx) = do_ticket {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-tiket.md")
                    .save_file();
                if let Some(p) = out {
                    let tab = &mut self.tabs[cur_idx];
                    let q = tab.search_text.clone();
                    match tab.doc.export_ticket_to_file(&p, &q, cx) {
                        Ok(n) => {
                            tab.doc.status = format!(
                                "Tiket ({} baris konteks) disimpan ke {}.",
                                n,
                                p.display()
                            );
                            tab.export_open = false;
                        }
                        Err(e) => tab.doc.status = e,
                    }
                }
            }
            if do_close {
                self.tabs[cur_idx].export_open = false;
            }
        }

        // ---- dialog cakupan pencarian ----
        if self.tabs[cur_idx].scope_open {
            let mut do_apply = false;
            let mut do_clear = false;
            let mut do_close = false;
            egui::Window::new("Cakupan pencarian (baris)")
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label("Cari hanya dalam rentang baris ini. Hemat untuk file besar.");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Dari");
                        ui.text_edit_singleline(&mut tab.scope_a);
                        ui.label("Sampai");
                        ui.text_edit_singleline(&mut tab.scope_b);
                    });
                    if !tab.scope_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, tab.scope_msg.clone());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Terapkan").clicked() {
                            do_apply = true;
                        }
                        if ui.button("Bersihkan").clicked() {
                            do_clear = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_apply {
                let tab = &mut self.tabs[cur_idx];
                let parse = |s: &str| {
                    s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().ok()
                };
                match (parse(&tab.scope_a.clone()), parse(&tab.scope_b.clone())) {
                    (Some(a), Some(b)) if a >= 1 && b >= a => {
                        tab.scope = Some((a, b));
                        tab.scope_msg.clear();
                        tab.scope_open = false;
                        if !tab.search_text.trim().is_empty() {
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                        } else {
                            tab.doc.status = format!(
                                "Cakupan {}-{} aktif; ketik query untuk mencari.",
                                format_count(a),
                                format_count(b)
                            );
                        }
                    }
                    _ => {
                        self.tabs[cur_idx].scope_msg =
                            String::from("Rentang tidak valid. Contoh: 1000000 sampai 2000000.");
                    }
                }
            }
            if do_clear {
                let tab = &mut self.tabs[cur_idx];
                tab.scope = None;
                tab.scope_a.clear();
                tab.scope_b.clear();
                tab.scope_msg.clear();
                tab.scope_open = false;
                if !tab.search_text.trim().is_empty() {
                    tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                }
            }
            if do_close {
                self.tabs[cur_idx].scope_open = false;
            }
        }

        // ---- dialog simpan preset ----
        if self.preset_save_open {
            let mut do_save = false;
            let mut do_close = false;
            egui::Window::new("Simpan pencarian sebagai preset")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Nama preset:");
                    ui.text_edit_singleline(&mut self.preset_save_name);
                    ui.horizontal(|ui| {
                        if ui.button("Simpan").clicked() {
                            do_save = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_save {
                let name = self.preset_save_name.trim().to_string();
                if name.is_empty() {
                    self.global_status = String::from("Nama preset tidak boleh kosong.");
                } else {
                    let t = &self.tabs[cur_idx];
                    if let Some(p) =
                        self.presets.iter_mut().find(|p| p.name == name)
                    {
                        p.query = t.search_text.clone();
                        p.regex = t.regex_on;
                        p.case_sensitive = t.case_sensitive;
                    } else {
                        self.presets.push(Preset {
                            name,
                            query: t.search_text.clone(),
                            regex: t.regex_on,
                            case_sensitive: t.case_sensitive,
                        });
                    }
                    self.save_config();
                    self.preset_save_open = false;
                }
            }
            if do_close {
                self.preset_save_open = false;
            }
        }

        // ---- jendela set sorotan ----
        if self.hl_open {
            let mut dirty = false;
            let mut do_export = false;
            let mut do_import = false;
            egui::Window::new("Set highlight (viewport)")
                .collapsible(false)
                .resizable(true)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.label("Set bernama per produk; aturan hanya untuk baris terlihat.");
                    // Pemilih set + buat + hapus.
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Set:");
                        let cur = self
                            .active_set
                            .clone()
                            .unwrap_or_else(|| String::from("-"));
                        egui::ComboBox::from_id_salt("hlset")
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                for s in &self.sets {
                                    let mut sel = self.active_set.as_deref() == Some(&s.name);
                                    if ui.checkbox(&mut sel, s.name.clone()).clicked() {
                                        self.active_set = Some(s.name.clone());
                                        dirty = true;
                                    }
                                }
                            });
                        ui.text_edit_singleline(&mut self.hl_set_name);
                        if ui.button("Buat").clicked() {
                            let name = self.hl_set_name.trim().to_string();
                            if name.is_empty() {
                                self.global_status =
                                    String::from("Nama set tidak boleh kosong.");
                            } else if self.sets.iter().any(|s| s.name == name) {
                                self.global_status =
                                    String::from("Set dengan nama itu sudah ada.");
                            } else {
                                self.sets.push(HighlightSet { name: name.clone(), rules: Vec::new() });
                                self.active_set = Some(name);
                                self.hl_set_name.clear();
                                dirty = true;
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Ekspor set…").clicked() {
                            do_export = true;
                        }
                        if ui.button("Impor set…").clicked() {
                            do_import = true;
                        }
                        if ui.button("Hapus set").clicked() {
                            if let Some(n) = self.active_set.clone() {
                                self.sets.retain(|s| s.name != n);
                                self.active_set =
                                    self.sets.first().map(|s| s.name.clone());
                                dirty = true;
                            }
                        }
                    });
                    ui.separator();
                    // Aturan milik set aktif.
                    let active = self.active_set.clone().unwrap_or_default();
                    let idx = self.sets.iter().position(|s| s.name == active);
                    if let Some(si) = idx {
                        let mut del: Option<usize> = None;
                        // Pinjam rules saja agar field form tetap bebas.
                        let rules = &mut self.sets[si].rules;
                        for (i, r) in rules.iter_mut().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                let show = format!(
                                    "{} [{}] {}",
                                    if r.enabled { "[x]" } else { "[ ]" },
                                    if r.regex { "regex" } else { "teks" },
                                    r.name
                                );
                                if ui.small_button(show).clicked() {
                                    r.enabled = !r.enabled;
                                    dirty = true;
                                }
                                ui.label(format!("\"{}\" -> {}", r.pattern, color_name_id(&r.color)));
                                if ui.small_button("×").on_hover_text("Hapus aturan").clicked() {
                                    del = Some(i);
                                }
                            });
                        }
                        if let Some(i) = del {
                            self.sets[si].rules.remove(i);
                            dirty = true;
                        }
                    } else {
                        ui.label("Belum ada set. Buat set dulu di atas.");
                    }
                    ui.separator();
                    ui.label("Tambah aturan ke set aktif:");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Nama");
                        ui.text_edit_singleline(&mut self.hl_name);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Pola");
                        ui.text_edit_singleline(&mut self.hl_pattern);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.hl_regex, "Regex");
                        ui.checkbox(&mut self.hl_case, "Peka huruf");
                        ui.checkbox(&mut self.hl_whole, "Baris penuh");
                        egui::ComboBox::from_id_salt("hlcolor")
                            .selected_text(color_name_id(
                                highlight_keys()[self.hl_color_idx % highlight_keys().len()].0,
                            ))
                            .show_ui(ui, |ui| {
                                for (k, nama) in highlight_keys() {
                                    ui.selectable_value(&mut self.hl_color_idx, key_index(k), *nama);
                                }
                            });
                    });
                    if ui.button("Tambah").clicked() {
                        let keys = highlight_keys();
                        let key = keys[self.hl_color_idx % keys.len()].0.to_string();
                        let rule = HighlightRule {
                            name: if self.hl_name.trim().is_empty() {
                                self.hl_pattern.trim().to_string()
                            } else {
                                self.hl_name.trim().to_string()
                            },
                            pattern: self.hl_pattern.trim().to_string(),
                            regex: self.hl_regex,
                            case_sensitive: self.hl_case,
                            color: key,
                            whole_line: self.hl_whole,
                            enabled: true,
                        };
                        match rule.validate() {
                            Ok(()) => {
                                let active = self.active_set.clone().unwrap_or_default();
                                if let Some(s) =
                                    self.sets.iter_mut().find(|s| s.name == active)
                                {
                                    s.rules.push(rule);
                                    self.hl_name.clear();
                                    self.hl_pattern.clear();
                                    dirty = true;
                                } else {
                                    self.global_status = String::from(
                                        "Buat/pilih set dulu sebelum menambah aturan.",
                                    );
                                }
                            }
                            Err(e) => self.global_status = e,
                        }
                    }
                    if ui.button("Tutup").clicked() {
                        self.hl_open = false;
                    }
                });
            if do_export {
                if let Some(n) = self.active_set.clone() {
                    if let Some(s) = self.sets.iter().find(|s| s.name == n) {
                        let fname = format!("{}-highlight.json", sanitize_name(&n));
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name(fname)
                            .save_file()
                        {
                            match serde_json::to_string_pretty(s) {
                                Ok(text) => match std::fs::write(&p, text) {
                                    Ok(()) => {
                                        self.global_status = format!(
                                            "Set '{}' diekspor ke {}.",
                                            n,
                                            p.display()
                                        )
                                    }
                                    Err(e) => {
                                        self.global_status =
                                            format!("Gagal menulis: {}", e)
                                    }
                                },
                                Err(e) => {
                                    self.global_status = format!("Gagal menyusun: {}", e)
                                }
                            }
                        }
                    }
                }
            }
            if do_import {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
                {
                    match std::fs::read_to_string(&p) {
                        Ok(text) => {
                            // Terima set penuh atau daftar aturan polos.
                            let parsed: Option<HighlightSet> =
                                serde_json::from_str(&text).ok().or_else(|| {
                                    serde_json::from_str::<Vec<HighlightRule>>(&text)
                                        .ok()
                                        .map(|rules| HighlightSet {
                                            name: p
                                                .file_stem()
                                                .map(|s| s.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| {
                                                    String::from("Impor")
                                                }),
                                            rules,
                                        })
                                });
                            match parsed {
                                Some(mut set) => {
                                    let mut skipped = 0;
                                    set.rules.retain(|r| {
                                        let ok = r.validate().is_ok();
                                        if !ok {
                                            skipped += 1;
                                        }
                                        ok
                                    });
                                    let base = set.name.clone();
                                    let mut name = base.clone();
                                    let mut n = 1;
                                    while self.sets.iter().any(|s| s.name == name) {
                                        n += 1;
                                        name = format!("{} ({})", base, n);
                                    }
                                    set.name = name.clone();
                                    self.sets.push(set);
                                    self.active_set = Some(name.clone());
                                    dirty = true;
                                    self.global_status = format!(
                                        "Set '{}' diimpor{}.",
                                        name,
                                        if skipped > 0 {
                                            format!(", {} aturan salah dilewati", skipped)
                                        } else {
                                            String::new()
                                        }
                                    );
                                }
                                None => {
                                    self.global_status =
                                        String::from("File bukan set highlight yang valid.");
                                }
                            }
                        }
                        Err(e) => self.global_status = format!("Gagal membaca: {}", e),
                    }
                }
            }
            if dirty {
                self.hl_dirty = true;
                self.save_config();
            }
        }

        // ---- dialog ubah label penanda ----
        if self.rename_open {
            let mut do_save = false;
            let mut do_close = false;
            egui::Window::new("Ubah label penanda")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(format!("Baris {}", format_count(self.rename_line)));
                    ui.label("Label:");
                    ui.text_edit_singleline(&mut self.rename_label);
                    egui::ComboBox::from_label("Warna")
                        .selected_text(self.rename_color.nama())
                        .show_ui(ui, |ui| {
                            for c in BookmarkColor::semua() {
                                ui.selectable_value(&mut self.rename_color, *c, c.nama());
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("Simpan").clicked() {
                            do_save = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_save {
                if let Some(t) = self.tabs.get_mut(cur_idx) {
                    if let Some(b) =
                        t.doc.bookmarks.iter_mut().find(|b| b.line == self.rename_line)
                    {
                        b.label = self.rename_label.clone();
                        b.color = self.rename_color;
                        t.marks_dirty = true;
                        t.doc.status = format!("Penanda baris {} diperbarui.", self.rename_line);
                    }
                }
                self.rename_open = false;
            }
            if do_close {
                self.rename_open = false;
            }
        }

        // ---- dialog rentang waktu ----
        if self.range_open {
            let mut do_go = false;
            let mut do_close = false;
            egui::Window::new("Tampilkan rentang waktu")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Contoh: 2026-08-24 13:00:00 sampai 2026-08-24 14:00:00");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Dari");
                        ui.text_edit_singleline(&mut self.range_start);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Sampai");
                        ui.text_edit_singleline(&mut self.range_end);
                    });
                    if !self.range_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, self.range_msg.clone());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Tampilkan rentang").clicked() {
                            do_go = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_go {
                let a = self.range_start.clone();
                let b = self.range_end.clone();
                match apply_time_range(&mut self.tabs[cur_idx], &a, &b)
                {
                    Ok(msg) => {
                        self.tabs[cur_idx].doc.status = msg;
                        self.tabs[cur_idx].range_applied = Some((a, b));
                        self.range_msg.clear();
                        self.range_open = false;
                    }
                    Err(e) => self.range_msg = e,
                }
            }
            if do_close {
                self.range_open = false;
                self.range_msg.clear();
            }
        }

        // ---- dialog buka URL ----
        if self.url_open {
            let mut do_dl = false;
            let mut do_close = false;
            egui::Window::new("Buka dari URL")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Contoh: https://server/app.log");
                    ui.text_edit_singleline(&mut self.url_text);
                    ui.horizontal(|ui| {
                        if ui.button("Unduh & buka").clicked() {
                            do_dl = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_dl {
                let url = self.url_text.trim().to_string();
                if url.is_empty() {
                    self.global_status = String::from("URL kosong.");
                } else {
                    let (tx, rx) = mpsc::channel();
                    self.dl_rx = Some(rx);
                    spawn_download(url, tx);
                    self.global_status = String::from("Mengunduh…");
                    self.url_open = false;
                }
            }
            if do_close {
                self.url_open = false;
            }
        }

        // ---- dialog tempel teks ----
        if self.paste_open {
            let mut do_open = false;
            let mut do_close = false;
            egui::Window::new("Tempel teks sebagai file")
                .collapsible(false)
                .resizable(true)
                .default_width(480.0)
                .show(ctx, |ui| {
                    ui.label("Tempel (Ctrl+V), lalu buka sebagai file temp.");
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 220.0),
                        egui::TextEdit::multiline(&mut self.paste_text)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Buka sebagai file").clicked() {
                            do_open = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_open {
                if self.paste_text.trim().is_empty() {
                    self.global_status = String::from("Teks kosong.");
                } else {
                    let mut p = std::env::temp_dir();
                    p.push("asislog-tempel");
                    let _ = std::fs::create_dir_all(&p);
                    p.push(format!(
                        "tempel-{}.log",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|t| t.as_millis())
                            .unwrap_or(0)
                    ));
                    match std::fs::write(&p, self.paste_text.clone()) {
                        Ok(()) => {
                            self.paste_open = false;
                            self.open_file(p);
                        }
                        Err(e) => self.global_status = format!("Gagal menulis temp: {}", e),
                    }
                }
            }
            if do_close {
                self.paste_open = false;
            }
        }

        // ---- jendela scratchpad ----
        if self.scratch_open {
            egui::Window::new("Scratchpad (catatan + transform)")
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .show(ctx, |ui| {
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 240.0),
                        egui::TextEdit::multiline(&mut self.scratch_text)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Catatan, token, JSON, JWT, SQL…"),
                    );
                    if !self.scratch_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, self.scratch_msg.clone());
                    }
                    let (cc, ww, ll) = crate::engine::scratch::stats(&self.scratch_text);
                    ui.label(format!("{} karakter · {} kata · {} baris", cc, ww, ll));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("JSON rapi").clicked() {
                            match crate::engine::scratch::json_pretty(&self.scratch_text) {
                                Ok(o) => {
                                    self.scratch_text = o;
                                    self.scratch_msg.clear();
                                    self.cfg_dirty = true;
                                }
                                Err(e) => self.scratch_msg = e,
                            }
                        }
                        if ui.button("Base64 decode").clicked() {
                            match crate::engine::scratch::b64_decode(&self.scratch_text) {
                                Ok(o) => {
                                    self.scratch_text = o;
                                    self.scratch_msg.clear();
                                    self.cfg_dirty = true;
                                }
                                Err(e) => self.scratch_msg = e,
                            }
                        }
                        if ui.button("JWT decode").clicked() {
                            match crate::engine::scratch::jwt_decode(&self.scratch_text) {
                                Ok(o) => {
                                    self.scratch_text = o;
                                    self.scratch_msg.clear();
                                    self.cfg_dirty = true;
                                }
                                Err(e) => self.scratch_msg = e,
                            }
                        }
                        if ui.button("SQL rapi").clicked() {
                            self.scratch_text =
                                crate::engine::scratch::sql_tidy(&self.scratch_text);
                            self.scratch_msg.clear();
                            self.cfg_dirty = true;
                        }
                        if ui.button("Bersihkan").clicked() {
                            self.scratch_text.clear();
                            self.scratch_msg.clear();
                            self.cfg_dirty = true;
                        }
                        if ui.button("Tutup").clicked() {
                            self.scratch_open = false;
                        }
                    });
                });
        }

        // Jendela daftar pintasan (F1).
        if self.shortcuts_open {            egui::Window::new("Pintasan AsisLog (F1)")
                .collapsible(false)
                .resizable(true)
                .default_width(380.0)
                .show(ctx, |ui| {
                    for (keys, desc) in [
                        ("Ctrl+O", "Buka file log"),
                        ("Ctrl+F", "Fokus ke kolom Cari"),
                        ("F3 / Shift+F3", "Hasil berikutnya / sebelumnya"),
                        ("n / N", "Hasil berikut / sebelum (di luar kolom ketik)"),
                        ("1-9", "Label warna dari query aktif"),
                        ("Ctrl+G", "Ke baris / persen / akhir / waktu"),
                        ("Ctrl+E", "Ekspor hasil pencarian"),
                        ("Ctrl+Home / Ctrl+End", "Awal / akhir file"),
                        ("Ctrl+Tab / Ctrl+Shift+Tab", "Pindah tab"),
                        ("Ctrl+Shift+F", "Ikuti akhir file (LIVE)"),
                        ("Ctrl+B", "Tandai baris aktif"),
                        ("Ctrl+Shift+B", "Panel penanda"),
                        ("F2", "Ubah label penanda"),
                        ("Alt+Left / Alt+Right", "History mundur / maju"),
                        ("Alt+Atas / Alt+Bawah", "Penanda sebelumnya / berikutnya"),
                        ("Ctrl+= / Ctrl+- / Ctrl+0", "Zoom UI"),
                        ("PgUp / PgDn, Panah", "Gulir viewport"),
                        ("Esc", "Batal & tutup dialog"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.strong(keys);
                            ui.label(desc);
                        });
                    }
                    if ui.button("Tutup").clicked() {
                        self.shortcuts_open = false;
                    }
                });
        }

        // Global error modal
        if self.global_error.is_some() {
            egui::Window::new("Galat")
                .collapsible(false)
                .show(ctx, |ui| {
                    if let Some(e) = &self.global_error {
                        ui.colored_label(egui::Color32::RED, e);
                    }
                    if ui.button("Tutup").clicked() {
                        self.global_error = None;
                    }
                });
        }

        ctx.request_repaint_after(Duration::from_millis(120));
    }
}
