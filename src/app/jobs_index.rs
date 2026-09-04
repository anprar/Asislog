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



pub(crate) fn spawn_filter(
    path: PathBuf,
    encoding: Encoding,
    _bom_len: usize,
    f: ParsedFilter,
    tx: mpsc::Sender<(Vec<u64>, bool)>,
    // Tail refresh: mulai dari (byte, nomor baris 1-based) alih-alih 0.
    // Penelepon menjamin byte adalah awal baris; semua nomor yang
    // dihasilkan >= nomor awal sehingga penggabungan tetap terurut.
    tail_from: Option<(u64, u64)>,
) {
    std::thread::spawn(move || {
        use std::io::{BufRead, Seek, SeekFrom};
        let file = match std::fs::File::open(&path) {
            Ok(x) => x,
            Err(_) => {
                let _ = tx.send((Vec::new(), false));
                return;
            }
        };
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        let mut line_no: u64 = 1;
        if let Some((sb, sl)) = tail_from {
            // Gagal seek = pindai penuh (benar, lebih lambat).
            if reader.seek(SeekFrom::Start(sb)).is_ok() {
                line_no = sl.max(1);
            }
        }
        let mut map: Vec<u64> = Vec::new();
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
/// Kasar per ±4 MB pada file 2 GB — cukup sebagai "peta masalah" strip.
pub(crate) fn spawn_marker_scan(
    path: PathBuf,
    total: u64,
    tx: mpsc::Sender<MarkerUpdate>,
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
        // Pass kedua: histogram ERROR per menit (garis ber-cap waktu saja).
        // Satu baca sekuensial tambahan; page cache masih hangat.
        let hist = time_hist_pass(&path);
        let _ = tx.send((bits, total, hist));
    });
}

/// Pass garis: kumpulkan (epoch-menit, baris) garis ERROR (maks 200 rb),
/// lalu kuantisasi menjadi histogram. None bila < 2 menit berbeda.
pub(crate) fn time_hist_pass(path: &PathBuf) -> Option<TimeHist> {
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
}
