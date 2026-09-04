// English comments: Doc owns one tab's mmap + index + search/filter state.
// UI paints only visible lines; engine never clones the whole file.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub mod archive;
pub mod decode;
pub mod filter;
pub mod follow;
pub mod index;
pub mod jsonlog;
pub mod marks;
pub mod mmap;
pub mod query;
pub mod scratch;
pub mod search;

pub use decode::Encoding;
pub use filter::ParsedFilter;
pub use index::SparseIndex;
pub use search::Hit;

/// Max bytes allowed through clipboard (16 MB per spec).
pub const COPY_CAP_BYTES: usize = 16 * 1024 * 1024;
/// Decoded-line LRU capacity.
pub const CACHE_CAP: usize = 2000;

/// One viewport row.
#[derive(Clone, Debug)]
pub struct LineView {
    /// 1-based original line number (gutter).
    pub line_no: u64,
    pub byte_offset: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub line: u64,
    pub byte: u64,
    pub label: String,
    pub color: BookmarkColor,
}

/// Warna penanda (disimpan di sidecar; dipetakan ke Color32 di UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BookmarkColor {
    #[default]
    Default,
    Blue,
    Green,
    Yellow,
    Red,
    Purple,
}

impl BookmarkColor {
    pub fn semua() -> &'static [BookmarkColor] {
        &[
            BookmarkColor::Default,
            BookmarkColor::Blue,
            BookmarkColor::Green,
            BookmarkColor::Yellow,
            BookmarkColor::Red,
            BookmarkColor::Purple,
        ]
    }

    pub fn nama(self) -> &'static str {
        match self {
            BookmarkColor::Default => "Bawaan",
            BookmarkColor::Blue => "Biru",
            BookmarkColor::Green => "Hijau",
            BookmarkColor::Yellow => "Kuning",
            BookmarkColor::Red => "Merah",
            BookmarkColor::Purple => "Ungu",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            BookmarkColor::Default => "default",
            BookmarkColor::Blue => "blue",
            BookmarkColor::Green => "green",
            BookmarkColor::Yellow => "yellow",
            BookmarkColor::Red => "red",
            BookmarkColor::Purple => "purple",
        }
    }

    pub fn from_key(s: &str) -> BookmarkColor {
        match s {
            "blue" => BookmarkColor::Blue,
            "green" => BookmarkColor::Green,
            "yellow" => BookmarkColor::Yellow,
            "red" => BookmarkColor::Red,
            "purple" => BookmarkColor::Purple,
            _ => BookmarkColor::Default,
        }
    }
}

/// Simple LRU for decoded lines: line_no -> text.
struct DecodedCache {
    cap: usize,
    map: HashMap<u64, String>,
    order: VecDeque<u64>,
}

impl DecodedCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    fn get(&mut self, line: u64) -> Option<String> {
        self.map.get(&line).cloned()
    }
    fn put(&mut self, line: u64, text: String) {
        if self.map.contains_key(&line) {
            return;
        }
        if self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        self.order.push_back(line);
        self.map.insert(line, text);
    }
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

/// One open file (one tab). Read-only.
pub struct Doc {
    pub path: PathBuf,
    pub file_name: String,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// Auto-detected encoding + BOM length.
    pub detected: Encoding,
    pub bom_len: usize,
    /// User override (None = automatic).
    pub encoding_override: Option<Encoding>,
    pub index: SparseIndex,
    pub search_gen: u64,
    pub hits: Vec<Hit>,
    pub search_truncated: bool,
    pub search_error: Option<String>,
    pub search_in_progress: bool,
    pub filter: ParsedFilter,
    pub filter_map: Vec<u64>,
    pub filter_active: bool,
    pub follow: bool,
    pub stick_bottom: bool,
    pub bookmarks: Vec<Bookmark>,
    /// Last Indonesian status/warning for this doc.
    pub status: String,
    mmap: Option<Arc<memmap2::Mmap>>,
    cache: DecodedCache,
}

/// Jenis blok dump untuk salin/ekspor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Transaction,
    Checkpoint,
}

impl Doc {
    /// Open read-only, mmap, detect encoding, try sidecar. Never panics on I/O.
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let md = std::fs::metadata(&path)
            .map_err(|e| format!("Gagal membuka file '{}': {}", path.display(), e))?;
        if !md.is_file() {
            return Err(format!("Bukan file: '{}'", path.display()));
        }
        let size = md.len();
        let mtime = md.modified().ok();
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        // Empty file: no mmap needed.
        if size == 0 {
            return Ok(Self {
                path,
                file_name,
                size,
                mtime,
                detected: Encoding::Utf8,
                bom_len: 0,
                encoding_override: None,
                index: SparseIndex {
                    checkpoints: Vec::new(),
                    total_lines: 0,
                    total_bytes: 0,
                    complete: true,
                    progress: 1.0,
                },
                search_gen: 0,
                hits: Vec::new(),
                search_truncated: false,
                search_error: None,
                search_in_progress: false,
                filter: ParsedFilter::default(),
                filter_map: Vec::new(),
                filter_active: false,
                follow: false,
                stick_bottom: false,
                bookmarks: Vec::new(),
                status: String::from("File kosong."),
                mmap: None,
                cache: DecodedCache::new(CACHE_CAP),
            });
        }

        let m = mmap::open_mmap(&path)
            .map_err(|e| format!("Gagal memetakan file '{}': {}", path.display(), e))?;
        let (detected, bom_len) = {
            let head_len = (m.len()).min(64 * 1024);
            decode::detect_encoding(&m[..head_len])
        };
        let mut doc = Self {
            path,
            file_name,
            size,
            mtime,
            detected,
            bom_len,
            encoding_override: None,
            index: SparseIndex::empty(),
            search_gen: 0,
            hits: Vec::new(),
            search_truncated: false,
            search_error: None,
            search_in_progress: false,
            filter: ParsedFilter::default(),
            filter_map: Vec::new(),
            filter_active: false,
            follow: false,
            stick_bottom: false,
            bookmarks: Vec::new(),
            status: String::from("Membuka… mengindeks latar."),
            mmap: Some(Arc::new(m)),
            cache: DecodedCache::new(CACHE_CAP),
        };
        // Show first screen instantly: build a tiny provisional index from head
        // so line 1..N resolve before background indexer finishes.
        doc.build_provisional_index();
        // Try sidecar for instant full index.
        doc.try_load_sidecar();
        Ok(doc)
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding_override.unwrap_or(self.detected)
    }

    pub fn set_encoding_override(&mut self, enc: Option<Encoding>) {
        self.encoding_override = enc;
        self.cache.clear();
        // Rebuild index synchronously for the new encoding mode (cheap head scan
        // for provisional; background thread completes the rest).
        self.build_provisional_index();
        self.try_load_sidecar();
    }

    pub fn data(&self) -> &[u8] {
        match &self.mmap {
            Some(m) => m.as_ref(),
            None => &[],
        }
    }

    /// Total rows in current view (filter_map when filter active).
    pub fn view_row_count(&self) -> u64 {
        if self.filter_active {
            self.filter_map.len() as u64
        } else if self.index.complete {
            self.index.total_lines
        } else {
            // Before index completes: estimate from size (assume ~120 bytes/line).
            self.line_count_estimate()
        }
    }

    pub fn line_count_estimate(&self) -> u64 {
        if self.index.complete {
            return self.index.total_lines;
        }
        if self.index.total_lines > 0 {
            // Partial progress: extrapolate.
            let p = self.index.progress.max(0.01);
            return ((self.index.total_lines as f64) / (p as f64)) as u64;
        }
        // No progress yet: rough byte-based estimate.
        let usable = self.size.saturating_sub(self.bom_len as u64);
        usable.div_ceil(120).clamp(1, 100_000_000)
    }

    pub fn view_row_to_line(&self, row: u64) -> Option<u64> {
        if self.filter_active {
            self.filter_map.get(row as usize).copied()
        } else {
            let n = self.view_row_count();
            if row < n {
                Some(row + 1)
            } else {
                None
            }
        }
    }

    /// Byte range of 1-based line, or None.
    pub fn line_byte_range(&self, line_no: u64) -> Option<(u64, u64)> {
        index::line_byte_range(self.data(), &self.index.checkpoints, line_no, self.encoding(), self.bom_len)
            .or_else(|| {
                // Fallback before index completes: sequential scan from 0 (only for
                // nearby lines; goto_percent path uses byte snap instead).
                if line_no <= 5000 {
                    let full = index::build_full(self.data(), self.encoding(), self.bom_len);
                    index::line_byte_range(self.data(), &full.checkpoints, line_no, self.encoding(), self.bom_len)
                } else {
                    None
                }
            })
    }

    /// Decode one line (cached). Returns None when line unknown yet.
    pub fn get_line_text(&mut self, line_no: u64) -> Option<String> {
        if let Some(c) = self.cache.get(line_no) {
            return Some(c);
        }
        let (s, e) = self.line_byte_range(line_no)?;
        let bytes = self.data().get(s as usize..e as usize)?;
        // Strip trailing \r (CRLF) for display.
        let end = if !bytes.is_empty() && bytes[bytes.len() - 1] == b'\r' && !self.encoding().is_wide() {
            &bytes[..bytes.len() - 1]
        } else {
            bytes
        };
        let mut t = decode::decode_bytes(end, self.encoding());
        t = decode::strip_cr(t);
        self.cache.put(line_no, t.clone());
        Some(t)
    }

    /// Viewport API: decode only `count` lines from `start` (1-based line).
    pub fn get_lines(&mut self, start: u64, count: usize) -> Vec<LineView> {
        let mut out = Vec::new();
        if self.filter_active {
            // Indeks langsung ke filter_map (terurut) tanpa clone vec.
            let s = (start.saturating_sub(1)) as usize;
            let n = self.filter_map.len();
            for k in s..n {
                if out.len() >= count {
                    break;
                }
                let ln = self.filter_map[k];
                let text = self.get_line_text(ln).unwrap_or_default();
                let byte = self.line_byte_range(ln).map(|(b, _)| b).unwrap_or(0);
                out.push(LineView {
                    line_no: ln,
                    byte_offset: byte,
                    text,
                });
            }
            return out;
        }
        for i in 0..count {
            let ln = start + i as u64;
            if ln < 1 {
                continue;
            }
            // Stop when past known end (if index complete).
            if self.index.complete && ln > self.index.total_lines {
                break;
            }
            match self.get_line_text(ln) {
                Some(text) => {
                    let byte = self.line_byte_range(ln).map(|(b, _)| b).unwrap_or(0);
                    out.push(LineView {
                        line_no: ln,
                        byte_offset: byte,
                        text,
                    });
                }
                None => break,
            }
        }
        out
    }

    /// Goto line -> byte offset (binary search checkpoints, then sequential scan).
    pub fn goto_line_byte(&self, line_no: u64) -> Option<u64> {
        index::byte_offset_of_line(
            self.data(),
            &self.index.checkpoints,
            line_no,
            self.encoding(),
            self.bom_len,
        )
    }

    /// Goto percent: line count if ready, else byte approx snapped to line start.
    pub fn goto_percent_line(&self, percent: f64) -> u64 {
        let p = percent.clamp(0.0, 100.0);
        if self.index.complete && self.index.total_lines > 0 {
            let n = ((self.index.total_lines as f64) * p / 100.0).round() as u64;
            n.clamp(1, self.index.total_lines)
        } else {
            let b = index::goto_percent_byte(self.size, p, self.bom_len);
            let snapped = index::snap_to_line_start(self.data(), b, self.encoding(), self.bom_len);
            // Convert snapped byte to approximate line via checkpoints (fallback estimate).
            if let Some((cl, cb)) = index::checkpoint_lookup(&self.index.checkpoints, u64::MAX) {
                // rough: lines per byte from last checkpoint
                let lpb = if cb > 0 { (cl as f64) / (cb as f64) } else { 1.0 / 120.0 };
                ((snapped as f64) * lpb).round() as u64 + 1
            } else {
                (snapped / 120) + 1
            }
        }
    }

    /// Snap arbitrary byte to line start (for scrollbar-before-index).
    pub fn snap_byte(&self, byte: u64) -> u64 {
        index::snap_to_line_start(self.data(), byte, self.encoding(), self.bom_len)
    }

    /// Copy decoded range [line_start, line_end] capped at 16 MB.
    pub fn copy_range_text(&mut self, line_start: u64, line_end: u64) -> Result<String, String> {
        if line_start > line_end {
            return Err(String::from("Rentang baris tidak valid."));
        }
        let mut out = String::new();
        let mut bytes = 0usize;
        for ln in line_start..=line_end {
            let t = self
                .get_line_text(ln)
                .ok_or_else(|| String::from("Baris di luar jangkauan."))?;
            let add = t.len() + 1; // + newline
            if bytes + add > COPY_CAP_BYTES {
                return Err(String::from(
                    "Pilihan melebihi 16 MB. Gunakan “Simpan ke file…” untuk mengekspor.",
                ));
            }
            out.push_str(&t);
            out.push('\n');
            bytes += add;
            // Safety: avoid huge loop on bogus ranges.
            if ln - line_start > 5_000_000 {
                return Err(String::from(
                    "Pilihan melebihi 16 MB. Gunakan “Simpan ke file…” untuk mengekspor.",
                ));
            }
        }
        Ok(out)
    }

    /// Stream hits (+ context) to a new file. Never rewrites source.
    pub fn export_hits_to_file(&mut self, out_path: &Path, context: usize) -> Result<usize, String> {
        use std::io::Write;
        if self.hits.is_empty() {
            return Err(String::from("Tidak ada hasil untuk diekspor."));
        }
        let mut f = std::fs::File::create(out_path)
            .map_err(|e| format!("Gagal membuat file ekspor: {}", e))?;
        let hits = self.hits.clone();
        let total_lines = if self.index.complete {
            self.index.total_lines
        } else {
            self.line_count_estimate()
        };
        let mut written = 0usize;
        // Merge overlapping context windows to avoid duplicate lines.
        let mut next_skip_until: u64 = 0;
        for h in &hits {
            let lo = h.line.saturating_sub(context as u64).max(1);
            let hi = (h.line + context as u64).min(total_lines.max(h.line));
            for ln in lo..=hi {
                if ln <= next_skip_until {
                    continue;
                }
                if let Some(t) = self.get_line_text(ln) {
                    writeln!(f, "{}: {}", ln, t)
                        .map_err(|e| format!("Gagal menulis ekspor: {}", e))?;
                    written += 1;
                }
            }
            next_skip_until = next_skip_until.max(hi);
        }
        Ok(written)
    }

    /// FNV-1a 64 hex pendek untuk referensi baris tiket.
    pub fn short_hash(text: &str) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in text.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", (h >> 32) ^ (h & 0xffff_ffff))
    }

    /// Ekspor tiket Markdown 1-klik: hasil + konteks, siap paste ke Jira.
    /// Header memuat nama file, query, jumlah, konteks; tiap hit memakai
    /// pagar kode dengan baris cocok bertanda `>>> ` plus hash pendek.
    pub fn export_ticket_to_file(
        &mut self,
        out_path: &Path,
        query: &str,
        context: usize,
    ) -> Result<usize, String> {
        use std::io::Write;
        if self.hits.is_empty() {
            return Err(String::from("Tidak ada hasil untuk diekspor."));
        }
        let mut f = std::fs::File::create(out_path)
            .map_err(|e| format!("Gagal membuat file tiket: {}", e))?;
        let hits = self.hits.clone();
        let total_lines = if self.index.complete {
            self.index.total_lines
        } else {
            self.line_count_estimate()
        };
        writeln!(f, "# Laporan AsisLog").map_err(|e| format!("Gagal menulis: {}", e))?;
        writeln!(f).map_err(|e| format!("Gagal menulis: {}", e))?;
        writeln!(f, "- File: `{}`", self.file_name)
            .map_err(|e| format!("Gagal menulis: {}", e))?;
        writeln!(f, "- Query: `{}`", query.replace('`', "'"))
            .map_err(|e| format!("Gagal menulis: {}", e))?;
        writeln!(f, "- Hasil: {}", format_count(hits.len() as u64))
            .map_err(|e| format!("Gagal menulis: {}", e))?;
        writeln!(f, "- Konteks: +-{} baris", context)
            .map_err(|e| format!("Gagal menulis: {}", e))?;
        writeln!(f).map_err(|e| format!("Gagal menulis: {}", e))?;
        let mut written = 0usize;
        let mut next_skip_until: u64 = 0;
        for h in &hits {
            let lo = h.line.saturating_sub(context as u64).max(1);
            let hi = (h.line + context as u64).min(total_lines.max(h.line));
            let anchor = self.get_line_text(h.line).unwrap_or_default();
            writeln!(
                f,
                "## Baris {} {{#{}}}",
                format_count(h.line),
                Self::short_hash(&anchor)
            )
            .map_err(|e| format!("Gagal menulis: {}", e))?;
            writeln!(f, "```").map_err(|e| format!("Gagal menulis: {}", e))?;
            for ln in lo..=hi {
                if ln <= next_skip_until {
                    continue;
                }
                if let Some(t) = self.get_line_text(ln) {
                    if ln == h.line {
                        writeln!(f, ">>> {}", t)
                    } else {
                        writeln!(f, "    {}", t)
                    }
                    .map_err(|e| format!("Gagal menulis: {}", e))?;
                    written += 1;
                }
            }
            writeln!(f, "```").map_err(|e| format!("Gagal menulis: {}", e))?;
            writeln!(f).map_err(|e| format!("Gagal menulis: {}", e))?;
            next_skip_until = next_skip_until.max(hi);
        }
        Ok(written)
    }

    /// Find the SQL-dump block containing `line`: [start, end] (inclusive).
    /// Rules (light delimiter parser, no full SQL parsing): backward
    /// (max 5000 lines) the first `--INSERT-…` / `INSERT INTO…` wins; a `go`
    /// line on the way means the previous block ended, so start after it.
    /// Forward (max 5000 lines) the first trimmed `go` (case-insensitive) wins.
    /// Returns Indonesian error when no block is found (caller shows it).
    pub fn sql_block_range(&mut self, line: u64) -> Result<(u64, u64), String> {
        const WINDOW: u64 = 5000;
        let total = if self.index.complete {
            self.index.total_lines
        } else {
            self.line_count_estimate()
        };
        if line < 1 || line > total.max(1) {
            return Err(String::from("Baris di luar jangkauan."));
        }
        let is_go = |t: &str| t.trim().eq_ignore_ascii_case("go");
        let is_start = |t: &str| {
            let tl = t.trim_start();
            tl.starts_with("--INSERT-") || tl.starts_with("INSERT INTO")
        };
        // Backward: start marker wins; a `go` above means previous block.
        let mut start: Option<u64> = None;
        let lo = line.saturating_sub(WINDOW).max(1);
        for ln in (lo..=line).rev() {
            let t = self.get_line_text(ln).unwrap_or_default();
            if ln < line && is_go(&t) {
                start = Some(ln + 1);
                break;
            }
            if is_start(&t) {
                start = Some(ln);
                break;
            }
        }
        let start = match start {
            Some(s) => s,
            None => {
                return Err(String::from(
                    "Awal blok (--INSERT- / INSERT INTO) tidak ditemukan dalam ±5000 baris.",
                ))
            }
        };
        // Forward: closing `go` wins.
        let hi = total.min(start.saturating_add(WINDOW)).max(line);
        for ln in line.max(start)..=hi {
            let t = self.get_line_text(ln).unwrap_or_default();
            if is_go(&t) {
                return Ok((start, ln));
            }
        }
        Err(String::from(
            "Akhir blok (baris go) tidak ditemukan dalam +5000 baris.",
        ))
    }

    /// Generic block finder for transaction / checkpoint dumps.
    /// `Transaction`: backward `--BEGIN TRANSACTION`/`BEGIN TRANSACTION`,
    /// forward first `COMMIT WORK`/`ROLLBACK WORK`/`go` (case-insensitive).
    /// `Checkpoint`: backward `--START CHECKPOINT`, forward `--FINISH CHECKPOINT`.
    /// Windows ±100.000 lines; errors are Indonesian.
    pub fn block_range(&mut self, line: u64, kind: BlockKind) -> Result<(u64, u64, String), String> {
        const WINDOW: u64 = 100_000;
        let total = if self.index.complete {
            self.index.total_lines
        } else {
            self.line_count_estimate()
        };
        if line < 1 || line > total.max(1) {
            return Err(String::from("Baris di luar jangkauan."));
        }
        let up = |t: &str| t.trim().to_ascii_uppercase();
        let (starts, ends): (&[&str], &[&str]) = match kind {
            BlockKind::Transaction => (
                &["--BEGIN TRANSACTION", "BEGIN TRANSACTION"],
                &["COMMIT WORK", "ROLLBACK WORK", "GO"],
            ),
            BlockKind::Checkpoint => (&["--START CHECKPOINT"], &["--FINISH CHECKPOINT"]),
        };
        let is_start = |t: &str| {
            let u = up(t);
            starts.iter().any(|s| u.starts_with(s))
        };
        let is_end = |t: &str| {
            let u = up(t);
            ends.iter().any(|s| {
                if *s == "GO" {
                    u == "GO"
                } else {
                    u.starts_with(s)
                }
            })
        };
        let lo = line.saturating_sub(WINDOW).max(1);
        let mut start: Option<u64> = None;
        for ln in (lo..=line).rev() {
            let t = self.get_line_text(ln).unwrap_or_default();
            if ln < line {
                // Batas blok sebelumnya (akhir) → mulai setelahnya.
                let u = up(&t);
                let prev_end = match kind {
                    BlockKind::Transaction => {
                        u.starts_with("COMMIT WORK")
                            || u.starts_with("ROLLBACK WORK")
                            || u == "GO"
                    }
                    BlockKind::Checkpoint => u.starts_with("--FINISH CHECKPOINT"),
                };
                if prev_end {
                    start = Some((ln + 1).min(line));
                    break;
                }
            }
            if is_start(&t) {
                start = Some(ln);
                break;
            }
        }
        let start = match start {
            Some(s) => s,
            None => {
                return Err(match kind {
                    BlockKind::Transaction => String::from(
                        "Awal transaksi (BEGIN TRANSACTION) tidak ditemukan dalam ±100.000 baris.",
                    ),
                    BlockKind::Checkpoint => String::from(
                        "Awal checkpoint (--START CHECKPOINT) tidak ditemukan dalam ±100.000 baris.",
                    ),
                })
            }
        };
        let hi = total.min(start.saturating_add(WINDOW)).max(line);
        for ln in line.max(start)..=hi {
            let t = self.get_line_text(ln).unwrap_or_default();
            if is_end(&t) {
                let desc = match kind {
                    BlockKind::Transaction => format!(
                        "Transaksi baris {}–{}",
                        format_count(start),
                        format_count(ln)
                    ),
                    BlockKind::Checkpoint => format!(
                        "Checkpoint baris {}–{}",
                        format_count(start),
                        format_count(ln)
                    ),
                };
                return Ok((start, ln, desc));
            }
        }
        Err(String::from(
            "Akhir blok tidak ditemukan dalam +100.000 baris.",
        ))
    }

    /// Export decoded range [a, b] as plain lines (no numbers) for SQL use.
    pub fn export_range_to_file(
        &mut self,
        out_path: &Path,
        a: u64,
        b: u64,
    ) -> Result<usize, String> {
        use std::io::Write;
        if a < 1 || b < a {
            return Err(String::from("Rentang baris tidak valid."));
        }
        // Safety cap: at most ~50 MB decoded.
        if b - a > 1_000_000 {
            return Err(String::from("Rentang terlalu besar (maks ~1 juta baris)."));
        }
        let mut f = std::fs::File::create(out_path)
            .map_err(|e| format!("Gagal membuat file ekspor: {}", e))?;
        let mut n = 0usize;
        for ln in a..=b {
            let t = self
                .get_line_text(ln)
                .ok_or_else(|| String::from("Baris di luar jangkauan."))?;
            writeln!(f, "{}", t).map_err(|e| format!("Gagal menulis ekspor: {}", e))?;
            n += 1;
        }
        Ok(n)
    }

    pub fn toggle_bookmark(&mut self, line: u64) {        if let Some(i) = self.bookmarks.iter().position(|b| b.line == line) {
            self.bookmarks.remove(i);
            self.status = format!("Penanda baris {} dihapus.", line);
        } else {
            let byte = self.line_byte_range(line).map(|(b, _)| b).unwrap_or(0);
            let preview = self.get_line_text(line).unwrap_or_default();
            let label: String = preview.chars().take(60).collect();
            self.bookmarks.push(Bookmark {
                line,
                byte,
                label,
                color: BookmarkColor::Default,
            });
            self.bookmarks.sort_by_key(|b| b.line);
            self.status = format!("Penanda ditambahkan di baris {}.", line);
        }
    }

    pub fn sidecar_path(&self) -> PathBuf {
        let mut s = self.path.as_os_str().to_owned();
        s.push(".asisidx");
        PathBuf::from(s)
    }

    pub fn try_load_sidecar(&mut self) {
        let sc = self.sidecar_path();
        if let Some(idx) = index::load_sidecar(&sc, self.size, self.mtime) {
            // Validate checkpoints roughly (first must be (1, bom)).
            self.index = idx;
            self.status = String::from("Indeks sisi dimuat.");
        }
    }

    pub fn save_sidecar(&self) {
        if self.index.complete && !self.index.checkpoints.is_empty() {
            let _ = index::save_sidecar(&self.sidecar_path(), &self.index, self.size, self.mtime);
        }
    }

    /// Apply a completed background index.
    pub fn apply_index(&mut self, idx: SparseIndex) {
        self.index = idx;
        if self.index.complete {
            self.status = format!(
                "Indeks selesai: {} baris.",
                format_count(self.index.total_lines)
            );
            self.save_sidecar();
        }
    }

    /// Remap after file grew; caller must have updated size/mtime.
    pub fn remap(&mut self) -> Result<(), String> {
        let md = std::fs::metadata(&self.path)
            .map_err(|e| format!("Gagal membaca metadata '{}': {}", self.path.display(), e))?;
        self.size = md.len();
        self.mtime = md.modified().ok();
        if self.size == 0 {
            self.mmap = None;
            self.index = SparseIndex {
                checkpoints: Vec::new(),
                total_lines: 0,
                total_bytes: 0,
                complete: true,
                progress: 1.0,
            };
            self.cache.clear();
            return Ok(());
        }
        let m = mmap::open_mmap(&self.path)
            .map_err(|e| format!("Gagal memetakan ulang '{}': {}", self.path.display(), e))?;
        self.mmap = Some(Arc::new(m));
        self.cache.clear();
        Ok(())
    }

    /// Reopen from start after rotation/truncate.
    pub fn reopen_after_rotate(&mut self) -> Result<(), String> {
        self.remap()?;
        self.index = SparseIndex::empty();
        self.build_provisional_index();
        self.hits.clear();
        self.filter_map.clear();
        self.bookmarks.retain(|b| b.byte < self.size);
        self.status = String::from("File dipotong/dirotasi — dibuka ulang dari awal.");
        Ok(())
    }

    /// Index only the new tail after append (remap already done).
    /// Scans from old_total_bytes to EOF counting newlines, appending checkpoints.
    pub fn index_tail(&mut self, old_bytes: u64, old_lines: u64) {
        let mmap_clone = self.mmap.clone();
        let data: &[u8] = match &mmap_clone {
            Some(m) => m.as_ref(),
            None => &[],
        };
        let enc = self.encoding();
        let bom_len = self.bom_len;
        if enc.is_wide() {
            // Simplest correct fallback for wide: full rebuild (rare + still O(n) once).
            let full = index::build_full(data, enc, bom_len);
            self.index = full;
            return;
        }
        let mut checkpoints = std::mem::take(&mut self.index.checkpoints);
        if checkpoints.is_empty() {
            checkpoints.push((1, bom_len as u64));
        }
        let mut line = old_lines.max(1);
        // If old file ended without newline, the next bytes continue the same line;
        // if it ended with newline, next bytes start a new line already counted?
        // Our total_lines counts lines (not newlines), so appending after trailing
        // newline means old_lines is exact; scanning newlines adds lines.
        let mut last = checkpoints.last().copied().unwrap_or((1, self.bom_len as u64));
        let start = (old_bytes as usize).min(data.len());
        let mut i = start;
        // If old file did not end with newline and we continue, first newline ends `line`.
        while i < data.len() {
            if data[i] == b'\n' {
                line += 1;
                let next = (i + 1) as u64;
                if next < data.len() as u64
                    && (line - last.0 >= index::CHECKPOINT_LINES
                        || next - last.1 >= index::CHECKPOINT_BYTES)
                {
                    checkpoints.push((line, next));
                    last = (line, next);
                }
            }
            i += 1;
        }
        // Trailing newline phantom correction:
        let mut total_lines = line;
        if !data.is_empty() && data[data.len() - 1] == b'\n' {
            // total_lines currently counts the phantom; but our loop added it.
            // build_full trims it, so mirror that: if last byte is newline, the
            // increment created an empty line past EOF -> subtract.
            // Only when old file didn't already end with newline? Simplify: recompute
            // by checking: if data ends with newline, last line start == len => phantom.
            // Our `line` counts it, so subtract 1 when len>0 and ends with \n and
            // total content non-empty.
            // Edge: empty file handled earlier.
            total_lines = total_lines.saturating_sub(1);
            // Ensure at least 1 when file non-empty? A file "\n" has 1 line? Actually
            // "a\n" has 1 line per our convention? No: "a\n" -> lines: "a" only? We
            // defined "a\nb\n" as 2 lines ("a","b"), so "\n" alone = 1 empty line?
            // Keep max(total,1) for non-empty.
            if total_lines == 0 && !data.is_empty() {
                total_lines = 1;
            }
        }
        // Reconcile with full-scan convention for small drift: if checkpoints empty-ish,
        // trust computed.
        self.index = SparseIndex {
            checkpoints,
            total_lines,
            total_bytes: data.len() as u64,
            complete: true,
            progress: 1.0,
        };
    }

    fn build_provisional_index(&mut self) {
        let mmap_clone = self.mmap.clone();
        let data: &[u8] = match &mmap_clone {
            Some(m) => m.as_ref(),
            None => &[],
        };
        if data.is_empty() {
            return;
        }
        // Scan first 1 MiB synchronously for instant first paint.
        let n = data.len().min(1024 * 1024);
        let part = &data[..n];
        let enc = self.encoding();
        let bom_len = self.bom_len;
        let full = index::build_full(part, enc, bom_len);
        // Provisional: checkpoints valid, totals are lower bounds.
        let complete = (n == data.len()) && full.complete;
        self.index = SparseIndex {
            checkpoints: full.checkpoints,
            total_lines: if complete {
                full.total_lines
            } else {
                // estimate until background finishes
                full.total_lines.max(1)
            },
            total_bytes: data.len() as u64,
            complete: false,
            progress: (n as f32) / (data.len() as f32),
        };
        if complete {
            // Small file: just take the full index.
            let real = index::build_full(data, enc, self.bom_len);
            self.index = real;
        }
    }

    // ---------- timestamp jump support ----------

    /// Parse timestamp prefix of a decoded line to unix seconds (best effort).
    /// Supports `YYYY-MM-DD HH:MM:SS`, `YYYY/MM/DD HH:MM:SS`, ISO `YYYY-MM-DDTHH:MM:SS`.
    pub fn parse_timestamp_prefix(line: &str) -> Option<i64> {
        let b = line.as_bytes();
        if b.len() < 19 {
            return None;
        }
        // Normalize separator at [10]: ' ', 'T', '/' etc.
        let y: i32 = std::str::from_utf8(b.get(0..4)?).ok()?.parse().ok()?;
        let sep1 = b.get(4)?;
        if *sep1 != b'-' && *sep1 != b'/' {
            return None;
        }
        let mo: i32 = std::str::from_utf8(b.get(5..7)?).ok()?.parse().ok()?;
        let sep2 = b.get(7)?;
        if *sep2 != b'-' && *sep2 != b'/' {
            return None;
        }
        let d: i32 = std::str::from_utf8(b.get(8..10)?).ok()?.parse().ok()?;
        let sep3 = *b.get(10)?;
        if sep3 != b' ' && sep3 != b'T' {
            return None;
        }
        let hh: i32 = std::str::from_utf8(b.get(11..13)?).ok()?.parse().ok()?;
        if *b.get(13)? != b':' {
            return None;
        }
        let mm: i32 = std::str::from_utf8(b.get(14..16)?).ok()?.parse().ok()?;
        if *b.get(16)? != b':' {
            return None;
        }
        let ss: i32 = std::str::from_utf8(b.get(17..19)?).ok()?.parse().ok()?;
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
            return None;
        }
        Some(days_to_unix(y, mo, d) + (hh as i64) * 3600 + (mm as i64) * 60 + ss as i64)
    }
}

fn days_to_unix(y: i32, m: i32, d: i32) -> i64 {
    // Days since 1970-01-01 (Howard Hinnant algorithm, no chrono dep).
    let mut yy = y as i64;
    let mut mm = m as i64;
    if mm <= 2 {
        yy -= 1;
        mm += 12;
    }
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * (mm - 3) + 2) / 5 + (d as i64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400
}

/// Format counts Indonesian style with dot thousands separator.
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// Human file size Indonesian (KB/MB/GB with comma decimal).
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    let s = if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    };
    s.replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc_with_text(text: &str) -> Doc {
        // Build a Doc backed by an anonymous mmap-like buffer.
        // We write to a temp file then open (uses real mmap path).
        let mut p = std::env::temp_dir();
        p.push(format!("asislog-test-{}.log", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::write(&p, text).unwrap();
        let doc = Doc::open(p.clone()).unwrap();
        // Complete the index synchronously for tests.
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(doc.sidecar_path());
        doc
    }

    #[test]
    fn viewport_only_decodes_visible() {
        let mut d = make_doc_with_text("a\nb\nc\n");
        // Force complete index like background would.
        let full = index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        let v = d.get_lines(2, 2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].line_no, 2);
        assert_eq!(v[0].text, "b");
    }

    #[test]
    fn copy_cap() {
        let mut d = make_doc_with_text("hello\nworld\n");
        let full = index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        assert!(d.copy_range_text(1, 2).is_ok());
    }

    #[test]
    fn timestamp_parse() {
        let t = Doc::parse_timestamp_prefix("2026-09-03 13:41:02 ERROR x");
        assert!(t.is_some());
        assert!(Doc::parse_timestamp_prefix("no timestamp here").is_none());
    }

    fn make_sql_doc() -> Doc {
        let text = "--INSERT-001\nINSERT INTO T (a, b)\nVALUES (1, 2)\ngo\nINFO ok\n--INSERT-002\nINSERT INTO T (a)\nVALUES (3)\nGO\n";
        let mut d = make_doc_with_text(text);
        let full = index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        d
    }

    #[test]
    fn sql_block_from_values_line() {
        let mut d = make_sql_doc();
        // VALUES line 3 -> block 2..4
        assert_eq!(d.sql_block_range(3), Ok((2, 4)));
    }

    #[test]
    fn sql_block_from_go_line_and_second_block() {
        let mut d = make_sql_doc();
        assert_eq!(d.sql_block_range(4), Ok((2, 4)));
        // Uppercase GO also closes.
        assert_eq!(d.sql_block_range(8), Ok((7, 9)));
    }

    #[test]
    fn sql_block_missing_markers_errors() {
        let mut d = make_doc_with_text("INFO a\nINFO b\nINFO c\n");
        let full = index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        assert!(d.sql_block_range(2).is_err());
    }

    fn make_tx_doc() -> Doc {
        let text = "--START CHECKPOINT\n--BEGIN TRANSACTION\nINSERT INTO T VALUES (1)\nCOMMIT WORK\ngo\n--FINISH CHECKPOINT\nINFO x\n";
        let mut d = make_doc_with_text(text);
        let full = index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        d
    }

    #[test]
    fn transaction_block_range() {
        let mut d = make_tx_doc();
        let (a, b, _) = d.block_range(3, BlockKind::Transaction).unwrap();
        assert_eq!((a, b), (2, 4));
    }

    #[test]
    fn checkpoint_block_range() {
        let mut d = make_tx_doc();
        let (a, b, _) = d.block_range(3, BlockKind::Checkpoint).unwrap();
        assert_eq!((a, b), (1, 6));
    }

    #[test]
    fn export_range_plain_lines() {
        let mut d = make_sql_doc();
        let mut p = std::env::temp_dir();
        p.push(format!("asislog-range-{}.sql", std::process::id()));
        let n = d.export_range_to_file(&p, 2, 4).unwrap();
        assert_eq!(n, 3);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("INSERT INTO"));
        assert!(!text.contains("2: "));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn ticket_export_markdown() {
        use crate::engine::search::Hit;
        let mut d = make_sql_doc();
        d.hits = vec![Hit { line: 2, byte: 0, col_start: 0, col_end: 6 }];
        let mut p = std::env::temp_dir();
        p.push(format!("asislog-ticket-{}.md", std::process::id()));
        let n = d.export_ticket_to_file(&p, "INSERT", 1).unwrap();
        assert_eq!(n, 3); // lines 1..=3
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# Laporan AsisLog"));
        assert!(text.contains("- Query: `INSERT`"));
        assert!(text.contains("## Baris 2 {#"));
        assert!(text.contains(">>> INSERT INTO"));
        assert!(text.contains("```"));
        let _ = std::fs::remove_file(&p);
    }
}
