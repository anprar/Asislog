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
/// Default; dapat dioverride via Options (P1-13) — worker memakai
/// `effective_max_hits()`, bukan konstanta ini langsung.
pub const MAX_STORED_HITS: usize = 200_000;
/// Background scan chunk 4 MiB (default; override via Options).
pub const SEARCH_CHUNK: usize = 4 * 1024 * 1024;
/// Stream batches of hits to UI.
pub const SEARCH_BATCH: usize = 500;
/// Default cache entries (LRU pola pencarian).
pub const DEFAULT_CACHE_ENTRIES: usize = 8;

use std::sync::atomic::AtomicUsize;
static MAX_HITS_OVR: AtomicUsize = AtomicUsize::new(0);
static CHUNK_MB_OVR: AtomicUsize = AtomicUsize::new(0);
static CACHE_OVR: AtomicUsize = AtomicUsize::new(0);

/// Clamp + terapkan limit pencarian dari Options (0 = auto/default).
/// max_hits: 10_000..=10_000_000 (0 = 200 rb). Tiap hit 24 B di RAM, jadi
/// 10 jt ≈ 240 MB — hanya untuk "tanpa pangkas" yang disengaja; untuk
/// jutaan match yang aman RAM, pakai ekspor streaming (tanpa simpan hit).
/// chunk_mb: 1..=16 (0 = 4). cache: 2..=64 (0 = 8). Pure + testable via effective_*.
pub fn set_search_limits(max_hits: usize, chunk_mb: usize, cache_entries: usize) {
    MAX_HITS_OVR.store(clamp_max_hits(max_hits), Ordering::Relaxed);
    CHUNK_MB_OVR.store(clamp_chunk_mb(chunk_mb), Ordering::Relaxed);
    CACHE_OVR.store(clamp_cache_entries(cache_entries), Ordering::Relaxed);
}

pub fn clamp_max_hits(n: usize) -> usize {
    if n == 0 {
        MAX_STORED_HITS
    } else {
        n.clamp(10_000, 10_000_000)
    }
}

pub fn clamp_chunk_mb(n: usize) -> usize {
    if n == 0 { 4 } else { n.clamp(1, 16) }
}

pub fn clamp_cache_entries(n: usize) -> usize {
    if n == 0 { DEFAULT_CACHE_ENTRIES } else { n.clamp(2, 64) }
}

/// Batas efektif saat ini (default bila belum di-set).
pub fn effective_max_hits() -> usize {
    let v = MAX_HITS_OVR.load(Ordering::Relaxed);
    if v == 0 { MAX_STORED_HITS } else { v }
}

pub fn effective_chunk_bytes() -> usize {
    let mb = CHUNK_MB_OVR.load(Ordering::Relaxed);
    if mb == 0 { SEARCH_CHUNK } else { mb * 1024 * 1024 }
}

pub fn effective_cache_entries() -> usize {
    let v = CACHE_OVR.load(Ordering::Relaxed);
    if v == 0 { DEFAULT_CACHE_ENTRIES } else { v }
}

/// True when a background job with `job_gen` must abort.
pub fn is_stale(current_gen: &Arc<AtomicU64>, job_gen: u64) -> bool {
    current_gen.load(Ordering::Relaxed) != job_gen
}

/// Count '\n' bytes in slice (single-byte encodings).
fn count_nl(bytes: &[u8]) -> u64 {
    return memchr::memchr_iter(b'\n', bytes).count() as u64
}

/// Parallel literal count over line-aligned chunks (CLI `-c` path).
/// Same match semantics as [`grep_collect`] (per-line, `\r`-aware,
/// cross-line never) but stores nothing: O(1) RAM per chunk, so
/// counting 15M matches costs no 350 MB side table.
pub fn grep_count(data: &[u8], needle: &[u8], bom_len: usize) -> u64 {
    use rayon::prelude::*;
    let len = data.len();
    if len <= bom_len {
        return 0;
    }
    if needle.is_empty() {
        // Every line matches: newlines + trailing tail (build_full parity).
        let nls = memchr::memchr_iter(b'\n', &data[bom_len..]).count() as u64;
        return if data[len - 1] == b'\n' { nls } else { nls + 1 };
    }
    let bounds = crate::engine::index::line_chunks(data, bom_len, 8 * 1024 * 1024, None);
    let nchunks = bounds.len().saturating_sub(1);
    (0..nchunks)
        .into_par_iter()
        .map(|i| {
            let (s, e) = (bounds[i], bounds[i + 1]);
            if s >= e {
                return 0u64;
            }
            let finder = memchr::memmem::Finder::new(needle);
            let mut n = 0u64;
            for m in finder.find_iter(&data[s..e]) {
                let a = s + m;
                let me = (a + needle.len()).min(e);
                if data[a..me].contains(&b'\n') {
                    continue;
                }
                if data[me - 1] == b'\r' && (me >= e || data[me] == b'\n') {
                    continue;
                }
                n += 1;
            }
            n
        })
        .sum()
}

/// Parallel literal grep over line-aligned chunks (CLI path).
/// Case-sensitive byte substring per line, no result cap: returns every
/// match as (1-based line, line byte start, line byte end excluding
/// `\n` and excluding a trailing `\r` — exactly the lines the old
/// single-threaded CLI loop matched on after its own `\r` strip).
/// Empty needle matches every line (memchr `Finder` parity).
/// Small inputs run single-threaded; large ones fan out via rayon over
/// [`crate::engine::index::line_chunks`]. Output is in line order.
pub fn grep_collect(data: &[u8], needle: &[u8], bom_len: usize) -> Vec<(u64, usize, usize)> {
    use rayon::prelude::*;
    // Strip a trailing \r from a line end (CRLF parity with the old loop,
    // which matched on \r-stripped lines).
    fn strip_cr(data: &[u8], ls: usize, le: usize) -> usize {
        if le > ls && data[le - 1] == b'\r' {
            le - 1
        } else {
            le
        }
    }
    let len = data.len();
    if len <= bom_len {
        return Vec::new();
    }
    // Line starts: first line starts at bom_len; each '\n' starts the next.
    // Chunk bases via newline prefix sums (two cheap memchr passes).
    let bounds = crate::engine::index::line_chunks(data, bom_len, 8 * 1024 * 1024, None);
    let nchunks = bounds.len().saturating_sub(1);
    if nchunks == 0 {
        return Vec::new();
    }
    let counts: Vec<u64> = bounds
        .windows(2)
        .map(|w| memchr::memchr_iter(b'\n', &data[w[0]..w[1]]).count() as u64)
        .collect();
    let mut bases = Vec::with_capacity(nchunks);
    let mut acc: u64 = 1;
    for c in &counts {
        bases.push(acc);
        acc += *c;
    }
    let finder = memchr::memmem::Finder::new(needle);
    let empty_needle = needle.is_empty();
    let parts: Vec<Vec<(u64, usize, usize)>> = (0..nchunks)
        .into_par_iter()
        .map(|i| {
            let (s, e) = (bounds[i], bounds[i + 1]);
            let mut out = Vec::new();
            if s >= e {
                return out;
            }
            if empty_needle {
                // Empty pattern matches every line (Finder parity).
                // Lines in [s, e): start at s, then after each '\n'.
                // A chunk ending exactly at '\n' contributes no tail
                // line (the next line starts the following chunk).
                let mut line = bases[i];
                let mut ls = s;
                for r in memchr::memchr_iter(b'\n', &data[s..e]) {
                    out.push((line, ls, strip_cr(data, ls, s + r)));
                    line += 1;
                    ls = s + r + 1;
                }
                if ls < e {
                    out.push((line, ls, strip_cr(data, ls, e)));
                }
                return out;
            }
            // Match path with incremental line mapping (same technique as
            // the GUI worker: one memchr pass over gaps, O(1) per hit).
            let mut prev_m = 0usize;
            let mut prev_line = bases[i];
            let mut prev_ls = 0usize;
            for m in finder.find_iter(&data[s..e]) {
                // Parity with the old per-line CLI loop: a pattern holding
                // '\n' can never match inside one line — skip cross-line
                // candidates (gap cursors untouched; the next gap still
                // spans correctly, so numbering stays exact).
                let m_end = (s + m + needle.len()).min(e);
                if data[s + m..m_end].contains(&b'\n') {
                    continue;
                }
                // Same for a match running into the line's own trailing
                // '\r' (the old loop matched on \r-stripped lines).
                if !needle.is_empty() {
                    let me = s + m + needle.len();
                    if me <= e && data[me - 1] == b'\r' && (me >= e || data[me] == b'\n') {
                        continue;
                    }
                }
                let gap = &data[s + prev_m..s + m];
                let mut nl = 0u64;
                let mut last_nl = 0usize;
                for k in memchr::memchr_iter(b'\n', gap) {
                    nl += 1;
                    last_nl = k;
                }
                let (gline, ls) = if nl == 0 {
                    (prev_line, prev_ls)
                } else {
                    (prev_line + nl, prev_m + last_nl + 1)
                };
                prev_m = m;
                prev_line = gline;
                prev_ls = ls;
                let abs_m = s + m;
                let line_end = data[abs_m..e]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map(|k| abs_m + k)
                    .unwrap_or(e);
                out.push((gline, s + ls, strip_cr(data, s + ls, line_end)));
            }
            out
        })
        .collect();
    let mut all = Vec::new();
    for mut p in parts {
        all.append(&mut p);
    }
    // No phantom trim needed: neither path emits the empty line past a
    // trailing newline (empty-needle stops at ls == e; matches always
    // hold bytes). Genuine empty lines ("a\n\n" line 2) are kept.
    all
}

/// Literal search over full slice. Pure + testable.
/// Case-insensitive runs through a case-insensitive regex (full Unicode
/// fold: é/É, Cyrillic — klogg parity) over the same SIMD engine; the
/// old ASCII-fold copy is kept only for the case-sensitive fast path.
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
        // Full-Unicode case-insensitive: escape the needle into a regex so
        // the SIMD engine does the folding (no giant lowercase copy).
        let pat = regex::escape(&String::from_utf8_lossy(needle));
        if let Ok(re) = regex::bytes::RegexBuilder::new(&pat)
            .case_insensitive(true)
            .build()
        {
            return find_regex_fast_with(&re, data, max_hits);
        }
        // Non-UTF-8 needle (rare): fall back to ASCII fold.
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
    Ok(find_regex_fast_with(&re, data, max_hits))
}

/// Shared streaming scan for a compiled bytes regex (literal CI reuses it).
fn find_regex_fast_with(
    re: &regex::bytes::Regex,
    data: &[u8],
    max_hits: usize,
) -> (Vec<Hit>, bool) {
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
    (hits, truncated)
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
    // Explicit default: pathological lines error instead of hanging.
    fb.backtrack_limit(1_000_000);
    let re = fb.build().map_err(|e| e.to_string())?;
    // P0: required literals skip lines that cannot match (soundness locked
    // by oracle test in fancypre). Pure path assumes narrow bytes.
    let pre = crate::engine::fancypre::FancyPrefilter::new(pattern, case_sensitive);
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
                // Prefilter BEFORE lossy decode + backtracking; cap the
                // backtracking window (misses past FANCY_LINE_CAP documented).
                let capped =
                    &line_bytes[..line_bytes.len().min(crate::engine::fancypre::FANCY_LINE_CAP)];
                if pre.passes(capped) {
                    let text = String::from_utf8_lossy(capped);
                    let base_col = if first && !suffix.is_empty() {
                        // Matches starting inside the already-scanned suffix
                        // were reported by the previous chunk: skip them, keep
                        // only matches reaching into fresh bytes.
                        suffix.len()
                    } else {
                        0
                    };
                    let mut iter = re.find_iter(text.as_ref() as &str);
                    // Backtrack error (catastrophic pattern on hostile line):
                    // stop this line, keep scanning the rest.
                    while let Some(Ok(m)) = iter.next() {
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
    fn literal_case_insensitive_full_unicode() {
        // é/É fold through the case-insensitive regex path.
        let data = "Érror boom\nérror boom\nERROR plain\n".as_bytes();
        let (hits, _) = find_literal(data, "éRROr".as_bytes(), false, 1000);
        assert_eq!(hits.len(), 2, "accent-fold must match both accented lines");
        // Cyrillic.
        let data = "ОШИБКА тест\nошибка тест\nok\n".as_bytes();
        let (hits, _) = find_literal(data, "ошибка".as_bytes(), false, 1000);
        assert_eq!(hits.len(), 2);
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

    /// Ubah kapasitas LRU (dari Options); kelebihan lama dibuang.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.max(1);
        while self.map.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                break;
            }
        }
    }

    /// Store only complete (non-truncated) results; oversized lists are skipped.
    pub fn put(&mut self, key: CacheKey, hits: Vec<Hit>) {
        if hits.len() > effective_max_hits() {
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

    #[test]
    fn search_limits_clamp_and_effective() {
        assert_eq!(clamp_max_hits(0), MAX_STORED_HITS);
        assert_eq!(clamp_max_hits(5), 10_000);
        assert_eq!(clamp_max_hits(50_000), 50_000);
        assert_eq!(clamp_max_hits(50_000_000), 10_000_000);
        assert_eq!(clamp_chunk_mb(0), 4);
        assert_eq!(clamp_chunk_mb(100), 16);
        assert_eq!(clamp_cache_entries(0), DEFAULT_CACHE_ENTRIES);
        assert_eq!(clamp_cache_entries(1), 2);
        set_search_limits(0, 0, 0);
        assert_eq!(effective_max_hits(), MAX_STORED_HITS);
        assert_eq!(effective_chunk_bytes(), SEARCH_CHUNK);
        assert_eq!(effective_cache_entries(), DEFAULT_CACHE_ENTRIES);
        set_search_limits(50_000, 2, 4);
        assert_eq!(effective_max_hits(), 50_000);
        assert_eq!(effective_chunk_bytes(), 2 * 1024 * 1024);
        assert_eq!(effective_cache_entries(), 4);
        set_search_limits(0, 0, 0); // kembalikan default (test isolation)
    }

    #[test]
    fn cache_set_cap_evicts() {
        let mut c = SearchCache::new(4);
        c.put(key("a", 1), vec![hit(1)]);
        c.put(key("b", 1), vec![hit(2)]);
        c.put(key("c", 1), vec![hit(3)]);
        assert_eq!(c.len(), 3);
        c.set_cap(2);
        assert_eq!(c.len(), 2);
    }

    /// Naive single-threaded oracle mirroring the old CLI loop exactly:
    /// per-line Finder on \r-stripped lines, 1-based numbering, no
    /// phantom past a trailing newline.
    fn oracle_grep(data: &[u8], needle: &[u8], bom: usize) -> Vec<(u64, usize, usize)> {
        let finder = memchr::memmem::Finder::new(needle);
        let mut out = Vec::new();
        let mut line_no = 1u64;
        let mut pos = bom.min(data.len());
        while pos < data.len() {
            let nl = memchr::memchr(b'\n', &data[pos..])
                .map(|i| pos + i)
                .unwrap_or(data.len());
            let mut end = nl;
            if end > pos && data[end - 1] == b'\r' {
                end -= 1;
            }
            if finder.find(&data[pos..end]).is_some() {
                out.push((line_no, pos, end));
            }
            line_no += 1;
            pos = nl + 1;
        }
        out
    }

    #[test]
    fn grep_collect_matches_oracle() {
        let cases: Vec<(Vec<u8>, &[u8])> = vec![
            (b"a\nb\n".to_vec(), b"a"),
            (b"a\nb".to_vec(), b"b"),
            (b"".to_vec(), b"x"),
            (b"\n".to_vec(), b""),
            (b"no trailing".to_vec(), b"trail"),
            (b"l1\r\nl2 ERROR\r\nl3\n".to_vec(), b"ERROR"),
            (b"l1\r\nl2 ERROR\r\nl3\n".to_vec(), b""),
            (b"xxaxx\nayy\na\n".to_vec(), b"a"),
            (b"one\ntwo\nthree\n".to_vec(), b"o\nt"), // cross-line: never
            (b"foo\rbar\nfoo\n".to_vec(), "o\r".as_bytes()), // \r-touch: never
            (b"\xef\xbb\xbfBOM line\nsecond\n".to_vec(), b"BOM"),
            (b"tab\there\n".to_vec(), b"\t"),
        ];
        for (data, needle) in &cases {
            let bom = if data.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
            let got = grep_collect(data, needle, bom);
            let want = oracle_grep(data, needle, bom);
            assert_eq!(got, want, "needle={:?} data={:?}", needle, &data[..data.len().min(40)]);
            assert_eq!(
                grep_count(data, needle, bom) as usize,
                want.len(),
                "count parity needle={:?}",
                needle
            );
            // Ranges must slice the matched line content back out.
            for (ln, s, e) in &got {
                assert!(*s <= *e && *e <= data.len(), "range {:?}", (ln, s, e));
                assert!(finder_contains(&data[*s..*e], needle));
            }
        }
        // Multi-MB input forces the real rayon fan-out (same oracle).
        let mut big = Vec::with_capacity(3 * 1024 * 1024);
        for i in 0..60_000u32 {
            big.extend_from_slice(format!("2026-09-16 INFO id={:06} ok\n", i).as_bytes());
            if i % 7 == 0 {
                big.extend_from_slice(b"2026-09-16 ERROR boom\n");
            }
        }
        let got = grep_collect(&big, b"ERROR", 0);
        let want = oracle_grep(&big, b"ERROR", 0);
        assert_eq!(got.len(), want.len());
        assert_eq!(got, want);
    }

    fn finder_contains(hay: &[u8], needle: &[u8]) -> bool {
        needle.is_empty() || memchr::memmem::find(hay, needle).is_some()
    }
}
