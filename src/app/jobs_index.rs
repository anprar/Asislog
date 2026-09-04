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



pub(crate) fn spawn_filter(    path: PathBuf,
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
