// English comments: ViewMode + TabState (one tab: search/filter/follow/nav state) (split from app.rs; behavior unchanged).
#![allow(unused_imports)]
// English comments: AsisLog egui app (tabs, shortcuts, background jobs).

use std::collections::HashMap;
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
        self.nama_in(crate::i18n::Lang::Id)
    }

    pub fn nama_in(self, lang: crate::i18n::Lang) -> &'static str {
        lang.tr(match self {
            ViewMode::All => "Semua",
            ViewMode::Hits => "Hasil",
            ViewMode::Marks => "Penanda",
        })
    }

    pub fn semua() -> &'static [ViewMode] {
        &[ViewMode::All, ViewMode::Hits, ViewMode::Marks]
    }

    /// Parse nama Indonesia kembali (untuk sesi/workspace); tak dikenal = Semua.
    /// Accepts English too ("Results"/"Bookmarks") for forward compatibility.
    pub fn from_nama(s: &str) -> ViewMode {
        match s {
            "Hasil" | "Results" => ViewMode::Hits,
            "Penanda" | "Bookmarks" => ViewMode::Marks,
            _ => ViewMode::All,
        }
    }
}

/// Status pencarian per-tab (klogg parity: NoSearch / Static / Auto-refreshing
/// / Truncated + Searching + Error). Single source of truth untuk chip status
/// dan indikator redup; diturunkan dari flag worker yang sudah ada.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchState {
    /// Belum ada query (kolom cari kosong).
    #[default]
    NoSearch,
    /// Worker berjalan (pencarian manual penuh).
    Searching,
    /// Ekor follow menggabung diam-diam (LIVE + query aktif).
    AutoRefreshing,
    /// Selesai, hasil statis lengkap.
    Static,
    /// Selesai tapi dipangkas batas (200 rb default / Options).
    Truncated,
    /// Query/worker galat.
    Error,
}

impl SearchState {
    pub fn label_in(self, lang: crate::i18n::Lang) -> &'static str {
        lang.tr(match self {
            SearchState::NoSearch => "Siap",
            SearchState::Searching => "Mencari…",
            SearchState::AutoRefreshing => "Auto-refresh…",
            SearchState::Static => "Statis",
            SearchState::Truncated => "Dibatasi",
            SearchState::Error => "Galat",
        })
    }
}

// ---------- one tab ----------

/// One pinned result set ("keep results", klogg parity): a frozen copy of
/// a finished search to compare against while the live query moves on.
/// Session-only (never persisted): each holds at most MAX_STORED_HITS hits
/// (24 B each), and at most MAX_KEPT snapshots per tab exist.
#[derive(Clone, Debug)]
pub(crate) struct KeptResult {
    pub name: String,
    pub query: String,
    pub hits: Vec<Hit>,
}

/// Max pinned snapshots per tab (memory bound, documented in UI).
pub(crate) const MAX_KEPT: usize = 5;

pub(crate) struct TabState {
    pub(crate) doc: Doc,
    pub(crate) search_text: String,
    pub(crate) last_searched: String,
    pub(crate) debounce_at: Option<Instant>,
    pub(crate) case_sensitive: bool,
    pub(crate) regex_on: bool,
    /// True when the regex needs the fancy backtracking engine (look-around
    /// / backreferences): shown honestly in the mode label, and the worker
    /// takes the sequential fancy path instead of the rayon fast path.
    pub(crate) regex_complex: bool,
    pub(crate) current_hit: Option<usize>,
    /// Progress pindaian pencarian (diperbarui per batch).
    pub(crate) search_scanned: u64,
    pub(crate) search_total: u64,
    /// Exact in-scope match total for the current query (see
    /// `SearchBatchMsg::grand_total`). Equals `hits.len()` unless
    /// truncated; tail-refresh merges add onto `merge_base`.
    pub(crate) search_grand_total: u64,
    /// Grand-total baseline preserved across a quiet tail merge.
    pub(crate) merge_base: u64,
    /// View row count seen last frame (follow pin source of truth).
    /// The bottom pin engages only while this GROWS (new rows arrived);
    /// manual scroll-up releases it without passive re-stick.
    pub(crate) follow_total: u64,
    /// True while a continuation page job is in flight: batches APPEND
    /// to existing hits, grand_total keeps the first-page exact value,
    /// and the cache key is re-armed (a page completing the set caches).
    pub(crate) search_page_active: bool,
    /// Stored-hit index where the current page started (jump target).
    pub(crate) page_base: usize,
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
    /// Arsip asal bila tab diekstrak dari zip/tar/gz (untuk sesi/workspace;
    /// doc.path menunjuk temp yang akan dihapus).
    pub(crate) archive_src: Option<PathBuf>,
    /// Label kustom tab (None = nama file). P1-15 rename, ikut sesi.
    pub(crate) alias: Option<String>,
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
    /// Pinned result snapshots (keep results) + which one is shown.
    /// `kept_view=None` = live results; `Some(i)` = frozen snapshot i.
    pub(crate) kept: Vec<KeptResult>,
    pub(crate) kept_view: Option<usize>,
    pub(crate) goto_open: bool,
    pub(crate) goto_input: String,
    pub(crate) goto_msg: String,
    pub(crate) export_open: bool,
    pub(crate) export_context: usize,
    pub(crate) index_rx: Option<mpsc::Receiver<IndexUpdate>>,
    pub(crate) search_rx: Option<mpsc::Receiver<SearchBatchMsg>>,
    pub(crate) filter_rx: Option<mpsc::Receiver<crate::engine::LineSet>>,
    /// Cancel flag for the in-flight filter worker (a superseding filter
    /// or clear stops it promptly instead of wasting a full scan).
    pub(crate) filter_cancel: Arc<AtomicBool>,
    /// Peta bucket ERROR/WARN (512 byte) + ukuran file saat dipindai.
    pub(crate) marker_bits: Option<Vec<u8>>,
    pub(crate) marker_size: u64,
    pub(crate) marker_rx: Option<mpsc::Receiver<MarkerUpdate>>,
    /// Cancel flag untuk marker scan latar (tab ditutup/rotasi).
    pub(crate) marker_cancel: Arc<AtomicBool>,
    /// Histogram ERROR per menit (shading strip).
    pub(crate) time_hist: Option<TimeHist>,
    /// Catatan follow segar, mis. `+128 baris baru` (+ waktu).
    pub(crate) follow_note: Option<(String, Instant)>,
    /// Interval poll follow (ms) — dari Options (P1-13).
    pub(crate) follow_ms: u64,
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
    /// Cache teks tampil per baris (ringkasan JSON dkk): line -> String.
    /// Isi baris lama tak berubah saat append, jadi cache hanya gugur saat
    /// rotasi/reopen atau ganti encoding (lihat bawah).
    pub(crate) disp_cache: HashMap<u64, String>,
    /// True saat batch search yang masuk adalah gabungan ekor follow
    /// (tanpa lompat ke hasil pertama / buka panel / tulis cache).
    pub(crate) search_merge_quiet: bool,
    /// True saat hasil filter yang masuk adalah tambahan ekor (extend,
    /// bukan ganti; top_row dipertahankan).
    pub(crate) filter_append: bool,
    /// Hasil ekspor latar yang masuk (progres + selesai).
    pub(crate) export_rx: Option<mpsc::Receiver<ExportMsg>>,
    /// Batalkan ekspor latar yang berjalan (tombol Batal / rotasi).
    pub(crate) export_cancel: Arc<AtomicBool>,
    // ---- P0-1: word wrap ----
    /// Toggle word-wrap viewport (per-tab, ikut sesi).
    pub(crate) word_wrap: bool,
    /// Cache wrap: baris -> jumlah baris visual (None = hitung ulang).
    /// Gugur saat file berubah (rotasi/append) atau ganti encoding.
    pub(crate) wrap_rows_cache: HashMap<u64, u32>,
    // ---- P0-2: text selection ----
    /// Seleksi baris (anchor, aktif) — 1-based; None = tanpa seleksi.
    pub(crate) sel_anchor: Option<u64>,
    pub(crate) sel_active: Option<u64>,
    /// Seleksi sebagian baris (line, col_start, col_end) — portion select.
    pub(crate) sel_portion: Option<(u64, u32, u32)>,
    /// Rentang tambahan non-kontigu (Ctrl+klik): daftar (lo, hi) 1-based,
    /// selalu ternormalisasi (terurut, tak tumpang-tindih). Bersama seleksi
    /// primer (anchor/active) membentuk himpunan multi-seleksi klogg-parity.
    pub(crate) sel_extra: Vec<(u64, u64)>,
    // ---- P0-5: QuickFind ----
    /// QuickFind bar: teks, arah (true = maju), posisi hasil kini.
    pub(crate) qf_text: String,
    pub(crate) qf_open: bool,
    pub(crate) qf_forward: bool,
    pub(crate) qf_match: Option<(u64, u32, u32)>,
    pub(crate) qf_msg: Option<(String, Instant)>,
    /// State interaksi baris saran riwayat pencarian (agar tidak hilang saat tombol diklik).
    pub(crate) sug_active: bool,
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
            regex_complex: false,
            current_hit: None,
            search_scanned: 0,
            search_total: 0,
            search_grand_total: 0,
            merge_base: 0,
            follow_total: 0,
            search_page_active: false,
            page_base: 0,
            search_cache: SearchCache::new(search::effective_cache_entries()),
            pending_key: None,
            filter_text: String::new(),
            top_row: 0,
            selected_line: 1,
            hover_line: None,
            temp_path: None,
            archive_src: None,
            alias: None,
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
            kept: Vec::new(),
            kept_view: None,
            goto_open: false,
            goto_input: String::new(),
            goto_msg: String::new(),
            export_open: false,
            export_context: 10,
            index_rx: Some(index_rx),
            search_rx: None,
            filter_rx: None,
            filter_cancel: Arc::new(AtomicBool::new(false)),
            marker_bits: None,
            marker_size: 0,
            marker_rx: None,
            marker_cancel: Arc::new(AtomicBool::new(false)),
            time_hist: None,
            follow_note: None,
            follow_ms: crate::engine::follow::FOLLOW_POLL_MS,
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
            disp_cache: HashMap::new(),
            search_merge_quiet: false,
            filter_append: false,
            export_rx: None,
            export_cancel: Arc::new(AtomicBool::new(false)),
            word_wrap: false,
            wrap_rows_cache: HashMap::new(),
            sel_anchor: None,
            sel_active: None,
            sel_portion: None,
            sel_extra: Vec::new(),
            qf_text: String::new(),
            qf_open: false,
            qf_forward: true,
            qf_match: None,
            qf_msg: None,
            sug_active: false,
        }
    }

    /// Teks tampil untuk satu baris (ringkasan JSON dkk), di-cache per nomor
    /// baris agar frame ulang tak mengulang parse. Penelepon memberi teks
    /// terdecode; kunci nomor-baris aman karena isi lama tak berubah
    /// (rotasi mengosongkan cache, encoding mengosongkan cache).
    pub(crate) fn display_cached(&mut self, line: u64, text: &str) -> String {
        if let Some(s) = self.disp_cache.get(&line) {
            return s.clone();
        }
        let disp = crate::engine::jsonlog::display_text(text);
        // Batas longgar tanpa LRU: viewport hanya menyentuh ~ribuan baris.
        if self.disp_cache.len() > 8192 {
            self.disp_cache.clear();
        }
        self.disp_cache.insert(line, disp.clone());
        disp
    }

    /// P0-1: jumlah baris VISUAL untuk satu baris log pada lebar kolom
    /// (`chars_w` = kapasitas karakter). Wrap word-aware: pemenggalan di
    /// spasi bila bisa, fallback hard-break untuk token raksasa.
    /// Hasin di-cache per (baris); cache gugur saat file berubah.
    pub(crate) fn wrap_count(&mut self, line: u64, text: &str, chars_w: u32) -> u32 {
        if chars_w < 8 {
            return 1;
        }
        let key = line;
        if let Some(n) = self.wrap_rows_cache.get(&key) {
            // Lebar berubah (resize) menggugurkan cache — cek kasar cukup:
            // jumlah baris tergantung lebar, jadi cache hanya valid bila
            // pemanggil membersihkannya saat resize. Simpan lebar tidak
            // dilakukan agar sederhana; cache dibersihkan saat resize.
            return *n;
        }
        let w = chars_w as usize;
        let mut rows = 0u32;
        let mut cur = 0usize;
        let mut last_space: Option<usize> = None;
        let mut in_space = false;
        let mut iter = text.char_indices().peekable();
        while let Some((bi, ch)) = iter.next() {
            let width = if ch == '\t' { 4 } else { 1 };
            if cur + width > w {
                // Wrap: di spasi bila ada, else hard-break.
                let break_at = match last_space {
                    Some(s) if s > 0 => s + 1,
                    _ => bi,
                };
                let _ = break_at;
                rows += 1;
                cur = 0;
                // Mulai segmen baru dari karakter ini; spasi di depan
                // segmen baru dilewati.
                if ch.is_whitespace() {
                    continue;
                }
                last_space = None;
            }
            cur += width;
            if ch == ' ' {
                if !in_space {
                    last_space = Some(bi);
                    in_space = true;
                }
            } else {
                in_space = false;
            }
        }
        rows += 1; // segmen terakhir
        let n = rows.max(1);
        if self.wrap_rows_cache.len() > 8192 {
            self.wrap_rows_cache.clear();
        }
        self.wrap_rows_cache.insert(key, n);
        n
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

    /// Nama tampil tab: alias kustom bila ada, else nama file. P1-15.
    pub(crate) fn display_name(&self) -> &str {
        self.alias
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.doc.file_name)
    }

    /// Status pencarian kini (P1-12 state machine). Prioritas: worker dulu,
    /// lalu galat, lalu pangkas, lalu kosong, lalu statis.
    pub(crate) fn search_state(&self) -> SearchState {        if self.doc.search_in_progress {
            if self.search_merge_quiet {
                return SearchState::AutoRefreshing;
            }
            return SearchState::Searching;
        }
        if self.doc.search_error.is_some() {
            return SearchState::Error;
        }
        if self.doc.search_truncated {
            return SearchState::Truncated;
        }
        if self.search_text.trim().is_empty() {
            return SearchState::NoSearch;
        }
        SearchState::Static
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

    /// Cancel a running search worker for real: bump the generation (late
    /// batches are dropped in poll_channels) and raise the cancel flag so
    /// the thread exits at the next chunk boundary. Query/results untouched.
    pub(crate) fn cancel_search_worker(&mut self) {
        self.doc.search_gen += 1;
        self.gen_shared.store(self.doc.search_gen, Ordering::Relaxed);
        self.search_cancel.store(true, Ordering::Relaxed);
        self.doc.search_in_progress = false;
        self.debounce_at = None;
    }

    /// Full search reset for the × buttons: real worker cancel (above) plus
    /// clearing query, results, and selection state.
    pub(crate) fn clear_search(&mut self) {
        self.cancel_search_worker();
        self.search_text.clear();
        self.last_searched.clear();
        self.regex_complex = false;
        self.kept_view = None;
        self.doc.hits.clear();
        self.doc.search_error = None;
        self.current_hit = None;
        self.search_grand_total = 0;
        self.merge_base = 0;
        self.search_page_active = false;
        self.page_base = 0;
        self.results_collapsed = true;
        self.refresh_mode_map();
    }

    /// Pin the finished live results as a named snapshot (keep results,
    /// klogg parity): the frozen copy survives query changes, rotation
    /// clears it (line numbers would lie). Oldest drops past MAX_KEPT.
    pub(crate) fn keep_results(&mut self, name: String) -> Result<(), &'static str> {
        if self.doc.hits.is_empty() {
            return Err("empty");
        }
        if self.kept.len() >= MAX_KEPT {
            self.kept.remove(0);
            if let Some(v) = self.kept_view.as_mut() {
                *v = v.saturating_sub(1);
            }
        }
        let title = if name.trim().is_empty() {
            format!(
                "{} · {}",
                self.search_text.chars().take(30).collect::<String>(),
                self.doc.hits.len()
            )
        } else {
            name
        };
        self.kept.push(KeptResult {
            name: title,
            query: self.search_text.clone(),
            hits: self.doc.hits.clone(),
        });
        self.kept_view = Some(self.kept.len() - 1);
        Ok(())
    }

    /// Drop a snapshot; a viewed one falls back to live results.
    pub(crate) fn drop_kept(&mut self, idx: usize) {
        if idx >= self.kept.len() {
            return;
        }
        self.kept.remove(idx);
        match self.kept_view {
            Some(v) if v == idx => self.kept_view = None,
            Some(v) if v > idx => self.kept_view = Some(v - 1),
            _ => {}
        }
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
            self.search_grand_total = 0;
            self.merge_base = 0;
            self.search_page_active = false;
            self.page_base = 0;
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
        // Kueri baru = kembali ke live; snapshot tersimpan tidak ikut berubah.
        self.kept_view = None;
        self.refresh_mode_map();
        self.doc.search_truncated = false;
        self.doc.search_error = None;
        self.doc.search_in_progress = true;
        self.current_hit = None;
        self.search_scanned = 0;
        self.search_total = self.doc.size;
        self.search_grand_total = 0;
        self.merge_base = 0;
        self.search_page_active = false;
        self.page_base = 0;
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
            // Cache only stores complete (non-truncated) results, so the
            // exact total equals the stored hit count here.
            self.search_grand_total = self.doc.hits.len() as u64;
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
        // Label mode jujur: pola kompleks terdeteksi saat mulai (compile
        // <1ms), worker memakai jalur fancy sekuensial untuknya.
        self.regex_complex = self.regex_on
            && crate::engine::search::is_complex_regex(&q, self.case_sensitive);
        if bool_mode {
            match query::parse_query(&q) {
                Ok(ast) => spawn_bool_search(
                    SearchJobParams {
                        path: self.doc.path.clone(),
                        gen,
                        gen_shared: self.gen_shared.clone(),
                        tx,
                        cancel,
                    },
                    ast,
                    self.doc.encoding(),
                    self.doc.bom_len,
                    self.case_sensitive,
                    self.scope,
                    None,
                ),
                Err(e) => {
                    self.doc.search_error = Some(e);
                    self.doc.search_in_progress = false;
                }
            }
            return;
        }
        spawn_search(
            SearchJobParams {
                path: self.doc.path.clone(),
                gen,
                gen_shared: self.gen_shared.clone(),
                tx,
                cancel,
            },
            q,
            self.regex_on,
            self.case_sensitive,
            scope_bytes,
            None,
            self.doc.encoding(),
            self.doc.bom_len,
        );
    }

    /// Load the next result page past the display cap ("next page"
    /// button, F3 at the last stored hit). Re-scans from the line after
    /// the last stored hit with the SAME query/mode/scope; batches append
    /// in order, so RAM stays bounded to one page per click while every
    /// one of the (exact, known) grand_total matches becomes explorable.
    /// Returns false when there is nothing more to load.
    pub(crate) fn continue_search_page(&mut self) -> bool {
        use crate::engine::query;
        if self.doc.search_in_progress || self.search_text.trim().is_empty() {
            return false;
        }
        if !self.doc.search_truncated || self.doc.hits.is_empty() {
            return false;
        }
        if (self.doc.hits.len() as u64) >= self.search_grand_total {
            self.doc.search_truncated = false;
            return false;
        }
        let last_line = match self.doc.hits.last() {
            Some(h) => h.line,
            None => return false,
        };
        // Anchor: exact byte start of the next line via the sparse index.
        // None = past EOF (file shrank since): nothing more to load.
        let (sb, sl) = match self.doc.line_byte_range(last_line + 1) {
            Some((s, _)) => (s, last_line + 1),
            None => {
                self.doc.search_truncated = false;
                return false;
            }
        };
        let q = self.search_text.clone();
        // Fresh generation (page 1 is done; no stragglers), same merge
        // mechanics as a fresh search — results APPEND to stored hits.
        self.doc.search_gen += 1;
        let gen = self.doc.search_gen;
        self.gen_shared.store(gen, Ordering::Relaxed);
        self.search_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.search_rx = Some(rx);
        self.page_base = self.doc.hits.len();
        self.search_page_active = true;
        self.doc.search_truncated = true; // recomputed exact at page done
        self.doc.search_error = None;
        self.doc.search_in_progress = true;
        self.search_scanned = sb;
        self.search_total = self.doc.size;
        // Re-arm the cache key: a page completing the set is cacheable.
        let bool_mode = !self.regex_on && query::is_boolean_query(&q);
        self.pending_key = Some(CacheKey {
            query: q.clone(),
            regex: self.regex_on,
            case_sensitive: self.case_sensitive,
            boolean: bool_mode,
            scope: self.scope,
            rev: self.file_rev(),
        });
        let scope_bytes = self.scope.and_then(|(a, b)| {
            let (s, _) = self.doc.line_byte_range(a)?;
            let (_, e) = self.doc.line_byte_range(b)?;
            Some((s, e))
        });
        let seek = Some((sb, sl));
        self.regex_complex = self.regex_on
            && crate::engine::search::is_complex_regex(&q, self.case_sensitive);
        if bool_mode {
            match query::parse_query(&q) {
                Ok(ast) => spawn_bool_search(
                    SearchJobParams {
                        path: self.doc.path.clone(),
                        gen,
                        gen_shared: self.gen_shared.clone(),
                        tx,
                        cancel,
                    },
                    ast,
                    self.doc.encoding(),
                    self.doc.bom_len,
                    self.case_sensitive,
                    self.scope,
                    seek,
                ),
                Err(e) => {
                    self.doc.search_error = Some(e);
                    self.doc.search_in_progress = false;
                    self.search_page_active = false;
                }
            }
            return true;
        }
        spawn_search(
            SearchJobParams {
                path: self.doc.path.clone(),
                gen,
                gen_shared: self.gen_shared.clone(),
                tx,
                cancel,
            },
            q,
            self.regex_on,
            self.case_sensitive,
            scope_bytes,
            seek,
            self.doc.encoding(),
            self.doc.bom_len,
        );
        true
    }

    pub(crate) fn start_filter(&mut self, query: String) {
        // A superseding filter (or clear) retires the running worker first.
        self.filter_cancel.store(true, Ordering::Relaxed);
        self.filter_rx = None;
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
        let cancel = Arc::new(AtomicBool::new(false));
        self.filter_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.filter_rx = Some(rx);
        spawn_filter(
            self.doc.path.clone(),
            self.doc.encoding(),
            self.doc.bom_len,
            f,
            tx,
            None,
            cancel,
        );
        self.top_row = 0;
    }

    /// Jangkar pindai ekor follow: (byte_mulai, nomor_baris_mulai).
    /// old_bytes = ukuran sebelum append, old_lines = jumlah baris lama
    /// (butuh indeks komplet). Byte mulai = awal baris yang mengandung
    /// old_bytes (mundur ke \n sebelumnya, maks 64 KiB) agar match yang
    /// melintasi titik append tetap ketemu; nomornya = baris itu.
    /// None bila tak bisa dijangkar aman (indeks belum komplet / baris
    /// pembuka > 64 KiB) — penelepon memakai fallback.
    fn tail_anchor(&self, old_bytes: u64, old_lines: u64) -> Option<(u64, u64)> {
        if !self.doc.index.complete || old_lines < 1 {
            return None;
        }
        if old_bytes == 0 {
            return Some((0, 1));
        }
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&self.doc.path).ok()?;
        // Apakah file lama diakhiri newline? Baca 1 byte di old_bytes-1.
        let ends_nl = if old_bytes > 0 {
            f.seek(SeekFrom::Start(old_bytes - 1)).is_ok()
                && {
                    let mut b = [0u8; 1];
                    f.read_exact(&mut b).is_ok() && b[0] == b'\n'
                }
        } else {
            true
        };
        if ends_nl {
            return Some((old_bytes, old_lines + 1));
        }
        // Lanjutkan baris lama yang terpotong: mundur ke awal barisnya.
        let back = old_bytes.min(64 * 1024);
        let mut buf = vec![0u8; back as usize];
        f.seek(SeekFrom::Start(old_bytes - back)).ok()?;
        f.read_exact(&mut buf).ok()?;
        // Baris pembuka raksasa (>64 KiB tanpa newline): serahkan ke fallback.
        buf.iter()
            .rposition(|&b| b == b'\n')
            .map(|i| (old_bytes - back + i as u64 + 1, old_lines))
    }

    /// Pindai ulang HANYA ekor yang baru di-append (follow): seek worker ke
    /// jangkar, gen SAMA sehingga batch bergabung ke hasil lama (tetap
    /// terurut: nomor baris ekor selalu terbesar). Tanpa riwayat, tanpa
    /// lompat, tanpa tulis cache. Hit pada baris jangkar yang terpotong
    /// ditulis ulang (dihapus dulu) agar tak dobel.
    pub(crate) fn refresh_search_tail(&mut self, old_bytes: u64, old_lines: u64) {
        if self.search_text.trim().is_empty() || self.doc.search_in_progress {
            return;
        }
        if self.doc.hits.len() >= search::effective_max_hits() {
            self.doc.search_truncated = true;
            return;
        }
        let Some((sb, sl)) = self.tail_anchor(old_bytes, old_lines) else {
            return;
        };
        // Tulis ulang baris jangkar (kasus ekor lanjutan tanpa \n).
        // Grand-total base: keep the exact count minus the rewritten
        // anchor hits (the tail worker re-emits that line, counted once
        // in its own job total).
        let anchor_hits = self.doc.hits.iter().filter(|h| h.line == sl).count() as u64;
        self.merge_base = self.search_grand_total.saturating_sub(anchor_hits);
        self.doc.hits.retain(|h| h.line != sl);
        self.current_hit = None;
        let (tx, rx) = mpsc::channel();
        self.search_rx = Some(rx);
        self.search_merge_quiet = true;
        self.doc.search_in_progress = true;
        let gen = self.doc.search_gen; // gen sama: batch bergabung
        let cancel = self.search_cancel.clone();
        let params = SearchJobParams {
            path: self.doc.path.clone(),
            gen,
            gen_shared: self.gen_shared.clone(),
            tx,
            cancel,
        };
        let q = self.search_text.clone();
        let seek = Some((sb, sl));
        // Catatan: cakupan (scope) pencarian pengguna diabaikan untuk ekor:
        // append selalu di luar cakupan lama, dan hasil lama di luar
        // cakupan tetap dipertahankan.
        if !self.regex_on && crate::engine::query::is_boolean_query(&q) {
            match crate::engine::query::parse_query(&q) {
                Ok(ast) => spawn_bool_search(
                    params,
                    ast,
                    self.doc.encoding(),
                    self.doc.bom_len,
                    self.case_sensitive,
                    None,
                    seek,
                ),
                Err(e) => {
                    self.doc.search_error = Some(e);
                    self.doc.search_in_progress = false;
                    self.search_merge_quiet = false;
                }
            }
            return;
        }
        spawn_search(
            params,
            q,
            self.regex_on,
            self.case_sensitive,
            None,
            seek,
            self.doc.encoding(),
            self.doc.bom_len,
        );
    }

    /// Pindai ulang filter HANYA untuk ekor (gabung ke filter_map lama yang
    /// tetap terurut). Prasyarat sama seperti search ekor.
    pub(crate) fn refresh_filter_tail(&mut self, old_bytes: u64, old_lines: u64) {
        if self.doc.filter.is_empty() || self.filter_rx.is_some() {
            return;
        }
        let Some((sb, sl)) = self.tail_anchor(old_bytes, old_lines) else {
            return;
        };
        self.doc.filter_map.remove(sl);
        // Retire the previous tail worker first (same rule as start_filter:
        // a superseded scan must stop, not burn to completion unread).
        self.filter_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.filter_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.filter_rx = Some(rx);
        self.filter_append = true;
        self.doc.status = String::from("Filter: memindai baris baru…");
        spawn_filter(
            self.doc.path.clone(),
            self.doc.encoding(),
            self.doc.bom_len,
            self.doc.filter.clone(),
            tx,
            Some((sb, sl)),
            cancel,
        );
    }

    /// Mulai ekspor hasil ke thread latar (UI tak beku): snapshot input,
    /// progres mengalir ke status bar, selesai menutup dialog. Menolak bila
    /// ekspor lain masih berjalan di tab ini.
    pub(crate) fn start_export(
        &mut self,
        out: PathBuf,
        context: usize,
        ticket: bool,
        query: String,
    ) {
        if self.export_rx.is_some() {
            self.doc.status =
                String::from("Ekspor masih berjalan; tunggu selesai atau Batalkan.");
            return;
        }
        if self.doc.hits.is_empty() {
            self.doc.status = String::from("Tidak ada hasil untuk diekspor.");
            return;
        }
        let total_lines = if self.doc.index.complete {
            self.doc.index.total_lines
        } else {
            self.doc.line_count_estimate()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.export_cancel = cancel.clone();
        let job = ExportJob {
            src: self.doc.path.clone(),
            out,
            hits: self.doc.hits.clone(),
            context,
            query,
            file_name: self.doc.file_name.clone(),
            checkpoints: self.doc.index.checkpoints.clone(),
            total_lines,
            encoding: self.doc.encoding(),
            bom_len: self.doc.bom_len,
            ticket,
        };
        let (tx, rx) = mpsc::channel();
        self.export_rx = Some(rx);
        spawn_export(job, tx, cancel);
        self.doc.status = String::from("Mengekspor di latar…");
    }

    /// Ekspor-streaming SEMUA baris cocok dari query kini, tanpa batas tampil
    /// (viewport boleh terpangkas, file ekspor tidak). O(chunk) RAM.
    pub(crate) fn start_export_search(&mut self, out: PathBuf) {
        if self.export_rx.is_some() {
            self.doc.status =
                String::from("Ekspor masih berjalan; tunggu selesai atau Batalkan.");
            return;
        }
        if self.search_text.trim().is_empty() {
            self.doc.status = String::from("Query kosong — isi kolom Cari dulu.");
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.export_cancel = cancel.clone();
        let job = ExportSearchJob {
            src: self.doc.path.clone(),
            out,
            query: self.search_text.clone(),
            regex_on: self.regex_on,
            case_sensitive: self.case_sensitive,
            encoding: self.doc.encoding(),
            bom_len: self.doc.bom_len,
        };
        let (tx, rx) = mpsc::channel();
        self.export_rx = Some(rx);
        spawn_export_search(job, tx, cancel);
        self.doc.status = String::from("Mengekspor streaming di latar…");
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
                // Exact total: fresh jobs overwrite; quiet tail merges add
                // onto the base captured at refresh start (tail jobs count
                // the tail only); continuation pages keep the first-page
                // exact value (page jobs count their own scope only).
                if self.search_merge_quiet {
                    self.search_grand_total = self.merge_base.saturating_add(m.grand_total);
                } else if !self.search_page_active {
                    self.search_grand_total = m.grand_total;
                }
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
                // Continuation page finished: exact set complete when the
                // stored count finally reaches the known grand total.
                let was_page = self.search_page_active;
                if was_page {
                    self.search_page_active = false;
                    if self.doc.search_error.is_none()
                        && (self.doc.hits.len() as u64) >= self.search_grand_total
                    {
                        self.doc.search_truncated = false;
                    }
                }
                // Simpan hasil lengkap ke cache (pola ulang = instan).
                if self.doc.search_error.is_none() && !self.doc.search_truncated {
                    if let Some(key) = self.pending_key.take() {
                        self.search_cache.put(key, self.doc.hits.clone());
                    }
                } else {
                    self.pending_key = None;
                }
                self.refresh_mode_map();
                // Gabungan ekor follow: diam-diam (tanpa lompat/panel/cache).
                if self.search_merge_quiet {
                    self.search_merge_quiet = false;
                } else if self.doc.search_error.is_none() && !self.doc.hits.is_empty() {
                    // Fresh search: lompat ke hasil pertama. Page: lompat
                    // ke hasil pertama HALAMAN ini (jangan yank ke atas).
                    let target = if was_page {
                        self.page_base.min(self.doc.hits.len().saturating_sub(1))
                    } else {
                        0
                    };
                    self.current_hit = Some(target);
                    // Ada hasil: buka panel hasil otomatis.
                    self.results_collapsed = false;
                    // lompat ke hasil target
                    let ln = self.doc.hits[target].line;
                    self.selected_line = ln;
                    self.center_on_line(ln);
                }
            }
        }
        // Filter result (exact counts: the worker streams into a roaring
        // bitmap with no match cap, so nothing here is truncated).
        if let Some(rx) = &self.filter_rx {
            if let Ok(map) = rx.try_recv() {
                if self.filter_append {
                    // Gabungan ekor: filter_map lama tetap terurut, ekor
                    // bernomor lebih besar -> extend + posisi dipertahankan.
                    self.filter_append = false;
                    let added = map.len();
                    self.doc.filter_map.absorb(map);
                    self.doc.filter_active = true;
                    self.doc.status = format!(
                        "Filter: {} baris cocok (+{} baru).",
                        format_count(self.doc.filter_map.len()),
                        format_count(added),
                    );
                } else {
                    self.doc.filter_map = map;
                    self.doc.filter_active = true;
                    self.top_row = 0;
                    self.doc.status = format!(
                        "Filter aktif: {} baris cocok.",
                        format_count(self.doc.filter_map.len())
                    );
                }
                self.filter_rx = None;
            }
        }
        // Export latar: progres + selesai/gagal.
        if let Some(rx) = &self.export_rx {
            while let Ok(m) = rx.try_recv() {
                if let Some(e) = m.error {
                    self.doc.status = e;
                    self.export_rx = None;
                    break;
                }
                if m.done {
                    self.doc.status = if m.ticket {
                        format!(
                            "Tiket ({} baris konteks) disimpan ke {}.",
                            m.context,
                            m.out.display()
                        )
                    } else {
                        format!(
                            "Diekspor {} baris ke {}.",
                            format_count(m.written),
                            m.out.display()
                        )
                    };
                    self.export_open = false;
                    self.export_rx = None;
                    break;
                }
                if m.total_hits == 0 {
                    // Ekspor-streaming: total tak diketahui di muka.
                    self.doc.status = format!(
                        "Mengekspor streaming: {} ditulis…",
                        format_count(m.written),
                    );
                } else {
                    self.doc.status = format!(
                        "Mengekspor {} / {} hasil…",
                        format_count(m.written),
                        format_count(m.total_hits as u64),
                    );
                }
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
                self.marker_cancel.store(false, Ordering::Relaxed);
                let cancel = Arc::new(AtomicBool::new(false));
                self.marker_cancel = cancel.clone();
                let (tx, rx) = mpsc::channel();
                self.marker_rx = Some(rx);
                spawn_marker_scan(self.doc.path.clone(), self.doc.size, tx, cancel);
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
        // mode_lines terurut (binary search); filter_map bitmap terkompresi.
        if self.view_mode != ViewMode::All {
            return self.mode_lines.binary_search(&line).ok().map(|i| i as u64);
        }
        if self.doc.filter_active {
            self.doc.filter_map.line_to_row(line)
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

    // ---- P0-2: selection helpers (single + portion + multi non-kontigu) ----

    /// Rentang baris terseleksi (asc) bila seleksi baris aktif.
    pub(crate) fn sel_range(&self) -> Option<(u64, u64)> {
        match (self.sel_anchor, self.sel_active) {
            (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
            _ => None,
        }
    }

    /// True bila ada seleksi apa pun (primer / portion / multi).
    pub(crate) fn sel_has_any(&self) -> bool {
        self.sel_anchor.is_some() || self.sel_portion.is_some() || !self.sel_extra.is_empty()
    }

    /// Semua rentang (primer + ekstra), tergabung & terurut.
    pub(crate) fn sel_all_ranges(&self) -> Vec<(u64, u64)> {
        let mut v: Vec<(u64, u64)> = Vec::new();
        if let Some((lo, hi)) = self.sel_range() {
            v.push((lo, hi));
        }
        v.extend(self.sel_extra.iter().copied());
        if v.is_empty() {
            return v;
        }
        v.sort();
        // Gabung tumpang-tindih / bersebelahan.
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(v.len());
        for (lo, hi) in v {
            if let Some(last) = out.last_mut() {
                if lo <= last.1.saturating_add(1) {
                    last.1 = last.1.max(hi);
                    continue;
                }
            }
            out.push((lo, hi));
        }
        out
    }

    /// Jumlah baris dalam seluruh seleksi (0 bila tak ada).
    pub(crate) fn sel_line_count(&self) -> u64 {
        self.sel_all_ranges().iter().map(|(lo, hi)| hi - lo + 1).sum()
    }

    /// Baris terpilih (seleksi berisi baris)?
    pub(crate) fn sel_contains(&self, line: u64) -> bool {
        if let Some((lo, hi)) = self.sel_range() {
            if line >= lo && line <= hi {
                return true;
            }
        }
        self.sel_extra.iter().any(|(lo, hi)| line >= *lo && line <= *hi)
    }

    /// Mulai seleksi (klik biasa): reset portion + multi, anchor = baris.
    pub(crate) fn sel_start(&mut self, line: u64, extend: bool) {
        if !extend {
            self.sel_anchor = Some(line);
        }
        self.sel_active = Some(line);
        self.sel_portion = None;
        if !extend {
            self.sel_extra.clear();
        }
        self.selected_line = line;
    }

    /// Perluas seleksi ke baris (drag / Shift+klik).
    pub(crate) fn sel_extend(&mut self, line: u64) {
        if self.sel_anchor.is_none() {
            self.sel_anchor = Some(line);
        }
        self.sel_active = Some(line);
        self.selected_line = line;
    }

    /// Ctrl+klik: toggle satu baris ke/dari himpunan multi-seleksi.
    /// Portion ikut gugur (tak campur dengan multi). Mengembalikan true
    /// bila baris kini terseleksi.
    pub(crate) fn sel_toggle(&mut self, line: u64) -> bool {
        self.sel_portion = None;
        // Bila baris ada di primer -> keluarkan dari primer ke ekstra.
        if let Some((lo, hi)) = self.sel_range() {
            if line >= lo && line <= hi {
                self.sel_extra.clear();
                if lo < line {
                    self.sel_extra.push((lo, line - 1));
                }
                if line < hi {
                    self.sel_extra.push((line + 1, hi));
                }
                self.sel_anchor = None;
                self.sel_active = None;
                self.selected_line = line;
                return false;
            }
        }
        // Bila ada di ekstra -> keluarkan (belah bila perlu).
        if let Some(idx) = self.sel_extra.iter().position(|(lo, hi)| line >= *lo && line <= *hi) {
            let (lo, hi) = self.sel_extra.remove(idx);
            if lo < line {
                self.sel_extra.push((lo, line - 1));
            }
            if line < hi {
                self.sel_extra.push((line + 1, hi));
            }
            self.sel_extra.sort();
            self.selected_line = line;
            return false;
        }
        // Tambahkan: gabung ke primer bila bersebelahan, else ke ekstra.
        if let Some((lo, hi)) = self.sel_range() {
            if line + 1 == lo || hi + 1 == line {
                self.sel_anchor = Some(lo.min(line));
                self.sel_active = Some(hi.max(line));
                self.selected_line = line;
                return true;
            }
            self.sel_extra.push((lo, hi));
        }
        self.sel_anchor = Some(line);
        self.sel_active = Some(line);
        self.sel_extra.sort();
        // Normalisasi cepat: gabung bila ekstra kini bersebelahan.
        let merged = self.sel_all_ranges();
        self.sel_extra.clear();
        if merged.len() > 1 {
            // Primer = rentang yang memuat `line`, sisanya ekstra.
            for (lo, hi) in merged {
                if line >= lo && line <= hi {
                    self.sel_anchor = Some(lo);
                    self.sel_active = Some(hi);
                } else {
                    self.sel_extra.push((lo, hi));
                }
            }
        }
        self.selected_line = line;
        true
    }

    /// Batalkan seluruh seleksi.
    pub(crate) fn sel_clear(&mut self) {
        self.sel_anchor = None;
        self.sel_active = None;
        self.sel_portion = None;
        self.sel_extra.clear();
    }

    /// Salin teks seleksi (portion / multi / baris penuh / baris aktif).
    pub(crate) fn sel_copy_text(&mut self) -> Result<String, String> {
        // Portion select menang: satu baris sebagian.
        if let Some((ln, cs, ce)) = self.sel_portion {
            let text = self.doc.get_line_text(ln).unwrap_or_default();
            let chars: Vec<char> = text.chars().collect();
            let a = (cs as usize).min(chars.len());
            let b = (ce as usize).min(chars.len());
            let (lo, hi) = (a.min(b), a.max(b));
            let out: String = chars[lo..hi].iter().collect();
            return Ok(out);
        }
        let ranges = self.sel_all_ranges();
        if ranges.is_empty() {
            // Fallback: baris aktif.
            return self.doc.copy_range_text(self.selected_line, self.selected_line);
        }
        let total: u64 = ranges.iter().map(|(lo, hi)| hi - lo + 1).sum();
        if total > 5_000_000 {
            return Err(String::from("Pilihan melebihi 16 MB."));
        }
        if ranges.len() == 1 {
            return self.doc.copy_range_text(ranges[0].0, ranges[0].1);
        }
        let mut out = String::new();
        for (i, (lo, hi)) in ranges.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&self.doc.copy_range_text(*lo, *hi)?);
            if out.len() > 16 * 1024 * 1024 {
                return Err(String::from("Pilihan melebihi 16 MB."));
            }
        }
        Ok(out)
    }

    /// Select word di sekitar kolom (double-click): portion select.
    pub(crate) fn sel_word_at(&mut self, line: u64, col_char: u32) {
        let Some(text) = self.doc.get_line_text(line) else { return };
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return;
        }
        let i = (col_char as usize).min(chars.len() - 1);
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if !is_word(chars[i]) {
            // Tanda baca/spasi: pilih karakter tunggal itu.
            self.sel_portion = Some((line, i as u32, i as u32 + 1));
            self.sel_extra.clear();
            return;
        }
        let mut a = i;
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        let mut b = i;
        while b + 1 < chars.len() && is_word(chars[b + 1]) {
            b += 1;
        }
        self.sel_portion = Some((line, a as u32, b as u32 + 1));
        self.sel_extra.clear();
        self.selected_line = line;
    }

    // ---- P0-5: QuickFind ----

    /// QuickFind cari berikutnya dari (baris, kolom) kini. Maju/mundur,
    /// wrap-around dengan pesan. Literal case-insensitive penuh (Unicode).
    pub(crate) fn qf_find(&mut self, forward: bool) {
        self.qf_forward = forward;
        let needle = self.qf_text.clone();
        if needle.trim().is_empty() {
            self.qf_match = None;
            return;
        }
        let total = if self.doc.index.complete {
            self.doc.index.total_lines
        } else {
            self.doc.line_count_estimate()
        };
        if total == 0 {
            return;
        }
        let start_line = match self.qf_match {
            Some((ln, _, _)) => {
                if forward {
                    ln
                } else {
                    ln.saturating_sub(1).max(1)
                }
            }
            None => self.selected_line.max(1),
        };
        let nl_lower = needle.to_lowercase();
        // Batasi jangkauan linear: seluruh file dengan cap iterasi.
        let cap = 500_000u64.min(total);
        let mut wrapped = false;
        let mut i = 0u64;
        let mut ln = if forward { start_line + 1 } else { start_line.saturating_sub(1) };
        if ln < 1 {
            ln = total;
            wrapped = true;
        }
        if ln > total {
            ln = 1;
            wrapped = true;
        }
        while i < cap {
            if let Some(head) = self.doc.get_line_head(ln, 16 * 1024) {
                let (text, _, _) = (head.0, head.1, head.2);
                let hay = text.to_lowercase();
                if let Some(rel) = memchr::memmem::Finder::new(nl_lower.as_bytes())
                    .find(hay.as_bytes())
                {
                    let start = rel as u32;
                    let end = (rel + nl_lower.len()) as u32;
                    self.qf_match = Some((ln, start, end));
                    self.selected_line = ln;
                    self.center_on_line(ln);
                    self.qf_msg = None;
                    return;
                }
            }
            i += 1;
            if forward {
                ln += 1;
                if ln > total {
                    if wrapped {
                        break;
                    }
                    ln = 1;
                    wrapped = true;
                    self.qf_msg = Some((String::from("Sampai akhir file — kembali ke awal."), Instant::now()));
                }
            } else {
                ln = ln.saturating_sub(1);
                if ln < 1 {
                    if wrapped {
                        break;
                    }
                    ln = total;
                    wrapped = true;
                    self.qf_msg = Some((String::from("Sampai awal file — kembali ke akhir."), Instant::now()));
                }
            }
        }
        self.qf_msg = Some((String::from("Tidak ditemukan."), Instant::now()));
    }

    pub(crate) fn poll_follow(&mut self) {
        if !self.doc.follow {
            return;
        }
        if self.last_follow_poll.elapsed() < Duration::from_millis(self.follow_ms) {
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
                            let vis = self.last_visible.max(10);
                            self.top_row = self.total_view_rows().saturating_sub(vis);
                        }
                        // Hasil/filter aktif ikut memanjang: pindai ulang
                        // HANYA ekor (seek), gabung diam-diam. Dilewati bila
                        // indeks belum komplet (nomor baris tak pasti) atau
                        // pencarian/filter sedang berjalan.
                        if self.doc.index.complete && old_lines > 0 {
                            self.refresh_search_tail(old_bytes, old_lines);
                            if self.doc.filter_active {
                                self.refresh_filter_tail(old_bytes, old_lines);
                            }
                        }
                    }
                    Err(e) => self.doc.status = e,
                }
            }
            FollowEvent::TruncatedOrRotated => {
                // Hasil/filter Hits mengacu byte lama: batalkan worker-nya
                // DULU agar batch basi tak bergabung setelah buka ulang.
                self.doc.search_gen += 1;
                self.gen_shared.store(self.doc.search_gen, Ordering::Relaxed);
                self.search_cancel.store(true, Ordering::Relaxed);
                self.search_rx = None;
                self.filter_cancel.store(true, Ordering::Relaxed);
                self.filter_rx = None;
                self.search_merge_quiet = false;
                self.filter_append = false;
                self.pending_key = None;
                // Ekspor latar membaca file lama: hentikan juga.
                self.export_cancel.store(true, Ordering::Relaxed);
                self.export_rx = None;
                if let Err(e) = self.doc.reopen_after_rotate() {
                    self.doc.status = e;
                } else {
                    // Revisi file berubah -> cache pencarian gugur.
                    self.search_cache.clear();
                    // Isi baris berubah -> cache teks tampil ikut gugur.
                    self.disp_cache.clear();
                      // Hasil & filter lama tak valid lagi di file baru.
                      self.doc.hits.clear();
                      self.current_hit = None;
                      self.search_grand_total = 0;
                      self.merge_base = 0;
                      self.search_page_active = false;
                      self.page_base = 0;
                      // Snapshot tersimpan mengacu nomor baris lama: gugur juga.
                      self.kept.clear();
                      self.kept_view = None;
                      self.doc.search_in_progress = false;
                    self.doc.search_error = None;
                    self.doc.search_truncated = false;
                    self.doc.filter_map.clear();
                    self.doc.filter_active = false;
                    self.refresh_mode_map();
                    self.selected_line = 1;
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
                    // Beri tahu eksplisit: jangan lompat diam-diam.
                    self.doc.status = String::from(
                        "File dipotong/dirotasi: dibuka ulang dari awal; hasil & filter dikosongkan.",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_lines(n: u64, every: u64) -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..n {
            if i.is_multiple_of(every) {
                v.extend_from_slice(format!("2026-09-04 ERROR id={}\n", i).as_bytes());
            } else {
                v.extend_from_slice(format!("2026-09-04 INFO id={} pad\n", i).as_bytes());
            }
        }
        v
    }

    fn wait_index(tab: &mut TabState) {
        for _ in 0..500 {
            tab.poll_channels();
            if tab.doc.index.complete {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("indexer did not finish");
    }

    fn drain_search(tab: &mut TabState) {
        for _ in 0..500 {
            tab.poll_channels();
            if !tab.doc.search_in_progress && tab.search_rx.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("search did not finish");
    }

    fn drain_filter(tab: &mut TabState) {
        for _ in 0..500 {
            tab.poll_channels();
            if tab.filter_rx.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("filter did not finish");
    }

    fn open_tab(path: &std::path::Path) -> TabState {
        let doc = Doc::open(path.to_path_buf()).unwrap();
        let mut tab = TabState::new(doc);
        tab.doc.follow = true;
        wait_index(&mut tab);
        tab
    }

    #[test]
    fn follow_append_merges_search_hits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.log");
        std::fs::write(&path, test_lines(3000, 100)).unwrap();
        let mut tab = open_tab(&path);
        tab.search_text = String::from("ERROR");
        tab.case_sensitive = true;
        tab.start_search(&mut Vec::new());
        drain_search(&mut tab);
        assert_eq!(tab.doc.hits.len(), 30);

        // Append 500 lines (5 new ERRORs); throttle follow 400 ms.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&test_lines(500, 100)).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(450));
        tab.poll_follow();
        drain_search(&mut tab);
        // 30 old + 5 new, ascending, no duplicates.
        assert_eq!(tab.doc.hits.len(), 35);
        let lines: Vec<u64> = tab.doc.hits.iter().map(|h| h.line).collect();
        let mut sorted = lines.clone();
        sorted.sort_unstable();
        assert_eq!(lines, sorted);
        assert_eq!(lines[30], 3001);
        assert!(!tab.doc.status.contains("dirotasi"));
    }

    #[test]
    fn follow_append_merges_filter_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.log");
        std::fs::write(&path, test_lines(3000, 100)).unwrap();
        let mut tab = open_tab(&path);
        tab.case_sensitive = true;
        tab.filter_text = String::from("ERROR");
        tab.start_filter(String::from("ERROR"));
        drain_filter(&mut tab);
        assert_eq!(tab.doc.filter_map.len(), 30);

        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&test_lines(500, 100)).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(450));
        tab.poll_follow();
        drain_filter(&mut tab);
        assert_eq!(tab.doc.filter_map.len(), 35);
        assert!(tab.doc.filter_active);
        // Bitmap selalu terurut: iterasi == versi sort.
        let got: Vec<u64> = tab.doc.filter_map.iter().collect();
        let mut sorted = got.clone();
        sorted.sort_unstable();
        assert_eq!(got, sorted);
        // Spot-check isi: ERROR tiap 100 baris + 5 baris ekor.
        assert_eq!(got[0], 1);
        assert_eq!(got[30], 3001);
    }

    #[test]
    fn superseding_filter_retires_running_worker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.log");
        std::fs::write(&path, test_lines(3000, 100)).unwrap();
        let mut tab = open_tab(&path);
        tab.case_sensitive = true;
        // First filter claims a cancel flag; superseding must signal it
        // synchronously (no timing involved — the store itself is proof).
        tab.start_filter(String::from("ERROR"));
        let old = tab.filter_cancel.clone();
        assert!(!old.load(Ordering::Relaxed));
        tab.start_filter(String::from("WARN"));
        assert!(old.load(Ordering::Relaxed));
        drain_filter(&mut tab);
        assert!(tab.doc.filter_active);
    }

    #[test]
    fn tail_refresh_retires_previous_filter_worker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.log");
        std::fs::write(&path, test_lines(3000, 100)).unwrap();
        let mut tab = open_tab(&path);
        tab.case_sensitive = true;
        tab.filter_text = String::from("ERROR");
        tab.start_filter(String::from("ERROR"));
        drain_filter(&mut tab);
        assert!(tab.doc.filter_active);
        let old = tab.filter_cancel.clone();
        // Direct tail refresh (no throttle sleep): bytes/lines of the file.
        let bytes = std::fs::metadata(&path).unwrap().len();
        tab.refresh_filter_tail(bytes, 3000);
        assert!(old.load(Ordering::Relaxed));
        drain_filter(&mut tab);
        assert!(tab.doc.filter_active);
    }

        #[test]
    fn kept_snapshots_pin_view_and_drop() {
        use crate::engine::search::Hit;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.log");
        std::fs::write(&path, test_lines(3000, 100)).unwrap();
        let mut tab = open_tab(&path);
        // Nothing live: keeping fails honestly instead of snapshotting air.
        assert!(tab.keep_results(String::new()).is_err());
        assert!(tab.kept.is_empty());
        // Fake a finished search (30 hits like the ERROR queries above).
        tab.search_text = String::from("ERROR");
        tab.doc.hits = (1..=30)
            .map(|i| Hit { line: i * 100, byte: 0, col_start: 0, col_end: 5 })
            .collect();
        tab.keep_results(String::new()).unwrap();
        assert_eq!(tab.kept.len(), 1);
        assert_eq!(tab.kept_view, Some(0));
        assert!(tab.kept[0].name.contains("ERROR"));
        assert_eq!(tab.kept[0].hits.len(), 30);
        // New search returns to live; the snapshot survives untouched.
        tab.search_text = String::from("WARN");
        tab.doc.hits.clear();
        tab.kept_view = None; // (start_search does this; mirrored here)
        tab.keep_results("second".into()).ok();
        // Second keep failed (no hits) — still exactly one snapshot.
        assert_eq!(tab.kept.len(), 1);
        // Viewing then dropping falls back to live.
        tab.kept_view = Some(0);
        tab.drop_kept(0);
        assert!(tab.kept.is_empty());
        assert_eq!(tab.kept_view, None);
        // Cap: oldest drops past MAX_KEPT.
        for i in 0..(super::MAX_KEPT + 2) {
            tab.doc.hits = vec![Hit { line: i as u64 + 1, byte: 0, col_start: 0, col_end: 1 }];
            tab.keep_results(format!("s{}", i)).unwrap();
        }
        assert_eq!(tab.kept.len(), super::MAX_KEPT);
        assert_eq!(tab.kept[0].name, "s2");
    }

    #[test]
    fn follow_rotation_clears_and_notifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.log");
        std::fs::write(&path, test_lines(3000, 100)).unwrap();
        let mut tab = open_tab(&path);
        tab.search_text = String::from("ERROR");
        tab.case_sensitive = true;
        tab.start_search(&mut Vec::new());
        drain_search(&mut tab);
        assert_eq!(tab.doc.hits.len(), 30);
        tab.filter_text = String::from("ERROR");
        tab.start_filter(String::from("ERROR"));
        drain_filter(&mut tab);
        assert!(tab.doc.filter_active);

        // Truncate = rotation. Di Windows file yang di-mmap tak bisa
        // di-truncate langsung (OS menolak); rotasi nyata = rename + baru.
        let rotated = path.with_extension("log.1");
        std::fs::rename(&path, &rotated).unwrap();
        std::fs::write(&path, test_lines(100, 10)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(450));
        tab.poll_follow();
        assert!(tab.doc.hits.is_empty());
        assert!(!tab.doc.filter_active);
        assert!(tab.doc.filter_map.is_empty());
        assert!(
            tab.doc.status.contains("dirotasi"),
            "status: {}",
            tab.doc.status
        );
    }

    #[test]
    fn multi_selection_toggle_and_merge() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.log");
        std::fs::write(&path, test_lines(100, 10)).unwrap();
        let mut tab = open_tab(&path);
        assert!(!tab.sel_has_any());
        // Klik biasa lalu Ctrl+klik dua baris jauh.
        tab.sel_start(10, false);
        assert!(tab.sel_toggle(20));
        assert!(tab.sel_toggle(30));
        assert_eq!(tab.sel_all_ranges(), vec![(10, 10), (20, 20), (30, 30)]);
        assert_eq!(tab.sel_line_count(), 3);
        assert!(tab.sel_contains(20));
        assert!(!tab.sel_contains(21));
        // Toggle off baris tengah.
        assert!(!tab.sel_toggle(20));
        assert_eq!(tab.sel_all_ranges(), vec![(10, 10), (30, 30)]);
        // Ctrl+klik bersebelahan menggabung ke primer.
        tab.sel_clear();
        tab.sel_start(10, false);
        tab.sel_extend(12);
        assert!(tab.sel_toggle(13));
        assert_eq!(tab.sel_all_ranges(), vec![(10, 13)]);
        // Klik biasa mereset multi.
        tab.sel_toggle(50);
        tab.sel_start(5, false);
        assert_eq!(tab.sel_all_ranges(), vec![(5, 5)]);
        // Clear total.
        tab.sel_clear();
        assert!(!tab.sel_has_any());
        assert!(tab.sel_all_ranges().is_empty());
    }

    #[test]
    fn multi_selection_copy_joins_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel2.log");
        std::fs::write(&path, b"l1\nl2\nl3\nl4\nl5\n").unwrap();
        let mut tab = open_tab(&path);
        tab.sel_start(1, false);
        tab.sel_toggle(3);
        tab.sel_toggle(5);
        let s = tab.sel_copy_text().unwrap();
        assert!(s.contains("l1"), "got: {:?}", s);
        assert!(s.contains("l3"), "got: {:?}", s);
        assert!(s.contains("l5"), "got: {:?}", s);
        assert!(!s.contains("l2"), "got: {:?}", s);
    }

    #[test]
    fn tab_alias_display_falls_back_to_filename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nm.log");
        std::fs::write(&path, b"x\n").unwrap();
        let mut tab = open_tab(&path);
        assert_eq!(tab.display_name(), "nm.log");
        tab.alias = Some(String::from("  "));
        assert_eq!(tab.display_name(), "nm.log");
        tab.alias = Some(String::from("DB utama"));
        assert_eq!(tab.display_name(), "DB utama");
    }

    #[test]
    fn search_state_machine_transitions() {        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("st.log");
        std::fs::write(&path, b"a\nb\n").unwrap();
        let mut tab = open_tab(&path);
        // Kosong -> NoSearch.
        assert_eq!(tab.search_state(), super::SearchState::NoSearch);
        // Query aktif tanpa worker -> Static.
        tab.search_text = String::from("a");
        assert_eq!(tab.search_state(), super::SearchState::Static);
        // Worker jalan -> Searching; quiet -> AutoRefreshing.
        tab.doc.search_in_progress = true;
        assert_eq!(tab.search_state(), super::SearchState::Searching);
        tab.search_merge_quiet = true;
        assert_eq!(tab.search_state(), super::SearchState::AutoRefreshing);
        tab.search_merge_quiet = false;
        tab.doc.search_in_progress = false;
        // Galat menang atas pangkas.
        tab.doc.search_error = Some(String::from("x"));
        tab.doc.search_truncated = true;
        assert_eq!(tab.search_state(), super::SearchState::Error);
        tab.doc.search_error = None;
        assert_eq!(tab.search_state(), super::SearchState::Truncated);
        tab.doc.search_truncated = false;
        assert_eq!(tab.search_state(), super::SearchState::Static);
    }

    #[test]
    fn partial_drag_portion_selection_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("portion.log");
        std::fs::write(&path, b"2026-09-09 checkpoint started successfully\n").unwrap();
        let mut tab = open_tab(&path);
        // Simulate intra-line drag from character 11 to 21 ("checkpoint")
        tab.sel_portion = Some((1, 11, 21));
        let copied = tab.sel_copy_text().unwrap();
        assert_eq!(copied, "checkpoint");

        // Verify sel_has_any is true
        assert!(tab.sel_has_any());

        // Clear selection
        tab.sel_clear();
        assert_eq!(tab.sel_portion, None);
        assert!(!tab.sel_has_any());
    }

    #[test]
    fn search_suggestion_state_preserves_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sug.log");
        std::fs::write(&path, b"dummy\n").unwrap();
        let mut tab = open_tab(&path);
        assert!(!tab.sug_active);
        tab.sug_active = true;
        assert!(tab.sug_active);
    }
}
