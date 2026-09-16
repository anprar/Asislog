// English comments: Background workers: indexer, filter, marker/map scan (split from app.rs; behavior unchanged).
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
use crate::engine::{format_count, format_size, BlockKind, BookmarkColor, Doc, WideLineReader};
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

// ---------- background workers (file-based, bounded RAM) ----------

pub(crate) fn spawn_indexer(
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
                // SIMD newline scan over the buffer (same checkpoint rule
                // as before; the old per-byte loop topped out ~580 MB/s).
                let base = byte; // absolute offset of buf[start]
                for rel in memchr::memchr_iter(b'\n', &buf[start..n]) {
                    byte = base + rel as u64 + 1;
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
                byte = base + (n - start) as u64;
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



pub(crate) fn spawn_filter(
    path: PathBuf,
    encoding: Encoding,
    _bom_len: usize,
    f: ParsedFilter,
    tx: mpsc::Sender<crate::engine::LineSet>,
    // Tail refresh: mulai dari (byte, nomor baris 1-based) alih-alih 0.
    // Penelepon menjamin byte adalah awal baris; semua nomor yang
    // dihasilkan >= nomor awal sehingga penggabungan tetap terurut.
    tail_from: Option<(u64, u64)>,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        use std::io::{BufRead, Seek, SeekFrom};
        let file = match std::fs::File::open(&path) {
            Ok(x) => x,
            Err(_) => {
                let _ = tx.send(crate::engine::LineSet::new());
                return;
            }
        };
        let wide = encoding.is_wide();
        let le = encoding == Encoding::Utf16Le;
        let mut narrow_reader: Option<std::io::BufReader<std::fs::File>> = None;
        let mut wide_reader: Option<WideLineReader<std::fs::File>> = if wide {
            Some(WideLineReader::new(
                std::io::BufReader::with_capacity(1024 * 1024, file),
                le,
            ))
        } else {
            narrow_reader = Some(std::io::BufReader::with_capacity(1024 * 1024, file));
            None
        };
        let mut line_no: u64 = 1;
        if let Some((sb, sl)) = tail_from {
            // Gagal seek = pindai penuh (benar, lebih lambat).
            use std::io::Seek;
            let ok = if let Some(wr) = wide_reader.as_mut() {
                wr.seek_start(sb).is_ok()
            } else if let Some(nr) = narrow_reader.as_mut() {
                nr.seek(SeekFrom::Start(sb)).is_ok()
            } else {
                false
            };
            if ok {
                line_no = sl.max(1);
            }
        }
        // No match cap: results stream straight into a roaring bitmap
        // (200M consecutive lines â‰ˆ 43 KB), so the old silent 5M-line
        // truncation is gone â€” reported counts are always exact.
        let mut map = crate::engine::LineSet::new();
        let mut buf: Vec<u8> = Vec::new();
        // Pre-cancelled (superseded before first line): send nothing.
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        loop {
            // Cooperative cancel so a superseded filter stops promptly
            // instead of burning a full scan nobody will read.
            if line_no & 4095 == 0 && cancel.load(Ordering::Relaxed) {
                return;
            }
            buf.clear();
            let n: u64 = if let Some(wr) = wide_reader.as_mut() {
                wr.read_line(&mut buf)
            } else if let Some(nr) = narrow_reader.as_mut() {
                match nr.read_until(b'\n', &mut buf) {
                    Ok(0) => 0,
                    Ok(n) => n as u64,
                    Err(_) => 0,
                }
            } else {
                0
            };
            if n == 0 && buf.is_empty() {
                break;
            }
            // strip newline
            let mut line_bytes = &buf[..];
            if wide {
                let (n1, n2) = if le { (0x0Au8, 0x00u8) } else { (0x00u8, 0x0Au8) };
                if line_bytes.len() >= 2
                    && line_bytes[line_bytes.len() - 2] == n1
                    && line_bytes[line_bytes.len() - 1] == n2
                {
                    line_bytes = &line_bytes[..line_bytes.len() - 2];
                    let (c1, c2) = if le { (0x0Du8, 0x00u8) } else { (0x00u8, 0x0Du8) };
                    if line_bytes.len() >= 2
                        && line_bytes[line_bytes.len() - 2] == c1
                        && line_bytes[line_bytes.len() - 1] == c2
                    {
                        line_bytes = &line_bytes[..line_bytes.len() - 2];
                    }
                }
            } else if line_bytes.ends_with(b"\n") {
                line_bytes = &line_bytes[..line_bytes.len() - 1];
            }
            let s = crate::engine::decode::decode_bytes(line_bytes, encoding);
            let ok = crate::engine::filter::line_matches(&s, &f);
            if ok {
                map.insert(line_no);
            }
            line_no += 1;
        }
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(map);
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
    let step = span.div_ceil(nb).max(1) as i64;
    // `step` >= 1, so the cast back is safe; unsigned div_ceil avoids
    // the (here unstable) signed-int rounding API with identical result.
    let n = span.div_ceil(step as usize);
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
/// Kasar per Â±4 MB pada file 2 GB â€” cukup sebagai "peta masalah" strip.
/// Menghormati flag cancel (tab ditutup/rotasi) â€” worker besar terakhir
/// kini bisa dihentikan seperti indexer/filter/search.
pub(crate) fn spawn_marker_scan(
    path: PathBuf,
    total: u64,
    tx: mpsc::Sender<MarkerUpdate>,
    cancel: Arc<AtomicBool>,
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
        if cancel.load(Ordering::Relaxed) {
            return;
        }
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
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            let has_err = err_fs.iter().any(|f| f.find(chunk).is_some());
            let has_warn = warn_f.find(chunk).is_some();
            if has_err || has_warn {
                let mask = u8::from(has_err) | (u8::from(has_warn) << 1);
                let b0 = ((offset * MARKER_BUCKETS as u64) / total.max(1)) as usize;
                let b1 =
                    (((offset + n as u64) * MARKER_BUCKETS as u64) / total.max(1)) as usize;
                for slot in &mut bits[b0..=b1.min(MARKER_BUCKETS - 1)] {
                    *slot |= mask;
                }
            }
            offset += n as u64;
        }
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // Pass kedua: histogram ERROR per menit (garis ber-cap waktu saja).
        // Satu baca sekuensial tambahan; page cache masih hangat.
        let hist = time_hist_pass(&path, &cancel);
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send((bits, total, hist));
    });
}

/// Pass garis: kumpulkan (epoch-menit, baris) garis ERROR (maks 200 rb),
/// lalu kuantisasi menjadi histogram. None bila < 2 menit berbeda.
pub(crate) fn time_hist_pass(path: &PathBuf, cancel: &Arc<AtomicBool>) -> Option<TimeHist> {
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
        if line_no & 4095 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
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

/// Satu pekerjaan ekspor: snapshot input agar worker latar tak meminjam Doc.
/// Cermin logika gabung-konteks `Doc::export_hits_to_file` /
/// `export_ticket_to_file` (sengaja diduplikasi agar versi sinkron tetap
/// sederhana; bila mengubah satu, ubah keduanya).
pub(crate) struct ExportJob {
    pub src: PathBuf,
    pub out: PathBuf,
    pub hits: Vec<Hit>,
    pub context: usize,
    pub query: String,
    pub file_name: String,
    pub checkpoints: Vec<(u64, u64)>,
    pub total_lines: u64,
    pub encoding: Encoding,
    pub bom_len: usize,
    pub ticket: bool,
}

/// Kemajuan ekspor untuk status bar.
pub(crate) struct ExportMsg {
    pub written: u64,
    pub total_hits: usize,
    pub out: PathBuf,
    pub ticket: bool,
    pub context: usize,
    pub error: Option<String>,
    pub done: bool,
}

fn export_decode(
    data: &[u8],
    cps: &[(u64, u64)],
    line: u64,
    encoding: Encoding,
    bom: usize,
) -> Option<String> {
    let (s, e) = index::line_byte_range(data, cps, line, encoding, bom)?;
    let mut b = &data[s as usize..e as usize];
    if !b.is_empty() && b[b.len() - 1] == b'\r' && !encoding.is_wide() {
        b = &b[..b.len() - 1];
    }
    let t = crate::engine::decode::decode_bytes(b, encoding);
    Some(crate::engine::decode::strip_cr(t))
}

pub(crate) fn spawn_export(
    job: ExportJob,
    tx: mpsc::Sender<ExportMsg>,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        use std::io::Write;
        let progress = |written: u64, done: bool, error: Option<String>| {
            let _ = tx.send(ExportMsg {
                written,
                total_hits: job.hits.len(),
                out: job.out.clone(),
                ticket: job.ticket,
                context: job.context,
                error,
                done,
            });
        };
        let data = match crate::engine::mmap::open_mmap(&job.src) {
            Ok(m) => m,
            Err(e) => {
                progress(0, true, Some(format!("Gagal membuka sumber ekspor: {}", e)));
                return;
            }
        };
        let bytes: &[u8] = &data;
        let mut f = match std::fs::File::create(&job.out) {
            Ok(f) => f,
            Err(e) => {
                progress(0, true, Some(format!("Gagal membuat file ekspor: {}", e)));
                return;
            }
        };
        if job.ticket {
            let _ = writeln!(f, "# Laporan AsisLog");
            let _ = writeln!(f);
            let _ = writeln!(f, "- File: `{}`", job.file_name);
            let _ = writeln!(f, "- Query: `{}`", job.query.replace('`', "'"));
            let _ = writeln!(f, "- Hasil: {}", crate::engine::format_count(job.hits.len() as u64));
            let _ = writeln!(f, "- Konteks: +-{} baris", job.context);
            let _ = writeln!(f);
        }
        let mut written: u64 = 0;
        let mut next_skip_until: u64 = 0;
        let mut since_progress: usize = 0;
        for h in &job.hits {
            if cancel.load(Ordering::Relaxed) {
                progress(written, true, Some(String::from("Ekspor dibatalkan.")));
                return;
            }
            let lo = h.line.saturating_sub(job.context as u64).max(1);
            let hi = (h.line + job.context as u64).min(job.total_lines.max(h.line));
            if job.ticket {
                let anchor =
                    export_decode(bytes, &job.checkpoints, h.line, job.encoding, job.bom_len)
                        .unwrap_or_default();
                if writeln!(
                    f,
                    "## Baris {} {{#{}}}",
                    crate::engine::format_count(h.line),
                    crate::engine::Doc::short_hash(&anchor)
                )
                .is_err()
                {
                    progress(written, true, Some(String::from("Gagal menulis ekspor.")));
                    return;
                }
                let _ = writeln!(f, "```");
            }
            for ln in lo..=hi {
                if ln <= next_skip_until {
                    continue;
                }
                if let Some(t) = export_decode(bytes, &job.checkpoints, ln, job.encoding, job.bom_len)
                {
                    let ok = if job.ticket {
                        if ln == h.line {
                            writeln!(f, ">>> {}", t)
                        } else {
                            writeln!(f, "    {}", t)
                        }
                    } else {
                        writeln!(f, "{}: {}", ln, t)
                    };
                    if ok.is_err() {
                        progress(written, true, Some(String::from("Gagal menulis ekspor.")));
                        return;
                    }
                    written += 1;
                }
            }
            if job.ticket {
                let _ = writeln!(f, "```");
                let _ = writeln!(f);
            }
            next_skip_until = next_skip_until.max(hi);
            since_progress += 1;
            if since_progress >= 256 {
                since_progress = 0;
                progress(written, false, None);
            }
        }
        progress(written, true, None);
    });
}

/// Ekspor-streaming tanpa batas tampil: pindai ulang file dari query kini
/// dan tulis baris cocok LANGSUNG ke disk (O(chunk) RAM, nol Hit tersimpan).
/// Jawaban atas "jutaan match": viewport tetap dibatasi (DV), tapi tidak
/// ada satu pun baris cocok yang hilang di file ekspor.
pub(crate) struct ExportSearchJob {
    pub src: PathBuf,
    pub out: PathBuf,
    pub query: String,
    pub regex_on: bool,
    pub case_sensitive: bool,
    pub encoding: Encoding,
    pub bom_len: usize,
}

/// Matcher per-baris untuk ekspor streaming. Semantik = jalur search:
/// literal sensitif = Finder byte; insensitif = fold Unicode penuh;
/// regex = mesin cepat (+ fallback fancy); boolean = AST di teks terdecode.
enum ExportMatcher {
    Literal { needle: Vec<u8> },
    LiteralCi { needle_lower: String },
    RegexFast(regex::bytes::Regex),
    RegexFancy(fancy_regex::Regex),
    Bool(crate::engine::query::Query),
}

fn compile_export_matcher(
    query: &str,
    regex_on: bool,
    case_sensitive: bool,
) -> Result<ExportMatcher, String> {
    if !regex_on && !crate::engine::query::is_boolean_query(query) {
        if case_sensitive {
            Ok(ExportMatcher::Literal { needle: query.as_bytes().to_vec() })
        } else {
            Ok(ExportMatcher::LiteralCi { needle_lower: query.to_lowercase() })
        }
    } else if regex_on {
        match crate::engine::search::compile_regex_auto(query, case_sensitive) {
            Ok(crate::engine::search::CompiledRegex::Fast(_)) => {
                let mut b = regex::bytes::RegexBuilder::new(query);
                b.case_insensitive(!case_sensitive);
                b.build()
                    .map(ExportMatcher::RegexFast)
                    .map_err(|e| format!("Regex tidak valid: {}", e))
            }
            Ok(crate::engine::search::CompiledRegex::Fancy(_)) => {
                let mut fb = fancy_regex::RegexBuilder::new(query);
                fb.case_insensitive(!case_sensitive);
                // Explicit default: pathological lines error, not hang.
                fb.backtrack_limit(1_000_000);
                fb.build()
                    .map(ExportMatcher::RegexFancy)
                    .map_err(|e| format!("Regex tidak valid: {}", e))
            }
            Err(e) => Err(format!("Regex tidak valid: {}", e)),
        }
    } else {
        crate::engine::query::parse_query(query)
            .map(ExportMatcher::Bool)
            .map_err(|e| format!("Query tidak valid: {}", e))
    }
}

/// Terapkan matcher ke satu baris. `text` = hasil decode; `scan` = byte yang
/// dipindai matcher byte (raw untuk narrow, decoded-utf8 untuk wide —
/// paritas dengan jalur search).
fn export_line_matches(
    m: &ExportMatcher,
    text: &str,
    scan: &[u8],
    case_sensitive: bool,
) -> bool {
    match m {
        ExportMatcher::Literal { needle } => memchr::memmem::find(scan, needle).is_some(),
        ExportMatcher::LiteralCi { needle_lower } => {
            text.to_lowercase().contains(needle_lower.as_str())
        }
        ExportMatcher::RegexFast(re) => re.is_match(scan),
        ExportMatcher::RegexFancy(re) => re.is_match(text).unwrap_or(false),
        ExportMatcher::Bool(ast) => ast.matches(text, case_sensitive),
    }
}

/// Inti sinkron (dipakai worker + tes): tulis `ln: teks` semua baris cocok.
/// Mengembalikan jumlah baris ditulis. `progress(written)` dipanggil berkala.
pub(crate) fn export_search_to_file(
    job: &ExportSearchJob,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<u64, String> {
    use std::io::{Read, Write};
    if job.query.trim().is_empty() {
        return Err(String::from("Query kosong — isi kolom Cari dulu."));
    }
    let matcher = compile_export_matcher(&job.query, job.regex_on, job.case_sensitive)?;
    // P0: same required-literal prefilter as the fancy search worker.
    // Narrow only (wide filters on decoded text below). Export stays EXACT:
    // no line cap here (search caps, export must not lose rows).
    let fancy_pre = match &matcher {
        ExportMatcher::RegexFancy(_) => {
            crate::engine::fancypre::FancyPrefilter::new(&job.query, job.case_sensitive)
        }
        _ => crate::engine::fancypre::FancyPrefilter::disabled(),
    };
    let is_fancy = matches!(&matcher, ExportMatcher::RegexFancy(_));
    let mut f = std::fs::File::create(&job.out)
        .map_err(|e| format!("Gagal membuat file ekspor: {}", e))?;
    let wide = job.encoding.is_wide();
    let le = job.encoding == Encoding::Utf16Le;
    let mut written: u64 = 0;
    let mut check_cancel_lines = 0u32;

    // Satu baris cocok -> tulis + hitung. Returns Err on I/O failure.
    let emit = |ln: u64,
                    text: &str,
                    f: &mut std::fs::File,
                    written: &mut u64,
                    progress: &mut dyn FnMut(u64)|
     -> Result<(), String> {
        writeln!(f, "{}: {}", ln, text).map_err(|_| String::from("Gagal menulis ekspor."))?;
        *written += 1;
        if (*written).is_multiple_of(1024) {
            progress(*written);
        }
        Ok(())
    };

    if wide {
        let file = std::fs::File::open(&job.src)
            .map_err(|e| format!("Gagal membuka sumber ekspor: {}", e))?;
        let mut reader =
            WideLineReader::new(std::io::BufReader::with_capacity(1024 * 1024, file), le);
        let mut line_no: u64 = 1;
        let mut buf: Vec<u8> = Vec::new();
        let mut first = true;
        let (n1, n2) = if le { (0x0Au8, 0x00u8) } else { (0x00u8, 0x0Au8) };
        let (c1, c2) = if le { (0x0Du8, 0x00u8) } else { (0x00u8, 0x0Du8) };
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(String::from("Ekspor dibatalkan."));
            }
            buf.clear();
            let n = reader.read_line(&mut buf);
            if n == 0 && buf.is_empty() {
                break;
            }
            let mut lb = &buf[..];
            if lb.len() >= 2 && lb[lb.len() - 2] == n1 && lb[lb.len() - 1] == n2 {
                lb = &lb[..lb.len() - 2];
                if lb.len() >= 2 && lb[lb.len() - 2] == c1 && lb[lb.len() - 1] == c2 {
                    lb = &lb[..lb.len() - 2];
                }
            }
            if first {
                first = false;
                if job.bom_len > 0 && lb.len() >= job.bom_len {
                    lb = &lb[job.bom_len..];
                }
            }
            // Baris padding NUL (odd-pad) dilewati seperti jalur search.
            if !lb.is_empty() && lb.iter().any(|&b| b != 0) {
                let text = crate::engine::decode::decode_bytes(lb, job.encoding);
                let pre_ok = !is_fancy || fancy_pre.passes(text.as_bytes());
                if pre_ok && export_line_matches(&matcher, &text, text.as_bytes(), job.case_sensitive) {
                    let disp = crate::engine::decode::strip_cr(text);
                    emit(line_no, &disp, &mut f, &mut written, &mut progress)?;
                }
            }
            line_no += 1;
            check_cancel_lines += 1;
            if check_cancel_lines >= 4096 {
                check_cancel_lines = 0;
                if cancel.load(Ordering::Relaxed) {
                    return Err(String::from("Ekspor dibatalkan."));
                }
                progress(written);
            }
        }
    } else {
        let file = std::fs::File::open(&job.src)
            .map_err(|e| format!("Gagal membuka sumber ekspor: {}", e))?;
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        let chunk_size = search::effective_chunk_bytes();
        let mut carry: Vec<u8> = Vec::new();
        let mut line_no: u64 = 1;
        let mut first_chunk = true;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(String::from("Ekspor dibatalkan."));
            }
            let mut tmp = vec![0u8; chunk_size];
            let n = match reader.read(&mut tmp) {
                Ok(n) => n,
                Err(e) => return Err(format!("Gagal membaca sumber ekspor: {}", e)),
            };
            let eof = n < chunk_size;
            tmp.truncate(n);
            if tmp.is_empty() && carry.is_empty() {
                break;
            }
            // Satukan carry + fresh, belah per baris penuh.
            let mut combined = std::mem::take(&mut carry);
            combined.extend_from_slice(&tmp);
            let mut start = 0usize;
            let mut first_line = first_chunk;
            first_chunk = false;
            for (i, &b) in combined.iter().enumerate() {
                if b != b'\n' {
                    continue;
                }
                let mut lb = &combined[start..i];
                if !lb.is_empty() && lb[lb.len() - 1] == b'\r' {
                    lb = &lb[..lb.len() - 1];
                }
                let mut lbm = lb;
                if first_line {
                    first_line = false;
                    if job.bom_len > 0 && lbm.len() >= job.bom_len {
                        lbm = &lbm[job.bom_len..];
                    }
                }
                // Matcher byte = raw (paritas jalur search narrow).
                // Fancy: required-literal prefilter dulu (tanpa decode).
                let hit = match &matcher {
                    ExportMatcher::Literal { needle } => {
                        memchr::memmem::find(lbm, needle).is_some()
                    }
                    ExportMatcher::RegexFancy(_) if !fancy_pre.passes(lbm) => false,
                    _ => {
                        let text = crate::engine::decode::decode_bytes(lbm, job.encoding);
                        export_line_matches(&matcher, &text, lbm, job.case_sensitive)
                    }
                };
                if hit {
                    let text = crate::engine::decode::decode_bytes(lbm, job.encoding);
                    let disp = crate::engine::decode::strip_cr(text);
                    emit(line_no, &disp, &mut f, &mut written, &mut progress)?;
                }
                line_no += 1;
                start = i + 1;
            }
            carry = combined[start..].to_vec();
            if eof {
                // Sisa ekor tanpa newline = baris terakhir.
                if !carry.is_empty() {
                    let mut lbm = &carry[..];
                    if !lbm.is_empty() && lbm[lbm.len() - 1] == b'\r' {
                        lbm = &lbm[..lbm.len() - 1];
                    }
                    let hit = match &matcher {
                        ExportMatcher::Literal { needle } => {
                            memchr::memmem::find(lbm, needle).is_some()
                        }
                        ExportMatcher::RegexFancy(_) if !fancy_pre.passes(lbm) => false,
                        _ => {
                            let text = crate::engine::decode::decode_bytes(lbm, job.encoding);
                            export_line_matches(&matcher, &text, lbm, job.case_sensitive)
                        }
                    };
                    if hit {
                        let text = crate::engine::decode::decode_bytes(lbm, job.encoding);
                        let disp = crate::engine::decode::strip_cr(text);
                        emit(line_no, &disp, &mut f, &mut written, &mut progress)?;
                    }
                }
                break;
            }
        }
    }
    progress(written);
    Ok(written)
}

pub(crate) fn spawn_export_search(
    job: ExportSearchJob,
    tx: mpsc::Sender<ExportMsg>,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let progress = |written: u64| {
            let _ = tx.send(ExportMsg {
                written,
                total_hits: 0, // streaming: total tak diketahui di muka
                out: job.out.clone(),
                ticket: false,
                context: 0,
                error: None,
                done: false,
            });
        };
        match export_search_to_file(&job, &cancel, progress) {
            Ok(w) => {
                let _ = tx.send(ExportMsg {
                    written: w,
                    total_hits: 0,
                    out: job.out.clone(),
                    ticket: false,
                    context: 0,
                    error: None,
                    done: true,
                });
            }
            Err(e) => {
                let _ = tx.send(ExportMsg {
                    written: 0,
                    total_hits: 0,
                    out: job.out.clone(),
                    ticket: false,
                    context: 0,
                    error: Some(e),
                    done: true,
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::search::Hit;

    fn sample_doc(dir: &std::path::Path) -> (PathBuf, Vec<Hit>, Vec<(u64, u64)>, u64) {
        let src = dir.join("e.log");
        let mut data = Vec::new();
        for i in 1..=200u64 {
            let line = if i % 10 == 0 {
                format!("2026-09-04 ERROR id={}\n", i)
            } else {
                format!("2026-09-04 INFO id={} pad\n", i)
            };
            data.extend_from_slice(line.as_bytes());
        }
        std::fs::write(&src, &data).unwrap();
        let idx = index::build_full(&data, Encoding::Utf8, 0);
        let hits: Vec<Hit> = (1..=200u64)
            .filter(|i| i % 10 == 0)
            .map(|i| {
                let off = index::byte_offset_of_line(&data, &idx.checkpoints, i, Encoding::Utf8, 0)
                    .unwrap();
                let col = memchr::memmem::Finder::new(b"ERROR")
                    .find(&data[off as usize..])
                    .unwrap();
                Hit { line: i, byte: off, col_start: col as u32, col_end: (col + 5) as u32 }
            })
            .collect();
        (src, hits, idx.checkpoints.clone(), idx.total_lines)
    }

    fn run_export(job: ExportJob) -> (Vec<ExportMsg>, PathBuf) {
        let out = job.out.clone();
        let (tx, rx) = mpsc::channel();
        spawn_export(job, tx, Arc::new(AtomicBool::new(false)));
        let mut msgs = Vec::new();
        for m in rx {
            let done = m.done;
            msgs.push(m);
            if done {
                break;
            }
        }
        (msgs, out)
    }

    fn run_filter(path: PathBuf, query: &str) -> crate::engine::LineSet {
        let (tx, rx) = mpsc::channel();
        spawn_filter(
            path,
            Encoding::Utf8,
            0,
            parse_filter(query, true),
            tx,
            None,
            Arc::new(AtomicBool::new(false)),
        );
        rx.recv_timeout(Duration::from_secs(60)).expect("filter finished")
    }

    #[test]
    fn filter_reports_exact_count_with_roaring_heap() {
        // 100k consecutive matches: the old Vec+5M-cap design is gone â€”
        // exact count, and a heap a fraction of the old 800 KB Vec.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dense.log");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).unwrap();
            for i in 0..100_000u64 {
                writeln!(f, "2026-09-04 ERROR id={}", i).unwrap();
            }
        }
        let map = run_filter(path, "ERROR");
        assert_eq!(map.len(), 100_000);
        assert_eq!(map.iter().next(), Some(1));
        assert_eq!(map.iter().last(), Some(100_000));
        assert!(
            map.heap_bytes() < 100_000,
            "roaring heap must stay tiny, got {}",
            map.heap_bytes()
        );
    }

    #[test]
    fn filter_precancelled_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.log");
        std::fs::write(&path, "2026-09-04 ERROR x\n").unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_filter(
            path,
            Encoding::Utf8,
            0,
            parse_filter("ERROR", true),
            tx,
            None,
            Arc::new(AtomicBool::new(true)),
        );
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_err(),
            "cancelled worker must stay silent"
        );
    }

    #[test]
    fn export_worker_matches_sync_plain() {
        let dir = tempfile::tempdir().unwrap();
        let (src, hits, cps, total) = sample_doc(dir.path());
        let out = dir.path().join("out.txt");
        let job = ExportJob {
            src: src.clone(),
            out: out.clone(),
            hits: hits.clone(),
            context: 2,
            query: String::new(),
            file_name: String::from("e.log"),
            checkpoints: cps,
            total_lines: total,
            encoding: Encoding::Utf8,
            bom_len: 0,
            ticket: false,
        };
        let (msgs, _) = run_export(job);
        assert!(msgs.iter().any(|m| m.done && m.error.is_none()));
        // Oracle sinkron: Doc::export_hits_to_file harus byte-identik.
        let mut doc = Doc::open(src).unwrap();
        doc.hits = hits;
        // Lengkapi indeks agar export sinkron memakai jalur sama.
        let data = std::fs::read(doc.path.clone()).unwrap();
        doc.index = index::build_full(&data, Encoding::Utf8, 0);
        let out2 = dir.path().join("out-sync.txt");
        let n = doc.export_hits_to_file(&out2, 2).unwrap();
        assert!(n > 0);
        assert_eq!(
            std::fs::read(&out).unwrap(),
            std::fs::read(&out2).unwrap()
        );
    }

    #[test]
    fn export_worker_ticket_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let (src, hits, cps, total) = sample_doc(dir.path());
        // Tiket: samakan dengan sinkron.
        let out = dir.path().join("t.md");
        let job = ExportJob {
            src: src.clone(),
            out: out.clone(),
            hits: hits.clone(),
            context: 1,
            query: String::from("ERROR"),
            file_name: String::from("e.log"),
            checkpoints: cps.clone(),
            total_lines: total,
            encoding: Encoding::Utf8,
            bom_len: 0,
            ticket: true,
        };
        let (msgs, _) = run_export(job);
        assert!(msgs.iter().any(|m| m.done && m.error.is_none()));
        let mut doc = Doc::open(src.clone()).unwrap();
        doc.hits = hits.clone();
        let data = std::fs::read(doc.path.clone()).unwrap();
        doc.index = index::build_full(&data, Encoding::Utf8, 0);
        let out2 = dir.path().join("t-sync.md");
        doc.export_ticket_to_file(&out2, "ERROR", 1).unwrap();
        assert_eq!(
            std::fs::read(&out).unwrap(),
            std::fs::read(&out2).unwrap()
        );
        // Cancel: worker berhenti dengan pesan galat-batal.
        let out3 = dir.path().join("c.txt");
        let cancel = Arc::new(AtomicBool::new(true)); // batal sejak awal
        let job3 = ExportJob {
            src,
            out: out3,
            hits,
            context: 0,
            query: String::new(),
            file_name: String::from("e.log"),
            checkpoints: cps,
            total_lines: total,
            encoding: Encoding::Utf8,
            bom_len: 0,
            ticket: false,
        };
        let (tx, rx) = mpsc::channel();
        spawn_export(job3, tx, cancel);
        let mut saw_cancel = false;
        for m in rx {
            if m.error.as_deref() == Some("Ekspor dibatalkan.") {
                saw_cancel = true;
            }
            if m.done {
                break;
            }
        }
        assert!(saw_cancel);
    }

    fn export_search_job(
        dir: &std::path::Path,
        name: &str,
        content: &[u8],
        query: &str,
        regex_on: bool,
        case_sensitive: bool,
    ) -> (ExportSearchJob, PathBuf) {
        let src = dir.join(name);
        std::fs::write(&src, content).unwrap();
        let out = dir.join("stream-out.txt");
        (
            ExportSearchJob {
                src,
                out: out.clone(),
                query: query.to_string(),
                regex_on,
                case_sensitive,
                encoding: Encoding::Utf8,
                bom_len: 0,
            },
            out,
        )
    }

    fn run_export_search(job: &ExportSearchJob) -> Result<u64, String> {
        let cancel = AtomicBool::new(false);
        export_search_to_file(job, &cancel, |_| {})
    }

    #[test]
    fn export_search_literal_case_modes() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"2026 ERROR timeout\n2026 info ok\n2026 error retry\n2026 WARN x\n";
        // Sensitif: 1 baris.
        let (job, out) = export_search_job(dir.path(), "s.log", data, "ERROR", false, true);
        assert_eq!(run_export_search(&job).unwrap(), 1);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("1: 2026 ERROR timeout"));
        // Insensitif Unicode: ÉCLAIR vs éclair (e polos tak ikut).
        let data2 = "w1 ÉCLAIR x\nw2 éclair y\nw3 plain\n".as_bytes();
        let (job, out) =
            export_search_job(dir.path(), "u.log", data2, "éclair", false, false);
        assert_eq!(run_export_search(&job).unwrap(), 2);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("1: w1"));
        assert!(text.contains("2: w2"));
    }

    #[test]
    fn export_search_regex_and_boolean() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"err 1 timeout\nerr 2 slow\nok 3 timeout\ninfo 4\n";
        let (job, out) =
            export_search_job(dir.path(), "r.log", data, "^err.*timeout", true, true);
        assert_eq!(run_export_search(&job).unwrap(), 1);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("1: err 1 timeout"));
        // Boolean + XOR baru.
        let (job, _out) =
            export_search_job(dir.path(), "b.log", data, "err timeout -slow", false, true);
        assert_eq!(run_export_search(&job).unwrap(), 1);
        let (job, out) =
            export_search_job(dir.path(), "x.log", data, "err XOR ok", false, true);
        assert_eq!(run_export_search(&job).unwrap(), 3);
        let _ = out;
        // Regex jelek + query kosong ditolak jujur.
        let (job, _) = export_search_job(dir.path(), "e1.log", data, "([a", true, true);
        assert!(run_export_search(&job).is_err());
        let (job, _) = export_search_job(dir.path(), "e2.log", data, "   ", false, true);
        assert!(run_export_search(&job).is_err());
    }

    #[test]
    fn export_search_wide_utf16_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        // UTF-16LE: BOM + 4 baris.
        let mut raw = vec![0xFFu8, 0xFE];
        for l in ["alpha one", "beta two", "alpha three", "gamma"] {
            for u in format!("{}\n", l).encode_utf16() {
                raw.extend_from_slice(&u.to_le_bytes());
            }
        }
        let src = dir.path().join("w.log");
        std::fs::write(&src, &raw).unwrap();
        let out = dir.path().join("w-out.txt");
        let job = ExportSearchJob {
            src,
            out: out.clone(),
            query: String::from("alpha"),
            regex_on: false,
            case_sensitive: true,
            encoding: Encoding::Utf16Le,
            bom_len: 2,
        };
        assert_eq!(run_export_search(&job).unwrap(), 2);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("1: alpha one"));
        assert!(text.contains("3: alpha three"));
        // Cancel sejak awal -> Err batal.
        let cancel = AtomicBool::new(true);
        let err = export_search_to_file(&job, &cancel, |_| {}).unwrap_err();
        assert_eq!(err, "Ekspor dibatalkan.");
    }

    #[test]
    fn export_search_ignores_display_cap() {
        // 20 rb baris cocok: jauh di atas cap tampil default, semua keluar.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("big.log");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&src).unwrap();
            for i in 0..20_000u64 {
                writeln!(f, "2026-09-04 ERROR id={}", i).unwrap();
            }
        }
        let out = dir.path().join("big-out.txt");
        let job = ExportSearchJob {
            src,
            out: out.clone(),
            query: String::from("ERROR"),
            regex_on: false,
            case_sensitive: true,
            encoding: Encoding::Utf8,
            bom_len: 0,
        };
        assert_eq!(run_export_search(&job).unwrap(), 20_000);
        assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), 20_000);
    }
}
