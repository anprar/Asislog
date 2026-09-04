// English comments: Background workers: literal/regex and boolean search (split from app.rs; behavior unchanged).
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

/// Shared plumbing for background search workers (keeps spawn_* signatures small).
pub(crate) struct SearchJobParams {
    pub path: PathBuf,
    pub gen: u64,
    pub gen_shared: Arc<AtomicU64>,
    pub tx: mpsc::Sender<SearchBatchMsg>,
    pub cancel: Arc<AtomicBool>,
}

/// True when the job must abort: superseded query (gen) or tab closed (cancel).
fn job_stale(gen_shared: &Arc<AtomicU64>, gen: u64, cancel: &Arc<AtomicBool>) -> bool {
    cancel.load(Ordering::Relaxed) || gen_shared.load(Ordering::Relaxed) != gen
}

pub(crate) fn spawn_search(
    params: SearchJobParams,
    query: String,
    regex_on: bool,
    case_sensitive: bool,
    scope: Option<(u64, u64)>,
    // Tail refresh: start scanning at (byte, 1-based line) instead of 0,
    // so follow-append re-scans only new bytes. None = full scan.
    seek_to: Option<(u64, u64)>,
) {
    std::thread::spawn(move || {
        use std::io::Read;
        let SearchJobParams { path, gen, gen_shared, tx, cancel } = params;
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
        let mut carry: Vec<u8> = Vec::new(); // overlap tail
        let mut global_offset: u64 = 0;
        let mut line_no: u64 = 1;
        if let Some((sb, sl)) = seek_to {
            // Gagal seek = pindai penuh (benar, lebih lambat). Praktis tak terjadi.
            use std::io::Seek;
            if reader.seek(std::io::SeekFrom::Start(sb)).is_ok() {
                global_offset = sb;
                line_no = sl.max(1);
            }
        }
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
            if job_stale(&gen_shared, gen, &cancel) {
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
            let cur_line = line_no.saturating_sub(carry_nl);
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
                    // Length-preserving ASCII fold, so offsets match `combined`.
                    drop(combined);
                    combined = hay_owned;
                    &combined
                };
                let f = finder.as_ref().unwrap();
                // Incremental line mapping: `find_iter` yields matches in
                // ascending order, so line(m) = line(prev) + newlines in
                // hay[prev_m..m]. One memchr pass total per chunk, O(1) per
                // hit — no line table, no binary search. Anchored at
                // combined[0], whose line is cur_line.
                let mut prev_m = 0usize;
                let mut prev_line = cur_line;
                let mut prev_ls = 0usize;
                for m in f.find_iter(hay) {
                    if job_stale(&gen_shared, gen, &cancel) {
                        return;
                    }
                    // Matches fully inside the carried prefix were already
                    // reported by the previous chunk (all their bytes were
                    // visible there). Matches reaching into fresh bytes are
                    // new: the previous chunk could not yield them.
                    if m + needle_cmp.len() <= carry_len {
                        continue;
                    }
                    // Advance (line, line-start) from the previous match.
                    // Gaps partition hay[0..m], so the newline total is exact.
                    let gap = &hay[prev_m..m];
                    let mut nl = 0u64;
                    let mut last_nl = 0usize;
                    for i in memchr::memchr_iter(b'\n', gap) {
                        nl += 1;
                        last_nl = i;
                    }
                    let (gline, ls) = if nl == 0 {
                        (prev_line, prev_ls)
                    } else {
                        (prev_line + nl, prev_m + last_nl + 1)
                    };
                    prev_m = m;
                    prev_line = gline;
                    prev_ls = ls;
                    let hit = Hit {
                        line: gline,
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
                // Fresh bytes only (carry was counted in its own chunk).
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
                // Same incremental line mapping as the literal path: no
                // line table, no binary search, O(1) amortised per hit.
                let mut prev_s = 0usize;
                let mut prev_line = cur_line;
                let mut prev_ls = 0usize;
                for m in re.find_iter(&combined) {
                    if job_stale(&gen_shared, gen, &cancel) {
                        return;
                    }
                    let (s, e) = (m.start(), m.end());
                    if e == s {
                        continue;
                    }
                    // Same dedup as the literal path: fully inside the carried
                    // prefix means the previous chunk already reported it.
                    if e <= carry_len {
                        continue;
                    }
                    if let Some((ss, ee)) = scope {
                        let gp = combined_base + s as u64;
                        if gp < ss || gp >= ee {
                            continue;
                        }
                    }
                    // Advance (line, line-start) from the previous match.
                    let gap = &combined[prev_s..s];
                    let mut nl = 0u64;
                    let mut last_nl = 0usize;
                    for i in memchr::memchr_iter(b'\n', gap) {
                        nl += 1;
                        last_nl = i;
                    }
                    let (gline, ls) = if nl == 0 {
                        (prev_line, prev_ls)
                    } else {
                        (prev_line + nl, prev_s + last_nl + 1)
                    };
                    prev_s = s;
                    prev_line = gline;
                    prev_ls = ls;
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
        if job_stale(&gen_shared, gen, &cancel) {
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
pub(crate) fn spawn_bool_search(
    params: SearchJobParams,
    ast: crate::engine::query::Query,
    encoding: Encoding,
    bom_len: usize,
    case_sensitive: bool,
    scope_lines: Option<(u64, u64)>,
    // Tail refresh: start at (byte, 1-based line). None = full scan.
    seek_to: Option<(u64, u64)>,
) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let SearchJobParams { path, gen, gen_shared, tx, cancel } = params;
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
        // Byte prefilter over positive terms (Aho-Corasick, zero decode):
        // only candidate lines pay decode + full AST eval. Enabled solely
        // when the union is sound (Query::is_prefilter_safe); otherwise every
        // line still goes through the exact path below.
        let prefilter: Option<aho_corasick::AhoCorasick> =
            if ast.is_prefilter_safe() && !terms.is_empty() {
                let mut builder = aho_corasick::AhoCorasickBuilder::new();
                if !case_sensitive {
                    builder.ascii_case_insensitive(true);
                }
                builder.build(&terms).ok()
            } else {
                None
            };
        // Conjunction-aware fast path: every full match must contain each
        // required term, so ONE Finder on the longest (= most selective)
        // beats the union automaton and subsumes it (its passers are a
        // subset of union passers). Sound for any shape, no safety gate.
        let required_pat: Option<Vec<u8>> = ast
            .required_terms()
            .into_iter()
            .max_by_key(|t| t.len())
            .map(|t| {
                if case_sensitive {
                    t.as_bytes().to_vec()
                } else {
                    t.as_bytes().to_ascii_lowercase()
                }
            })
            .filter(|p| !p.is_empty());
        let required_finder: Option<memchr::memmem::Finder> =
            required_pat.as_deref().map(memchr::memmem::Finder::new);
        // Scratch fold buffer (reused per line, no realloc churn).
        let mut fold_buf: Vec<u8> = Vec::new();
        let mut line_no: u64 = 1;
        let mut byte_off: u64 = 0;
        let mut scanned: u64 = 0;
        let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
        let mut total_found: usize = 0;
        let mut truncated = false;
        let mut buf: Vec<u8> = Vec::new();
        // Lewati BOM pada baris pertama (kecuali seek melewatinya).
        let mut first = true;
        if let Some((sb, sl)) = seek_to {
            use std::io::Seek;
            if reader.seek(std::io::SeekFrom::Start(sb)).is_ok() {
                byte_off = sb;
                scanned = sb;
                line_no = sl.max(1);
                first = sb == 0;
            }
        }
        loop {
            if job_stale(&gen_shared, gen, &cancel) {
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
            // Prefilter hierarki: required-term tunggal (paling selektif,
            // subsumes union) dulu, lalu union AC, lalu jalur eksak.
            if let Some(f) = required_finder.as_ref() {
                let hit = if case_sensitive {
                    f.find(bytes).is_some()
                } else {
                    fold_buf.clear();
                    fold_buf.extend_from_slice(bytes);
                    fold_buf.make_ascii_lowercase();
                    f.find(&fold_buf).is_some()
                };
                if !hit {
                    line_no += 1;
                    continue;
                }
            } else if let Some(ac) = prefilter.as_ref() {
                // Byte prefilter: lines without any positive term cannot match.
                if !ac.is_match(bytes) {
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
        if job_stale(&gen_shared, gen, &cancel) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a search worker to completion and collect hits in order.
    fn run_worker(
        data: &[u8],
        query: &str,
        regex_on: bool,
        case_sensitive: bool,
        scope: Option<(u64, u64)>,
    ) -> Vec<Hit> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.log");
        std::fs::write(&path, data).unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_search(
            SearchJobParams {
                path,
                gen: 1,
                gen_shared: Arc::new(AtomicU64::new(1)),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            query.to_string(),
            regex_on,
            case_sensitive,
            scope,
            None,
        );
        let mut out = Vec::new();
        for msg in rx {
            assert!(msg.error.is_none(), "worker error: {:?}", msg.error);
            out.extend(msg.batch);
            if msg.done {
                break;
            }
        }
        out
    }

    /// Independent oracle: literal occurrences with (line, byte, col).
    fn oracle_literal(data: &[u8], needle: &[u8], case_sensitive: bool) -> Vec<(u64, u64, u32, u32)> {
        let (hay, nd): (Vec<u8>, Vec<u8>) = if case_sensitive {
            (data.to_vec(), needle.to_vec())
        } else {
            (data.to_ascii_lowercase(), needle.to_ascii_lowercase())
        };
        let mut out = Vec::new();
        let mut line: u64 = 1;
        let mut line_start = 0usize;
        let mut i = 0usize;
        while i + nd.len() <= hay.len() {
            if &hay[i..i + nd.len()] == nd.as_slice() {
                out.push((line, line_start as u64, (i - line_start) as u32, (i - line_start + nd.len()) as u32));
                i += nd.len().max(1);
            } else {
                if hay[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
        }
        out
    }

    /// ~10 MB across 4 MiB chunk boundaries, mixed case, multi-hit lines.
    fn worker_data() -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..200_000u64 {
            let line = match i % 7 {
                0 => format!("2026-09-04 ERROR disk id={} ERROR twice\n", i),
                1 => format!("2026-09-04 error lower id={}\n", i),
                2 => format!("2026-09-04 WARN id={}\n", i),
                _ => format!("2026-09-04 INFO id={} pad-pad-pad-pad-pad-pad\n", i),
            };
            v.extend_from_slice(line.as_bytes());
        }
        assert!(v.len() > 2 * search::SEARCH_CHUNK);
        v
    }

    #[test]
    fn worker_literal_matches_oracle_case_sensitive() {
        let data = worker_data();
        let got = run_worker(&data, "ERROR", false, true, None);
        let want = oracle_literal(&data, b"ERROR", true);
        assert_eq!(got.len(), want.len(), "hit count differs");
        // Contract: exact line + exact absolute span. `byte` alone may be
        // combined-anchored for continuation lines (see mapping above).
        for (h, (line, ls, cs, ce)) in got.iter().zip(want.iter()) {
            assert_eq!(
                (h.line, h.byte + h.col_start as u64, h.byte + h.col_end as u64),
                (*line, ls + *cs as u64, ls + *ce as u64)
            );
        }
        // Multi-hit line present (two ERRORs on one line).
        assert!(got.windows(2).any(|w| w[0].line == w[1].line));
    }

    #[test]
    fn worker_literal_matches_oracle_insensitive() {
        let data = worker_data();
        let got = run_worker(&data, "error", false, false, None);
        let want = oracle_literal(&data, b"error", false);
        assert_eq!(got.len(), want.len(), "hit count differs");
        // Contract: exact line + exact absolute span. `byte` alone may be
        // combined-anchored for continuation lines (see mapping above).
        for (h, (line, ls, cs, ce)) in got.iter().zip(want.iter()) {
            assert_eq!(
                (h.line, h.byte + h.col_start as u64, h.byte + h.col_end as u64),
                (*line, ls + *cs as u64, ls + *ce as u64)
            );
        }
        assert!(got.len() > run_worker(&data, "ERROR", false, true, None).len());
    }

    #[test]
    fn worker_regex_matches_literal_union() {
        let data = worker_data();
        let got = run_worker(&data, "WARN|ERROR", true, true, None);
        let mut want = oracle_literal(&data, b"WARN", true);
        want.extend(oracle_literal(&data, b"ERROR", true));
        want.sort();
        want.dedup();
        assert_eq!(got.len(), want.len(), "hit count differs");
        // Contract: exact line + exact absolute span. `byte` alone may be
        // combined-anchored for continuation lines (see mapping above).
        for (h, (line, ls, cs, ce)) in got.iter().zip(want.iter()) {
            assert_eq!(
                (h.line, h.byte + h.col_start as u64, h.byte + h.col_end as u64),
                (*line, ls + *cs as u64, ls + *ce as u64)
            );
        }
    }

    /// Matches at exact chunk edges: fully inside the carried tail (must
    /// not duplicate) and straddling the boundary (must not miss).
    /// Chunk 0 = [0, C), carry = last `overlap` bytes.
    #[test]
    fn worker_chunk_boundary_no_dup_no_miss() {
        let c = search::SEARCH_CHUNK;
        let mut v = vec![b'A'; c - 7];
        v.extend_from_slice(b"ERROR"); // [c-7, c-2): tail, partial in carry
        v.extend_from_slice(b"ER"); // [c-2, c)
        v.extend_from_slice(b"ROR"); // [c, c+3): [c-2, c+3) == "ERROR" crossing
        v.extend_from_slice(b"TAIL"); // [c+3, c+7)
        v.extend_from_slice(&vec![b'B'; c]); // push well into chunk 1
        v.extend_from_slice(b"\nINFO ok\n");
        // Second trap: match ending exactly at the boundary: [c-5, c)
        // lies fully in chunk 0 tail and chunk 1 carry (maximal overlap).
        let mut v2 = vec![b'C'; c - 5];
        v2.extend_from_slice(b"ERROR"); // absolute [c-5, c): ends at boundary
        v2.extend_from_slice(&v[c..]);
        for (name, data) in [("cross", v.as_slice()), ("dup-trap", v2.as_slice())] {
            let got = run_worker(data, "ERROR", false, true, None);
            let want = oracle_literal(data, b"ERROR", true);
            assert_eq!(got.len(), want.len(), "{}: hit count differs", name);
            for (h, (line, ls, cs, ce)) in got.iter().zip(want.iter()) {
                assert_eq!(
                    (h.line, h.byte + h.col_start as u64, h.byte + h.col_end as u64),
                    (*line, ls + *cs as u64, ls + *ce as u64),
                    "{}",
                    name
                );
            }
            // Same bytes through the regex path must agree exactly.
            let rgot = run_worker(data, "ERROR", true, true, None);
            assert_eq!(rgot.len(), got.len(), "{}: regex/literal differ", name);
            for (r, h) in rgot.iter().zip(got.iter()) {
                assert_eq!(
                    (r.line, r.byte + r.col_start as u64, r.byte + r.col_end as u64),
                    (h.line, h.byte + h.col_start as u64, h.byte + h.col_end as u64),
                    "{}",
                    name
                );
            }
        }
    }

    /// Drive the boolean worker to completion.
    fn run_bool_worker(data: &[u8], query: &str, case_sensitive: bool) -> Vec<Hit> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.log");
        std::fs::write(&path, data).unwrap();
        run_bool_file(&path, query, case_sensitive)
    }

    fn run_bool_file(path: &std::path::Path, query: &str, case_sensitive: bool) -> Vec<Hit> {
        use crate::engine::query;
        let ast = query::parse_query(query).unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_bool_search(
            SearchJobParams {
                path: path.to_path_buf(),
                gen: 1,
                gen_shared: Arc::new(AtomicU64::new(1)),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            ast,
            Encoding::Utf8,
            0,
            case_sensitive,
            None,
            None,
        );
        let mut out = Vec::new();
        for msg in rx {
            assert!(msg.error.is_none(), "worker error: {:?}", msg.error);
            out.extend(msg.batch);
            if msg.done {
                break;
            }
        }
        out
    }

    /// Independent oracle: decode every line, full AST eval.
    fn oracle_bool(data: &[u8], query: &str, case_sensitive: bool) -> Vec<(u64, u64, u32, u32)> {
        use crate::engine::{decode, query};
        let ast = query::parse_query(query).unwrap();
        let terms: Vec<String> = ast.positive_terms().into_iter().map(|s| s.to_string()).collect();
        let refs: Vec<&str> = terms.iter().map(|s| s.as_str()).collect();
        let mut out = Vec::new();
        let mut start = 0usize;
        // Trailing segment without '\n' counts as a line (read_until parity).
        let mut spans: Vec<(usize, usize)> = memchr::memchr_iter(b'\n', data)
            .map(|i| {
                let s = (start, i);
                start = i + 1;
                s
            })
            .collect();
        if start < data.len() {
            spans.push((start, data.len()));
        }
        for (idx, (s, e)) in spans.iter().enumerate() {
            let (s, e) = (*s, *e);
            let line = idx as u64 + 1;
            let mut lb = &data[s..e];
            if !lb.is_empty() && lb[lb.len() - 1] == b'\r' {
                lb = &lb[..lb.len() - 1];
            }
            let text = decode::decode_bytes(lb, Encoding::Utf8);
            if ast.matches(&text, case_sensitive) {
                let (cs, ce) = query::first_match_span(&text, &refs, case_sensitive);
                out.push((line, s as u64, cs, ce));
            }
        }
        out
    }

    #[test]
    fn worker_bool_matches_oracle_prefiltered_and_fallback() {
        // Small deterministic data (ASCII, \n endings).
        let mut data = Vec::new();
        for i in 0..5000u64 {
            let line = match i % 6 {
                0 => format!("ERROR timeout id={}\n", i),
                1 => format!("error id={}\n", i),
                2 => format!("WARN timeout id={}\n", i),
                3 => format!("DEBUG noise id={}\n", i),
                _ => format!("INFO ok id={}\n", i),
            };
            data.extend_from_slice(line.as_bytes());
        }
        // Safe query (prefilter on), required-only query, and fully
        // unprefilterable query (union unsafe + required empty).
        for (q, cs) in [
            ("error timeout", false),
            ("ERROR -DEBUG", true),
            ("ERROR OR -DEBUG", true),
        ] {
            let got = run_bool_worker(&data, q, cs);
            let want = oracle_bool(&data, q, cs);
            assert_eq!(got.len(), want.len(), "{}: hit count differs", q);
            for (h, (line, byte, csa, cea)) in got.iter().zip(want.iter()) {
                assert_eq!(
                    (h.line, h.byte, h.col_start, h.col_end),
                    (*line, *byte, *csa, *cea),
                    "{}",
                    q
                );
            }
        }
        // Prefilter actually engaged on the safe query: recompute manually is
        // covered by oracle equality; hits must be non-trivial.
        assert!(!oracle_bool(&data, "error timeout", false).is_empty());
    }

    /// Heavy local-only benchmark (ignored in CI):
    /// `cargo test --release --lib -- --ignored bench_1gb_bool --nocapture`.
    /// Compares a selective positive query (prefilter on) against a common
    /// one (most lines decode anyway) over ~1 GB.
    #[test]
    #[ignore]
    fn bench_1gb_bool() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bool-1gb.log");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
            let mut written: u64 = 0;
            let mut i: u64 = 0;
            while written < 1024 * 1024 * 1024 {
                let line = if i.is_multiple_of(20_000) {
                    format!("2026-09-04 OutOfMemoryError id={} heap exhausted\n", i)
                } else if i.is_multiple_of(50) {
                    format!("2026-09-04 ERROR timeout id={}\n", i)
                } else {
                    format!("2026-09-04 INFO ok id={} user=andi status=OK pad-pad\n", i)
                };
                f.write_all(line.as_bytes()).unwrap();
                written += line.len() as u64;
                i += 1;
            }
            f.flush().unwrap();
        }
        for q in ["OutOfMemoryError", "ERROR INFO"] {
            let t = Instant::now();
            let hits = run_bool_file(&path, q, false);
            eprintln!(
                "[bench-1gb-bool] {:?}: {:.2}s, {} hits",
                q,
                t.elapsed().as_secs_f64(),
                hits.len()
            );
        }
    }

    #[test]
    fn worker_scope_filters_by_match_byte() {
        let data = worker_data();
        let all = run_worker(&data, "ERROR", false, true, None);
        assert!(!all.is_empty());
        // Scope = byte interval covering the middle third of hits.
        let ss = all[all.len() / 3].byte;
        let ee = all[2 * all.len() / 3].byte + 1;
        let got = run_worker(&data, "ERROR", false, true, Some((ss, ee)));
        let want: Vec<&Hit> = all
            .iter()
            .filter(|h| {
                let gp = h.byte + h.col_start as u64;
                gp >= ss && gp < ee
            })
            .collect();
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want.iter()) {
            assert_eq!((g.line, g.byte), (w.line, w.byte));
        }
    }
}
