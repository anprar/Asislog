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
                    // search within this scope using owned copy; positions identical
                    // length, so we can proceed with a block:
                    let f = finder.as_ref().unwrap();
                    // line starts relative
                    let mut rel_line = cur_line;
                    let mut line_start_rel = 0usize;
                    // If combined_base > 0 and first byte continues a line, cur_line is
                    // already correct (continuation). Find first newline to set starts.
                    // Iterate matches:
                    for m in f.find_iter(&hay_owned) {
                        if job_stale(&gen_shared, gen, &cancel) {
                            return;
                        }
                        if m < carry_len.saturating_sub(overlap) && global_offset != 0 {
                            // already reported in previous chunk
                            continue;
                        }
                        // count newlines between line_start tracking? Simpler: count
                        // newlines from previous match? For batch sizes this O(n*m)
                        // could be slow; instead precompute line starts once below.
                        let _ = (rel_line, line_start_rel);
                        let _ = m;
                    }
                    // Fall through to unified path below using hay_owned:
                    // To keep code simple, replace combined with lowered copy:
                    drop(combined);
                    combined = hay_owned;
                    // recompute below with case_sensitive=true semantics
                    let _ = &mut rel_line;
                    let _ = &mut line_start_rel;
                    &combined
                };
                // Precompute relative line starts for combined.
                let starts_mid = combined_base > 0;
                let _ = starts_mid;
                let mut rel_starts: Vec<usize> = Vec::new();
                // Determine if combined starts mid-line: check byte before combined_base.
                // We don't have it; approximate: if global_offset != 0 then the first
                // line is a continuation UNLESS carry was empty and previous chunk ended
                // exactly at newline. Track via flag: previous chunk end char.
                // Simplify: if carry_len > 0, first line is continuation (no new start).
                let cont = carry_len > 0;
                if !cont {
                    rel_starts.push(0);
                }
                for i in memchr::memchr_iter(b'\n', hay) {
                    if i + 1 < hay.len() {
                        rel_starts.push(i + 1);
                    }
                }
                let f = finder.as_ref().unwrap();
                for m in f.find_iter(hay) {
                    if job_stale(&gen_shared, gen, &cancel) {
                        return;
                    }
                    if global_offset != 0 && m < carry_len.saturating_sub(overlap) {
                        continue;
                    }
                    // line lookup
                    let li = match rel_starts.binary_search(&m) {
                        Ok(i) => i,
                        Err(i) => {
                            if i == 0 {
                                // inside continuation line
                                // line = cur_line, col = distance to combined start
                                // + need global line start for byte: scan back in file?
                                // Approximate byte as combined_base (line started earlier).
                                // For byte field, find previous newline in hay before m,
                                // else use combined_base.
                                let mut p = m;
                                while p > 0 && hay[p - 1] != b'\n' {
                                    p -= 1;
                                }
                                let gstart = if p == 0 && cont {
                                    // true start is before combined; estimate
                                    combined_base
                                } else {
                                    combined_base + p as u64
                                };
                                let hit = Hit {
                                    line: cur_line,
                                    byte: gstart,
                                    col_start: (m - p) as u32,
                                    col_end: (m - p + needle_cmp.len()) as u32,
                                };
                                if let Some((ss, ee)) = scope {
                                    let gp = combined_base + m as u64;
                                    if gp < ss || gp >= ee {
                                        continue;
                                    }
                                }
                                pending.push(hit.clone());
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
                                }
                                if truncated {
                                    break;
                                }
                                continue;
                            } else {
                                i - 1
                            }
                        }
                    };
                    let ls = rel_starts[li];
                    // When cont, rel_starts[0] is the second line; gline2 adjusts.
                    // cur_line is the line number of combined[0]'s line.
                    let gline2 = if cont { cur_line + li as u64 + 1 } else { cur_line + li as u64 };
                    let hit = Hit {
                        line: gline2,
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
                // advance line_no by newlines in the non-carry part only (avoid double count)
                let new_part = if combined.len() >= carry_len {
                    &combined[carry_len.saturating_sub(overlap.min(carry_len))..]
                } else {
                    &combined[..]
                };
                // Actually simpler: line_no += newlines in tmp (fresh bytes only).
                let _ = new_part;
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
                let cont = carry_len > 0;
                let mut rel_starts: Vec<usize> = Vec::new();
                if !cont {
                    rel_starts.push(0);
                }
                for i in memchr::memchr_iter(b'\n', &combined) {
                    if i + 1 < combined.len() {
                        rel_starts.push(i + 1);
                    }
                }
                for m in re.find_iter(&combined) {
                    if job_stale(&gen_shared, gen, &cancel) {
                        return;
                    }
                    let (s, e) = (m.start(), m.end());
                    if e == s {
                        continue;
                    }
                    if global_offset != 0 && s < carry_len.saturating_sub(overlap) {
                        continue;
                    }
                    if let Some((ss, ee)) = scope {
                        let gp = combined_base + s as u64;
                        if gp < ss || gp >= ee {
                            continue;
                        }
                    }
                    // line lookup
                    let (gline, ls) = match rel_starts.binary_search(&s) {
                        Ok(i) => {
                            let g = if cont { cur_line + i as u64 + 1 } else { cur_line + i as u64 };
                            (g, rel_starts[i])
                        }
                        Err(0) => (cur_line, {
                            let mut p = s;
                            while p > 0 && combined[p - 1] != b'\n' {
                                p -= 1;
                            }
                            p
                        }),
                        Err(i) => {
                            let g = if cont { cur_line + (i - 1) as u64 + 1 } else { cur_line + (i - 1) as u64 };
                            (g, rel_starts[i - 1])
                        }
                    };
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
                let _ = cur_line;
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
        let mut line_no: u64 = 1;
        let mut byte_off: u64 = 0;
        let mut scanned: u64 = 0;
        let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
        let mut total_found: usize = 0;
        let mut truncated = false;
        let mut buf: Vec<u8> = Vec::new();
        // Lewati BOM pada baris pertama.
        let mut first = true;
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
