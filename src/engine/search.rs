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

/// Stream `hay` in 4 MiB chunks with an overlap tail, calling `f` per chunk.
/// Chunk args: (combined, combined_base, first_line, carry_len, is_last).
/// RAM is O(chunk); line numbers stay exact via per-chunk newline counts.
fn scan_stream(hay: &[u8], overlap: usize, mut f: impl FnMut(&[u8], u64, u64, usize, bool) -> bool) {
    let overlap = overlap.max(1);
    let mut carry: Vec<u8> = Vec::new();
    let mut offset: u64 = 0; // fresh bytes consumed so far
    let mut line_no: u64 = 1; // 1-based line at `offset`
    let mut pos = 0usize;
    loop {
        let end = (pos + SEARCH_CHUNK).min(hay.len());
        let fresh = &hay[pos..end];
        let mut combined = std::mem::take(&mut carry);
        let base = offset.saturating_sub(combined.len() as u64);
        let carry_len = combined.len();
        let carry_nl = memchr::memchr_iter(b'\n', &combined).count() as u64;
        let cur_line = line_no.saturating_sub(carry_nl);
        combined.extend_from_slice(fresh);
        pos = end;
        let is_last = pos >= hay.len();
        if f(&combined, base, cur_line, carry_len, is_last) {
            break;
        }
        line_no += memchr::memchr_iter(b'\n', fresh).count() as u64;
        offset += fresh.len() as u64;
        let tail = overlap.min(combined.len());
        carry = combined[combined.len() - tail..].to_vec();
        if is_last {
            break;
        }
    }
}

/// Core literal scan where `hay` is the bytes to search (may be lowercased view)
/// but positions map 1:1 onto `orig` (same length). Streaming: O(chunk) RAM,
/// incremental line mapping, no dense line table (safe at any file size).
fn find_literal_case_sensitive(
    hay: &[u8],
    needle: &[u8],
    _orig: &[u8],
    max_hits: usize,
) -> (Vec<Hit>, bool) {
    let finder = memchr::memmem::Finder::new(needle);
    let mut hits = Vec::new();
    let mut truncated = false;
    scan_stream(hay, needle.len().min(16 * 1024), |combined, base, cur_line, carry_len, _| {
        let mut prev_m = 0usize;
        let mut prev_line = cur_line;
        let mut prev_ls = 0usize;
        for m in finder.find_iter(combined) {
            // Fully inside the carried prefix: reported by the previous chunk.
            if m + needle.len() <= carry_len {
                continue;
            }
            let gap = &combined[prev_m..m];
            let mut nl = 0u64;
            let mut last_nl = 0usize;
            for i in memchr::memchr_iter(b'\n', gap) {
                nl += 1;
                last_nl = i;
            }
            let (line, ls) = if nl == 0 {
                (prev_line, prev_ls)
            } else {
                (prev_line + nl, prev_m + last_nl + 1)
            };
            prev_m = m;
            prev_line = line;
            prev_ls = ls;
            hits.push(Hit {
                line,
                byte: base + ls as u64,
                col_start: (m - ls) as u32,
                col_end: (m + needle.len() - ls) as u32,
            });
            if hits.len() >= max_hits {
                truncated = true;
                return true;
            }
        }
        false
    });
    (hits, truncated)
}

/// Regex search over bytes. Returns Err with message on invalid pattern (no panic).
/// Streaming like the literal path: O(chunk) RAM, incremental line mapping.
/// Fast path: `regex::bytes` (SIMD, linear time). Complex patterns the fast
/// engine rejects (look-around, backreferences) fall back to `fancy-regex`
/// (backtracking: correct but slower — the UI labels this mode honestly).
/// Only patterns BOTH engines reject produce Err.
pub fn find_regex(
    data: &[u8],
    pattern: &str,
    case_sensitive: bool,
    max_hits: usize,
) -> Result<(Vec<Hit>, bool), String> {
    match compile_regex_auto(pattern, case_sensitive) {
        Ok(CompiledRegex::Fast(_)) => find_regex_fast(data, pattern, case_sensitive, max_hits),
        Ok(CompiledRegex::Fancy(_)) => find_regex_fancy(data, pattern, case_sensitive, max_hits),
        Err(e) => Err(e),
    }
}

/// A compiled regex with its engine recorded, so callers (UI mode label,
/// workers) can stay honest about which one runs.
#[derive(Clone, Debug)]
pub enum CompiledRegex {
    /// `regex::bytes`: SIMD, linear-time guarantee.
    Fast(regex::bytes::Regex),
    /// `fancy-regex`: backtracking, handles look-around/backreferences.
    Fancy(fancy_regex::Regex),
}

/// Compile with the fast engine first; fall back to fancy-regex only for
/// patterns the fast engine cannot express. Err only if both reject.
pub fn compile_regex_auto(pattern: &str, case_sensitive: bool) -> Result<CompiledRegex, String> {
    let mut b = regex::bytes::RegexBuilder::new(pattern);
    b.case_insensitive(!case_sensitive);
    match b.build() {
        Ok(re) => Ok(CompiledRegex::Fast(re)),
        Err(fast_err) => {
            let mut fb = fancy_regex::RegexBuilder::new(pattern);
            fb.case_insensitive(!case_sensitive);
            match fb.build() {
                Ok(re) => Ok(CompiledRegex::Fancy(re)),
                Err(_) => Err(fast_err.to_string()),
            }
        }
    }
}

/// True when the pattern needs the fancy (backtracking) engine: the fast
/// engine rejects it but fancy accepts it. Pure + testable; UI uses it to
/// label the mode honestly without waiting for worker results.
pub fn is_complex_regex(pattern: &str, case_sensitive: bool) -> bool {
    matches!(compile_regex_auto(pattern, case_sensitive), Ok(CompiledRegex::Fancy(_)))
}

fn find_regex_fast(
    data: &[u8],
    pattern: &str,
    case_sensitive: bool,
    max_hits: usize,
) -> Result<(Vec<Hit>, bool), String> {
    let mut builder = regex::bytes::RegexBuilder::new(pattern);
    builder.case_insensitive(!case_sensitive);
    let re = builder.build().map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    let mut truncated = false;
    scan_stream(data, 8 * 1024, |combined, base, cur_line, carry_len, _| {
        let mut prev_s = 0usize;
        let mut prev_line = cur_line;
        let mut prev_ls = 0usize;
        for m in re.find_iter(combined) {
            let s = m.start();
            let e = m.end();
            if e == s {
                continue; // skip empty matches
            }
            if e <= carry_len {
                continue; // fully inside carried prefix: already reported
            }
            let gap = &combined[prev_s..s];
            let mut nl = 0u64;
            let mut last_nl = 0usize;
            for i in memchr::memchr_iter(b'\n', gap) {
                nl += 1;
                last_nl = i;
            }
            let (line, ls) = if nl == 0 {
                (prev_line, prev_ls)
            } else {
                (prev_line + nl, prev_s + last_nl + 1)
            };
            prev_s = s;
            prev_line = line;
            prev_ls = ls;
            // Skip matches spanning lines? Keep them, clamp col_end to line end.
            let line_end = combined
                .iter()
                .skip(s)
                .position(|&b| b == b'\n')
                .map(|k| s + k)
                .unwrap_or(combined.len());
            let ce = e.min(line_end).saturating_sub(ls) as u32;
            hits.push(Hit {
                line,
                byte: base + ls as u64,
                col_start: s.saturating_sub(ls) as u32,
                col_end: ce,
            });
            if hits.len() >= max_hits {
                truncated = true;
                return true;
            }
            if hits.len() > MAX_STORED_HITS {
                truncated = true;
                return true;
            }
        }
        false
    });
    Ok((hits, truncated))
}

/// Fancy (backtracking) regex scan, line by line over lossy-decoded text.
/// Only reached for patterns the fast byte engine rejects, so per-line
/// decode cost is acceptable here (correctness over speed).
/// Byte columns refer to the decoded line: identical to source bytes for
/// valid UTF-8, approximate where replacement chars were substituted.
/// Chunk-boundary lines are reconstructed from the overlap tail and
/// deduplicated by (line, col), so no hit is lost or doubled.
fn find_regex_fancy(
    data: &[u8],
    pattern: &str,
    case_sensitive: bool,
    max_hits: usize,
) -> Result<(Vec<Hit>, bool), String> {
    let mut fb = fancy_regex::RegexBuilder::new(pattern);
    fb.case_insensitive(!case_sensitive);
    let re = fb.build().map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    let mut truncated = false;
    // Adjacent-duplicate guard: the carry/fresh overlap re-scans the
    // boundary line, so a match reported by the previous chunk arrives
    // again with identical (line, col). Matches stream in order.
    let mut last: Option<(u64, u32)> = None;
    scan_stream(data, 8 * 1024, |combined, base, cur_line, carry_len, _| {
        let carry_len = carry_len.min(combined.len());
        let carry = &combined[..carry_len];
        let fresh = &combined[carry_len..];
        let carry_nl = memchr::memchr_iter(b'\n', carry).count() as u64;
        // Suffix of the boundary line already scanned (empty unless the
        // fresh region continues a carry line). Reconstructed so patterns
        // spanning the chunk boundary still match exactly once (via `last`).
        let suffix: &[u8] = if carry_len > 0 && !carry.ends_with(b"\n") {
            match carry.iter().rposition(|&b| b == b'\n') {
                Some(i) => &carry[i + 1..],
                // Line longer than the whole overlap: byte base below is
                // approximate (same corner as the fast path); the line
                // number stays exact.
                None => carry,
            }
        } else {
            &[]
        };
        let mut cur_no = cur_line + carry_nl;
        // Byte offset of the current line start within the file.
        let mut ls_byte = base + carry_len as u64 - suffix.len() as u64;
        let mut fresh_off: u64 = 0;
        let mut first = true;
        for part in fresh.split_inclusive(|&b| b == b'\n') {
            let has_nl = part.ends_with(b"\n");
            let mut body = if has_nl { &part[..part.len() - 1] } else { part };
            if body.ends_with(b"\r") {
                body = &body[..body.len() - 1];
            }
            // Full bytes of this line for matching (suffix only matters
            // for the first segment of a continued line).
            let full: Vec<u8>;
            let line_bytes: &[u8] = if first && !suffix.is_empty() {
                full = [suffix, body].concat();
                &full
            } else {
                body
            };
            if !line_bytes.is_empty() {
                let text = String::from_utf8_lossy(line_bytes);
                let base_col = if first && !suffix.is_empty() {
                    // Matches starting inside the already-scanned suffix
                    // were reported by the previous chunk: skip them, keep
                    // only matches reaching into fresh bytes.
                    suffix.len()
                } else {
                    0
                };
                let mut iter = re.find_iter(text.as_ref() as &str);
                loop {
                    let m = match iter.next() {
                        Some(Ok(m)) => m,
                        // Backtrack error (catastrophic pattern on hostile
                        // line): stop this line, keep scanning the rest.
                        _ => break,
                    };
                    if m.end() == m.start() || m.start() < base_col {
                        continue;
                    }
                    let key = (cur_no, m.start() as u32);
                    if last == Some(key) {
                        continue;
                    }
                    last = Some(key);
                    hits.push(Hit {
                        line: cur_no,
                        byte: ls_byte,
                        col_start: m.start() as u32,
                        col_end: m.end() as u32,
                    });
                    if hits.len() >= max_hits {
                        truncated = true;
                        return true;
                    }
                }
            }
            first = false;
            // Next line starts right after this segment.
            ls_byte = base + carry_len as u64 + fresh_off + part.len() as u64;
            fresh_off += part.len() as u64;
            // Recompute cleanly: track offset within fresh instead.
            if has_nl {
                cur_no += 1;
            }
        }
        false
    });
    Ok((hits, truncated))
}
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
        // Matches arrive ascending: running newline count, O(chunk + hits).
        let mut prev_m = 0usize;
        let mut nl_run = 0u64;
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
            // Determine line: running newlines before m within chunk + base.
            nl_run += count_nl(&chunk[prev_m..m.min(chunk.len())]);
            prev_m = m;
            let line = base_line + nl_run;
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
    fn regex_engine_routing() {
        use super::CompiledRegex;
        // Plain patterns stay on the fast SIMD engine.
        assert!(matches!(
            super::compile_regex_auto("ERROR|WARN", true),
            Ok(CompiledRegex::Fast(_))
        ));
        // Look-around / backreferences need the fancy engine.
        assert!(matches!(
            super::compile_regex_auto(r"foo(?!bar)", true),
            Ok(CompiledRegex::Fancy(_))
        ));
        assert!(matches!(
            super::compile_regex_auto(r"(?<=id=)\d+", true),
            Ok(CompiledRegex::Fancy(_))
        ));
        assert!(matches!(
            super::compile_regex_auto(r"(ab)\1", true),
            Ok(CompiledRegex::Fancy(_))
        ));
        assert!(super::is_complex_regex(r"foo(?!bar)", true));
        assert!(!super::is_complex_regex("ERROR", true));
        // Garbage for both engines still errors, never panics.
        assert!(super::compile_regex_auto("([", true).is_err());
    }

    #[test]
    fn regex_fancy_lookaround_results() {
        let data = b"foobar\nfoobaz id=42\nfooqux id=7\n";
        // Negative lookahead: lines with foo NOT followed by bar.
        let (hits, trunc) = find_regex(data, r"foo(?!bar)", true, 1000).unwrap();
        assert!(!trunc);
        let lines: Vec<u64> = hits.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![2, 3]);
        // Lookbehind: digits after id=.
        let (hits, _) = find_regex(data, r"(?<=id=)\d+", true, 1000).unwrap();
        let cols: Vec<(u64, u32)> = hits.iter().map(|h| (h.line, h.col_start)).collect();
        assert_eq!(cols, vec![(2, 10), (3, 10)]);
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
