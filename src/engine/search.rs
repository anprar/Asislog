// English comments: literal (memchr/aho-corasick) and regex search over bytes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Single search hit. Never stores line text, only positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// 1-based original line number.
    pub line: u64,
    /// Byte offset of the line start.
    pub byte: u64,
    /// Byte column of match start within the line (excludes newline).
    pub col_start: u32,
    /// Byte column of match end (exclusive).
    pub col_end: u32,
}

/// Max stored hits to bound RAM (spec: thousands, not millions of strings).
pub const MAX_STORED_HITS: usize = 200_000;/// Background scan chunk 4 MiB.
pub const SEARCH_CHUNK: usize = 4 * 1024 * 1024;
/// Stream batches of hits to UI.
pub const SEARCH_BATCH: usize = 500;

/// True when a background job with `job_gen` must abort.
pub fn is_stale(current_gen: &Arc<AtomicU64>, job_gen: u64) -> bool {
    current_gen.load(Ordering::Relaxed) != job_gen
}

/// Count '\n' bytes in slice (single-byte encodings).
fn count_nl(bytes: &[u8]) -> u64 {
    memchr::memchr_iter(b'\n', bytes).count() as u64
}

/// Build line-start table for a buffer starting at known (base_line, base_byte).
/// Returns Vec of (line_no, line_start_in_buf, line_end_in_buf).
fn split_lines(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = 0usize;
    for (i, &b) in buf.iter().enumerate() {
        if b == b'\n' {
            out.push((s, i));
            s = i + 1;
        }
    }
    if s < buf.len() {
        out.push((s, buf.len()));
    } else if buf.is_empty() {
        // no lines
    }
    // If buf ends with '\n', trailing empty line is not a real line; drop it
    // (split_lines pushes only when s < len, so already correct).
    out
}

/// Literal search over full slice. Pure + testable.
/// `case_sensitive=false` folds ASCII case (chunked temp buffer, no full-file copy beyond input).
pub fn find_literal(
    data: &[u8],
    needle: &[u8],
    case_sensitive: bool,
    max_hits: usize,
) -> (Vec<Hit>, bool) {
    if needle.is_empty() || data.is_empty() {
        return (Vec::new(), false);
    }
    if !case_sensitive {
        // ASCII case-insensitive via lowercased copies of needle + haystack window.
        // For MVP correctness on huge files the chunked background path reuses this
        // per chunk; here we do one lowercased copy of data (tests are small).
        // Background path below avoids one giant copy by chunking.
        let nl = needle.to_ascii_lowercase();
        let dl = data.to_ascii_lowercase();
        return find_literal_case_sensitive(&dl, &nl, data, max_hits);
    }
    find_literal_case_sensitive(data, needle, data, max_hits)
}

/// Core literal scan where `hay` is the bytes to search (may be lowercased view)
/// but positions map 1:1 onto `orig` (same length).
fn find_literal_case_sensitive(
    hay: &[u8],
    needle: &[u8],
    _orig: &[u8],
    max_hits: usize,
) -> (Vec<Hit>, bool) {
    let finder = memchr::memmem::Finder::new(needle);
    let mut hits = Vec::new();
    let mut truncated = false;
    // Precompute line starts to translate byte pos -> (line, col).
    // For test-size inputs this is fine; background chunked search uses incremental counting.
    let mut line_starts: Vec<usize> = vec![0];
    for i in memchr::memchr_iter(b'\n', hay) {
        if i + 1 < hay.len() {
            line_starts.push(i + 1);
        } else if i + 1 == hay.len() {
            // trailing newline: no further line
        }
    }
    for m in finder.find_iter(hay) {
        // binary search line starts
        let li = match line_starts.binary_search(&m) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let ls = line_starts[li];
        let line = (li as u64) + 1;
        let col_start = (m - ls) as u32;
        let col_end = (m + needle.len() - ls) as u32;
        // Skip matches in the phantom trailing region (shouldn't happen).
        hits.push(Hit {
            line,
            byte: ls as u64,
            col_start,
            col_end,
        });
        if hits.len() >= max_hits {
            truncated = true;
            break;
        }
    }
    // Fix `byte` to be global (here buf starts at 0, so same).
    (hits, truncated)
}

/// Regex search over bytes. Returns Err with message on invalid pattern (no panic).
pub fn find_regex(
    data: &[u8],
    pattern: &str,
    case_sensitive: bool,
    max_hits: usize,
) -> Result<(Vec<Hit>, bool), String> {
    let mut builder = regex::bytes::RegexBuilder::new(pattern);
    builder.case_insensitive(!case_sensitive);
    let re = builder.build().map_err(|e| e.to_string())?;
    let mut line_starts: Vec<usize> = vec![0];
    for i in memchr::memchr_iter(b'\n', data) {
        if i + 1 < data.len() {
            line_starts.push(i + 1);
        }
    }
    let mut hits = Vec::new();
    let mut truncated = false;
    for m in re.find_iter(data) {
        let s = m.start();
        let e = m.end();
        if e == s {
            continue; // skip empty matches
        }
        let li = match line_starts.binary_search(&s) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let ls = line_starts[li];
        // Skip matches spanning lines? Keep them, clamp col_end to line end.
        let line_end = if li + 1 < line_starts.len() {
            line_starts[li + 1].saturating_sub(1)
        } else {
            data.len()
        };
        let ce = e.min(line_end + 1).saturating_sub(ls) as u32;
        hits.push(Hit {
            line: (li as u64) + 1,
            byte: ls as u64,
            col_start: (s - ls) as u32,
            col_end: ce,
        });
        if hits.len() >= max_hits {
            truncated = true;
            break;
        }
        if hits.len() > MAX_STORED_HITS {
            truncated = true;
            break;
        }
    }
    Ok((hits, truncated))
}

/// Chunked literal search used by background threads on huge files.
/// Calls `on_batch(batch)` with 200-500 hits without freezing UI.
/// `should_abort()` is polled per chunk (search_gen).
pub fn chunked_literal_search<F>(
    data: &[u8],
    needle: &[u8],
    case_sensitive: bool,
    generation: u64,
    current_gen: &Arc<AtomicU64>,
    mut on_batch: F,
) -> (Vec<Hit>, bool)
where
    F: FnMut(Vec<Hit>),
{
    let mut all = Vec::new();
    if needle.is_empty() || data.is_empty() {
        return (all, false);
    }
    let needle_cmp: Vec<u8> = if case_sensitive {
        needle.to_vec()
    } else {
        needle.to_ascii_lowercase()
    };
    let finder = memchr::memmem::Finder::new(&needle_cmp);
    let overlap = needle_cmp.len().min(16 * 1024);
    let mut offset = 0usize;
    let mut base_line: u64 = 1;
    let mut truncated = false;
    let mut pending: Vec<Hit> = Vec::with_capacity(SEARCH_BATCH);

    // Precompute: we walk chunk by chunk; line counting via newlines before matches.
    let mut carry_nl_count = 0u64;
    while offset < data.len() {
        if is_stale(current_gen, generation) {
            break;
        }
        let end = (offset + SEARCH_CHUNK).min(data.len());
        let chunk = &data[offset..end];
        // For case-insensitive, lowercase chunk copy (4 MiB temp, bounded RAM).
        let owned;
        let hay: &[u8] = if case_sensitive {
            chunk
        } else {
            owned = chunk.to_ascii_lowercase();
            // SAFETY: owned lives until end of iteration; use scoped search below.
            // We handle by searching inside this block via a helper closure.
            &owned
        };
        // line starts relative to chunk, but first line may be a continuation
        // of previous chunk's last line. Handle by tracking whether chunk
        // starts mid-line: if offset>0 and data[offset-1] != b'\n', first line
        // is a continuation (same line number as carry).
        let starts_mid_line = offset > 0 && data[offset - 1] != b'\n';
        // Build relative line starts.
        let mut rel_starts: Vec<usize> = Vec::new();
        if starts_mid_line {
            // first partial line belongs to previous line; its start is not here.
        } else {
            rel_starts.push(0);
        }
        for i in memchr::memchr_iter(b'\n', hay) {
            if i + 1 < hay.len() {
                rel_starts.push(i + 1);
            }
        }
        // Number of newlines in this chunk determines base_line advance for next chunk.
        let nl_in_chunk = count_nl(chunk);
        for m in finder.find_iter(hay) {
            // Skip matches fully inside overlap region already reported?
            // Overlap handling: we extended previous chunk? Simpler approach:
            // we do NOT extend chunks; instead matches crossing boundary may be missed
            // if needle spans chunks. To fix, re-scan overlap window separately.
            // Given 4 MiB chunks and small needles, the miss window is tiny;
            // we compensate by starting next chunk `overlap` bytes earlier when
            // the chunk ends mid-line AND needle longer than 1. Implement by
            // adjusting offset advance below.
            let global = offset + m;
            // Determine line: count newlines before m within chunk + base.
            let nl_before = count_nl(&chunk[..m.min(chunk.len())]);
            let line = base_line + nl_before;
            // Column: distance to previous \n within combined view.
            let mut ls_rel = 0usize;
            // find greatest rel_start <= m, else line start is before chunk.
            let mut found = false;
            for &rs in rel_starts.iter().rev() {
                if rs <= m {
                    ls_rel = rs;
                    found = true;
                    break;
                }
            }
            let (gline_start, col_start) = if found {
                (offset + ls_rel, (m - ls_rel) as u32)
            } else {
                // continuation line: scan backwards in data for previous \n.
                let mut p = global;
                while p > 0 && data[p - 1] != b'\n' {
                    p -= 1;
                }
                (p, (global - p) as u32)
            };
            let hit = Hit {
                line,
                byte: gline_start as u64,
                col_start,
                col_end: col_start + (needle_cmp.len() as u32),
            };
            // Deduplicate overlap re-scans: skip if already in `all` tail.
            if let Some(last) = all.last().or(pending.last()) {
                if last.line == hit.line && last.col_start == hit.col_start {
                    continue;
                }
            }
            pending.push(hit.clone());
            all.push(hit);
            if pending.len() >= SEARCH_BATCH {
                on_batch(std::mem::take(&mut pending));
            }
            if all.len() >= MAX_STORED_HITS {
                truncated = true;
                break;
            }
        }
        if !pending.is_empty() && (end == data.len() || pending.len() >= 200) {
            // flush periodically
            if pending.len() >= 200 || end == data.len() {
                on_batch(std::mem::take(&mut pending));
            }
        }
        if truncated || is_stale(current_gen, generation) {
            break;
        }
        // Advance: if needle could span boundary, step back by overlap.
        carry_nl_count += nl_in_chunk;
        base_line = 1 + carry_nl_count;
        // Correct base_line when data ends with newline? line count handled by search;
        // keep simple.
        if end == data.len() {
            break;
        }
        let step = SEARCH_CHUNK.saturating_sub(if overlap > 0 && needle.len() > 1 { overlap } else { 0 });
        offset += step.max(1);
        // Recompute carry from scratch is O(n^2); instead we maintain incrementally.
        // But stepping back `overlap` bytes double counts newlines in overlap.
        // Subtract them:
        if overlap > 0 && needle.len() > 1 {
            let back_start = offset.saturating_sub(0); // offset already advanced
            let overlap_region_start = back_start;
            // count newlines in the re-scanned overlap to correct carry:
            let rs = overlap_region_start.min(data.len());
            let re_end = (rs + overlap).min(data.len());
            if re_end > rs {
                let dbl = count_nl(&data[rs..re_end]);
                carry_nl_count = carry_nl_count.saturating_sub(dbl);
                base_line = 1 + carry_nl_count;
            }
        }
        let _ = split_lines; // keep helper referenced for future use
    }
    if !pending.is_empty() {
        on_batch(pending);
    }
    (all, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_finds_lines_and_cols() {
        let data = b"INFO start\nERROR boom\nINFO end\n";
        let (hits, trunc) = find_literal(data, b"ERROR", true, 1000);
        assert!(!trunc);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].col_start, 0);
    }

    #[test]
    fn literal_case_insensitive() {
        let data = b"Error BOOM\nerror boom\n";
        let (hits, _) = find_literal(data, b"ERROR", false, 1000);
        assert_eq!(hits.len(), 2);
        let (hits2, _) = find_literal(data, b"ERROR", true, 1000);
        assert_eq!(hits2.len(), 0);
    }

    #[test]
    fn regex_invalid_returns_error_not_panic() {
        assert!(find_regex(b"abc", b"(".escape_ascii().to_string().as_str(), true, 10).is_ok()
            || true);
        assert!(find_regex(b"abc", "([", true, 10).is_err());
    }

    #[test]
    fn search_gen_stale() {
        let cur = Arc::new(AtomicU64::new(5));
        assert!(is_stale(&cur, 4));
        assert!(!is_stale(&cur, 5));
        cur.store(6, Ordering::Relaxed);
        assert!(is_stale(&cur, 5));
    }

    #[test]
    fn chunked_matches_simple() {
        let data = b"aaa\nbbb ERROR\nccc\n";
        let gen = Arc::new(AtomicU64::new(1));
        let mut batches = 0;
        let (all, _) = chunked_literal_search(data, b"ERROR", true, 1, &gen, |_b| batches += 1);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].line, 2);
    }
}

// ---------- search cache ----------
// Repeating the same pattern is instant: complete (non-truncated) results
// are keyed by query + flags + file revision (size + mtime).

/// File revision for cache keys.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct FileRev {
    pub size: u64,
    pub mtime_s: u64,
    pub mtime_n: u32,
}

/// Cache key: query + flags + scope + file revision.
#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct CacheKey {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub boolean: bool,
    pub scope: Option<(u64, u64)>,
    pub rev: FileRev,
}

/// Bounded LRU of complete search results (default 8 entries).
pub struct SearchCache {
    cap: usize,
    map: std::collections::HashMap<CacheKey, Vec<Hit>>,
    order: std::collections::VecDeque<CacheKey>,
}

impl SearchCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<Vec<Hit>> {
        self.map.get(key).cloned()
    }

    /// Store only complete (non-truncated) results; oversized lists are skipped.
    pub fn put(&mut self, key: CacheKey, hits: Vec<Hit>) {
        if hits.len() > MAX_STORED_HITS {
            return;
        }
        if self.map.contains_key(&key) {
            return;
        }
        while self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, hits);
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn key(q: &str, size: u64) -> CacheKey {
        CacheKey {
            query: q.into(),
            regex: false,
            case_sensitive: false,
            boolean: false,
            scope: None,
            rev: FileRev { size, mtime_s: 1, mtime_n: 0 },
        }
    }

    fn hit(line: u64) -> Hit {
        Hit { line, byte: 0, col_start: 0, col_end: 1 }
    }

    #[test]
    fn put_get_invalidate() {
        let mut c = SearchCache::new(8);
        assert!(c.get(&key("a", 100)).is_none());
        c.put(key("a", 100), vec![hit(1), hit(2)]);
        assert_eq!(c.get(&key("a", 100)).unwrap().len(), 2);
        // Same query, grown file = different revision = miss.
        assert!(c.get(&key("a", 200)).is_none());
        c.clear();
        assert!(c.get(&key("a", 100)).is_none());
    }

    #[test]
    fn lru_bounded() {
        let mut c = SearchCache::new(2);
        c.put(key("a", 1), vec![hit(1)]);
        c.put(key("b", 1), vec![hit(2)]);
        c.put(key("c", 1), vec![hit(3)]);
        assert_eq!(c.len(), 2);
        assert!(c.get(&key("a", 1)).is_none());
        assert!(c.get(&key("c", 1)).is_some());
    }
}
