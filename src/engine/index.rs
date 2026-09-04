// English comments: sparse line index (checkpoints every 1024 lines or 64 KiB).

use std::path::Path;
use std::time::SystemTime;

use super::decode::Encoding;

pub const CHECKPOINT_LINES: u64 = 1024;
pub const CHECKPOINT_BYTES: u64 = 64 * 1024;
const SIDECAR_MAGIC: &str = "ASISIDX1";

/// Sparse index: checkpoints map line number (1-based) -> byte offset.
#[derive(Clone, Debug, Default)]
pub struct SparseIndex {
    /// Sorted by line number. First entry is always (1, bom_len) when non-empty.
    pub checkpoints: Vec<(u64, u64)>,
    pub total_lines: u64,
    pub total_bytes: u64,
    pub complete: bool,
    pub progress: f32,
}

impl SparseIndex {
    pub fn empty() -> Self {
        Self {
            checkpoints: Vec::new(),
            total_lines: 0,
            total_bytes: 0,
            complete: false,
            progress: 0.0,
        }
    }
}

/// Build a complete index synchronously (used by tests and small files).
/// `data` is the full mapped bytes, `bom_len` is skipped (line 1 starts there).
pub fn build_full(data: &[u8], encoding: Encoding, bom_len: usize) -> SparseIndex {
    let total_bytes = data.len() as u64;
    if data.is_empty() || (data.len() as u64) <= bom_len as u64 {
        return SparseIndex {
            checkpoints: Vec::new(),
            total_lines: 0,
            total_bytes,
            complete: true,
            progress: 1.0,
        };
    }
    let mut checkpoints = vec![(1u64, bom_len as u64)];
    let mut line: u64 = 1;
    let mut last_cp_line: u64 = 1;
    let mut last_cp_byte: u64 = bom_len as u64;

    if encoding.is_wide() {
        let le = encoding == Encoding::Utf16Le;
        let mut i = bom_len;
        // Align to even offset.
        if i % 2 == 1 {
            i += 1;
        }
        while i + 1 < data.len() {
            let is_nl = if le {
                data[i] == 0x0A && data[i + 1] == 0x00
            } else {
                data[i] == 0x00 && data[i + 1] == 0x0A
            };
            if is_nl {
                line += 1;
                let next = (i + 2) as u64;
                if line - last_cp_line >= CHECKPOINT_LINES || next - last_cp_byte >= CHECKPOINT_BYTES
                {
                    checkpoints.push((line, next));
                    last_cp_line = line;
                    last_cp_byte = next;
                }
            }
            i += 2;
        }
        // Count trailing line without newline: if last unit is not newline, line already counts.
        // If file ends with newline, the increment above created an empty trailing line
        // that has no bytes; drop it when its offset == total_bytes.
        if line > 1 {
            if let Some(&(_, b)) = checkpoints.last() {
                let _ = b;
            }
            let last_start = byte_offset_of_line_inner(data, &checkpoints, line, encoding, bom_len);
            if let Some(s) = last_start {
                if s >= total_bytes && ends_with_newline_wide(data, le) {
                    line -= 1;
                }
            }
        }
    } else {
        let mut pos = bom_len;
        while pos < data.len() {
            if data[pos] == b'\n' {
                line += 1;
                let next = (pos + 1) as u64;
                // Don't create a checkpoint for an empty trailing line past EOF;
                // it will be trimmed below. Still track for progress.
                if (pos + 1) < data.len()
                    && (line - last_cp_line >= CHECKPOINT_LINES
                        || next - last_cp_byte >= CHECKPOINT_BYTES)
                {
                    checkpoints.push((line, next));
                    last_cp_line = line;
                    last_cp_byte = next;
                }
            }
            pos += 1;
        }
        // Trim phantom trailing line when file ends with '\n'.
        if data.last() == Some(&b'\n') {
            line = line.saturating_sub(1);
        }
        if line == 0 {
            line = 0;
        }
    }

    // Empty file edge already handled; file with only BOM has 0 lines.
    let total_lines = if total_bytes as usize <= bom_len { 0 } else { line };

    SparseIndex {
        checkpoints,
        total_lines,
        total_bytes,
        complete: true,
        progress: 1.0,
    }
}

fn ends_with_newline_wide(data: &[u8], le: bool) -> bool {
    if data.len() < 2 {
        return false;
    }
    let n = data.len();
    if le {
        data[n - 2] == 0x0A && data[n - 1] == 0x00
    } else {
        data[n - 2] == 0x00 && data[n - 1] == 0x0A
    }
}

/// Binary search checkpoints for the greatest entry with line <= target.
pub fn checkpoint_lookup(checkpoints: &[(u64, u64)], target_line: u64) -> Option<(u64, u64)> {
    if checkpoints.is_empty() || target_line < 1 {
        return None;
    }
    let mut lo = 0usize;
    let mut hi = checkpoints.len();
    let mut best: Option<(u64, u64)> = None;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (l, b) = checkpoints[mid];
        if l <= target_line {
            best = Some((l, b));
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    best
}

fn byte_offset_of_line_inner(
    data: &[u8],
    checkpoints: &[(u64, u64)],
    target: u64,
    encoding: Encoding,
    bom_len: usize,
) -> Option<u64> {
    if target < 1 {
        return None;
    }
    let (mut line, mut byte) = checkpoint_lookup(checkpoints, target).unwrap_or((1, bom_len as u64));
    if line == target {
        return Some(byte);
    }
    if encoding.is_wide() {
        let le = encoding == Encoding::Utf16Le;
        let mut i = byte as usize;
        if i % 2 == 1 {
            i += 1;
        }
        while i + 1 < data.len() {
            let is_nl = if le {
                data[i] == 0x0A && data[i + 1] == 0x00
            } else {
                data[i] == 0x00 && data[i + 1] == 0x0A
            };
            if is_nl {
                line += 1;
                byte = (i + 2) as u64;
                if line == target {
                    return Some(byte);
                }
            }
            i += 2;
        }
        None
    } else {
        let mut i = byte as usize;
        while i < data.len() {
            if data[i] == b'\n' {
                line += 1;
                byte = (i + 1) as u64;
                if line == target {
                    // Past EOF means no such line.
                    if (byte as usize) >= data.len() {
                        return None;
                    }
                    return Some(byte);
                }
            }
            i += 1;
        }
        None
    }
}

/// Public: byte offset of 1-based line number, or None if out of range.
pub fn byte_offset_of_line(
    data: &[u8],
    checkpoints: &[(u64, u64)],
    target: u64,
    encoding: Encoding,
    bom_len: usize,
) -> Option<u64> {
    if data.is_empty() {
        return None;
    }
    if target < 1 {
        return None;
    }
    // Fast path for line 1.
    if target == 1 {
        if (bom_len as u64) < data.len() as u64 {
            return Some(bom_len as u64);
        }
        return None;
    }
    byte_offset_of_line_inner(data, checkpoints, target, encoding, bom_len)
}

/// End offset (exclusive, without \n but including \r) of line starting at `start`.
pub fn line_end_offset(data: &[u8], start: u64, encoding: Encoding) -> u64 {
    let s = start as usize;
    if s >= data.len() {
        return start;
    }
    if encoding.is_wide() {
        let le = encoding == Encoding::Utf16Le;
        let mut i = s;
        if i % 2 == 1 {
            i += 1;
        }
        while i + 1 < data.len() {
            let is_nl = if le {
                data[i] == 0x0A && data[i + 1] == 0x00
            } else {
                data[i] == 0x00 && data[i + 1] == 0x0A
            };
            if is_nl {
                return i as u64;
            }
            i += 2;
        }
        data.len() as u64
    } else {
        let mut i = s;
        while i < data.len() {
            if data[i] == b'\n' {
                return i as u64;
            }
            i += 1;
        }
        data.len() as u64
    }
}

/// Byte range (start, end) for a 1-based line, end excludes \n. Returns None if missing.
pub fn line_byte_range(
    data: &[u8],
    checkpoints: &[(u64, u64)],
    line: u64,
    encoding: Encoding,
    bom_len: usize,
) -> Option<(u64, u64)> {
    let start = byte_offset_of_line(data, checkpoints, line, encoding, bom_len)?;
    let end = line_end_offset(data, start, encoding);
    Some((start, end))
}

/// Snap an arbitrary byte position to its line start.
pub fn snap_to_line_start(data: &[u8], pos: u64, encoding: Encoding, bom_len: usize) -> u64 {
    let mut p = (pos as usize).min(data.len());
    let start = bom_len.min(data.len());
    if p <= start {
        return start as u64;
    }
    if !encoding.is_wide() {
        while p > start {
            if data[p - 1] == b'\n' {
                break;
            }
            p -= 1;
        }
        p as u64
    } else {
        let le = encoding == Encoding::Utf16Le;
        // Align down to unit boundary relative to bom_len parity.
        // Scan backwards unit by unit for newline.
        let mut i = p;
        if (i as isize - bom_len as isize) % 2 != 0 && i > start {
            i -= 1;
        }
        while i > start {
            if i + 1 < data.len() + 2 {
                let is_nl = if i >= 2 {
                    let q = i - 2;
                    if q + 1 < data.len() {
                        if le {
                            data[q] == 0x0A && data[q + 1] == 0x00
                        } else {
                            data[q] == 0x00 && data[q + 1] == 0x0A
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                if is_nl {
                    break;
                }
            }
            if i < 2 {
                break;
            }
            i -= 2;
        }
        (i.max(start)) as u64
    }
}

/// Approximate byte for a scrollbar percent before index completes.
pub fn goto_percent_byte(size: u64, percent: f64, bom_len: usize) -> u64 {
    if size <= bom_len as u64 {
        return bom_len as u64;
    }
    let p = percent.clamp(0.0, 100.0) / 100.0;
    bom_len as u64 + (((size - bom_len as u64) as f64) * p) as u64
}

// ---------- sidecar ----------

fn mtime_key(mtime: Option<SystemTime>) -> (u64, u32) {
    if let Some(t) = mtime {
        match t.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => (d.as_secs(), d.subsec_nanos()),
            Err(_) => (0, 0),
        }
    } else {
        (0, 0)
    }
}

/// Persist sidecar when index completes. Plain text, deletable by user.
pub fn save_sidecar(
    sidecar: &Path,
    index: &SparseIndex,
    size: u64,
    mtime: Option<SystemTime>,
) -> std::io::Result<()> {
    use std::fmt::Write as _;
    let (s, n) = mtime_key(mtime);
    let mut out = String::new();
    let _ = writeln!(out, "{}", SIDECAR_MAGIC);
    let _ = writeln!(out, "{} {} {}", size, s, n);
    let _ = writeln!(out, "{} {}", index.total_lines, index.total_bytes);
    for (l, b) in &index.checkpoints {
        let _ = writeln!(out, "{} {}", l, b);
    }
    std::fs::write(sidecar, out)
}

/// Load sidecar only if size+mtime match. Returns None on any mismatch.
pub fn load_sidecar(sidecar: &Path, size: u64, mtime: Option<SystemTime>) -> Option<SparseIndex> {
    let text = std::fs::read_to_string(sidecar).ok()?;
    let mut lines = text.lines();
    if lines.next()? != SIDECAR_MAGIC {
        return None;
    }
    let header: Vec<&str> = lines.next()?.split_whitespace().collect();
    if header.len() != 3 {
        return None;
    }
    let fsize: u64 = header[0].parse().ok()?;
    let fsec: u64 = header[1].parse().ok()?;
    let fnano: u32 = header[2].parse().ok()?;
    let (cs, cn) = mtime_key(mtime);
    if fsize != size || fsec != cs || fnano != cn {
        return None;
    }
    let counts: Vec<&str> = lines.next()?.split_whitespace().collect();
    if counts.len() != 2 {
        return None;
    }
    let total_lines: u64 = counts[0].parse().ok()?;
    let total_bytes: u64 = counts[1].parse().ok()?;
    let mut checkpoints = Vec::new();
    for l in lines {
        if l.trim().is_empty() {
            continue;
        }
        let p: Vec<&str> = l.split_whitespace().collect();
        if p.len() != 2 {
            return None;
        }
        checkpoints.push((p[0].parse().ok()?, p[1].parse().ok()?));
    }
    Some(SparseIndex {
        checkpoints,
        total_lines,
        total_bytes,
        complete: true,
        progress: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_lf_and_crlf() {
        let data = b"a\nb\r\nc";
        let idx = build_full(data, Encoding::Utf8, 0);
        assert_eq!(idx.total_lines, 3);
        assert_eq!(line_byte_range(data, &idx.checkpoints, 1, Encoding::Utf8, 0), Some((0, 1)));
        assert_eq!(line_byte_range(data, &idx.checkpoints, 2, Encoding::Utf8, 0), Some((2, 4)));
        assert_eq!(line_byte_range(data, &idx.checkpoints, 3, Encoding::Utf8, 0), Some((5, 6)));
    }

    #[test]
    fn final_line_without_newline_and_trailing_newline() {
        let a = build_full(b"a\nb\n", Encoding::Utf8, 0);
        assert_eq!(a.total_lines, 2);
        let b = build_full(b"a\nb", Encoding::Utf8, 0);
        assert_eq!(b.total_lines, 2);
        let c = build_full(b"", Encoding::Utf8, 0);
        assert_eq!(c.total_lines, 0);
    }

    #[test]
    fn checkpoint_lookup_binary_search() {
        let cps = vec![(1, 0), (1025, 70000), (2049, 140000)];
        assert_eq!(checkpoint_lookup(&cps, 1), Some((1, 0)));
        assert_eq!(checkpoint_lookup(&cps, 1500), Some((1025, 70000)));
        assert_eq!(checkpoint_lookup(&cps, 3000), Some((2049, 140000)));
        assert_eq!(checkpoint_lookup(&cps, 0), None);
    }

    #[test]
    fn goto_line_via_checkpoints() {
        let mut data = Vec::new();
        for i in 0..5000 {
            data.extend_from_slice(format!("line {}\n", i).as_bytes());
        }
        let idx = build_full(&data, Encoding::Utf8, 0);
        assert!(idx.total_lines == 5000);
        assert!(!idx.checkpoints.is_empty());
        // line 2500 must resolve and start with expected text
        let (s, e) = line_byte_range(&data, &idx.checkpoints, 2500, Encoding::Utf8, 0).unwrap();
        assert!(data[s as usize..e as usize].starts_with(b"line 2499"));
    }
}
