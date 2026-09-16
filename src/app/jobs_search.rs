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
use rayon::prelude::*;

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

/// Wide (UTF-16) decode helper for search: decode raw bytes (no \r strip
/// needed here; callers handle line endings) so literal/regex/boolean scans
/// can run on UTF-8 text like any byte-oriented encoding.
fn wide_decode(bytes: &[u8], encoding: Encoding) -> String {
    crate::engine::decode::decode_bytes(bytes, encoding)
}

/// UTF-16 newline scanner: count wide newlines (0A 00 / 00 0A) in bytes,
/// stopping at the last COMPLETE unit. Returns (count, consumed_pairs).
fn count_nl_wide(bytes: &[u8], le: bool) -> u64 {
    let mut n = 0u64;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        let is_nl = if le {
            bytes[i] == 0x0A && bytes[i + 1] == 0x00
        } else {
            bytes[i] == 0x00 && bytes[i + 1] == 0x0A
        };
        if is_nl {
            n += 1;
        }
        i += 2;
    }
    n
}

/// Byte offset (raw) of the wide newline at/after `from` (even alignment).
fn find_nl_wide(bytes: &[u8], from: usize, le: bool) -> Option<usize> {
    let mut i = from + (from & 1); // snap to even
    while i + 1 < bytes.len() {
        let is_nl = if le {
            bytes[i] == 0x0A && bytes[i + 1] == 0x00
        } else {
            bytes[i] == 0x00 && bytes[i + 1] == 0x0A
        };
        if is_nl {
            return Some(i);
        }
        i += 2;
    }
    None
}

/// C-D2: Ekstrak cabang alternasi literal dari pola regex (contoh: `A|B|C` atau `(A|B|C)`).
/// Mengembalikan `Some(Vec<String>)` jika seluruh cabang adalah literal tanpa metakarakter regex.
pub(crate) fn extract_literal_alternations(pattern: &str) -> Option<Vec<String>> {
    let s = pattern.trim();
    let s = if s.starts_with('(') && s.ends_with(')') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    if !s.contains('|') {
        return None;
    }
    let parts: Vec<&str> = s.split('|').collect();
    if parts.len() < 2 {
        return None;
    }
    let mut literals = Vec::new();
    for part in parts {
        let p = part.trim();
        if p.is_empty() {
            return None;
        }
        if p.chars().any(|c| {
            matches!(
                c,
                '\\' | '^' | '$' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
            )
        }) {
            return None;
        }
        literals.push(p.to_string());
    }
    Some(literals)
}

/// Pindai satu chunk biner utuh menggunakan pencocok (AC, Finder, atau Regex) dan hitung offset baris/kolom.
/// Returns (stored hits, exact in-scope match total): past the display cap
/// the scan continues in count-only mode (no Hit allocation, no line math),
/// so one pass yields both the first-N display set and the exact grand total.
#[allow(clippy::too_many_arguments)]
fn search_chunk_bytes(
    chunk_bytes: &[u8],
    base_byte: u64,
    start_line: u64,
    finder: Option<&memchr::memmem::Finder>,
    needle_len: usize,
    re: Option<&regex::bytes::Regex>,
    ac: Option<&aho_corasick::AhoCorasick>,
    case_sensitive: bool,
    scope: Option<(u64, u64)>,
) -> (Vec<Hit>, u64) {
    let mut hits = Vec::new();
    let mut total: u64 = 0;
    let cap = || search::effective_max_hits();
    if let Some(ac) = ac {
        // C-D2: Jalur super cepat Aho-Corasick untuk alternasi literal
        let mut prev_s = 0usize;
        let mut prev_line = start_line;
        let mut prev_ls = 0usize;
        for m in ac.find_iter(chunk_bytes) {
            let (s, e) = (m.start(), m.end());
            if e == s {
                continue;
            }
            if let Some((ss, ee)) = scope {
                let gp = base_byte + s as u64;
                if gp < ss || gp >= ee {
                    continue;
                }
            }
            total += 1;
            if hits.len() >= cap() {
                continue; // count-only past the cap
            }
            let gap = &chunk_bytes[prev_s..s];
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
            let line_end = chunk_bytes[s..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|k| s + k)
                .unwrap_or(chunk_bytes.len());
            let ce = e.min(line_end).saturating_sub(ls) as u32;
            hits.push(Hit {
                line: gline,
                byte: base_byte + ls as u64,
                col_start: s.saturating_sub(ls) as u32,
                col_end: ce,
            });
        }
    } else if let Some(finder) = finder {
        let hay_owned;
        let hay: &[u8] = if case_sensitive {
            chunk_bytes
        } else {
            hay_owned = chunk_bytes.to_ascii_lowercase();
            &hay_owned
        };
        let mut prev_m = 0usize;
        let mut prev_line = start_line;
        let mut prev_ls = 0usize;
        for m in finder.find_iter(hay) {
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
                byte: base_byte + ls as u64,
                col_start: (m - ls) as u32,
                col_end: (m - ls + needle_len) as u32,
            };
            if let Some((ss, ee)) = scope {
                let gp = base_byte + m as u64;
                if gp < ss || gp >= ee {
                    continue;
                }
            }
            total += 1;
            if hits.len() >= cap() {
                continue; // count-only past the cap
            }
            hits.push(hit);
        }
    } else if let Some(re) = re {
        let mut prev_s = 0usize;
        let mut prev_line = start_line;
        let mut prev_ls = 0usize;
        for m in re.find_iter(chunk_bytes) {
            let (s, e) = (m.start(), m.end());
            if e == s {
                continue;
            }
            if let Some((ss, ee)) = scope {
                let gp = base_byte + s as u64;
                if gp < ss || gp >= ee {
                    continue;
                }
            }
            total += 1;
            if hits.len() >= cap() {
                continue; // count-only past the cap
            }
            let gap = &chunk_bytes[prev_s..s];
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
            let line_end = chunk_bytes[s..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|k| s + k)
                .unwrap_or(chunk_bytes.len());
            let ce = e.min(line_end).saturating_sub(ls) as u32;
            hits.push(Hit {
                line: gline,
                byte: base_byte + ls as u64,
                col_start: s.saturating_sub(ls) as u32,
                col_end: ce,
            });
        }
    }
    (hits, total)
}

/// 8 args (clippy:too_many_arguments allowed): the worker needs the full
/// search context and every caller passes plain values (no builder needed).
/// P0-4: scan satu chunk UTF-16 (berbatas baris penuh). Setiap baris
/// di-decode ke UTF-8 lalu di-scan dengan matcher teks â€” nomor baris dan
/// kolom konsisten dengan jalur byte-oriented.
#[allow(clippy::too_many_arguments)]
fn scan_wide_chunk(
    chunk_bytes: &[u8],
    base_byte: u64,
    start_line: u64,
    encoding: Encoding,
    bom_len: usize,
    first_chunk: bool,
    finder: Option<&memchr::memmem::Finder>,
    needle_len: usize,
    re: Option<&regex::bytes::Regex>,
    ac: Option<&aho_corasick::AhoCorasick>,
    case_sensitive: bool,
    scope: Option<(u64, u64)>,
) -> (Vec<Hit>, u64) {
    let le = encoding == Encoding::Utf16Le;
    let mut hits = Vec::new();
    let mut total: u64 = 0;
    let mut line_no = start_line;
    let mut i = 0usize;
    if first_chunk && bom_len > 0 && chunk_bytes.len() >= bom_len {
        i = bom_len;
    }
    while i < chunk_bytes.len() {
        let line_end = find_nl_wide(chunk_bytes, i, le).unwrap_or(chunk_bytes.len());
        let mut body_end = line_end;
        // Strip wide CR (0D 00 / 00 0D) sebelum newline bila ada.
        if body_end >= 2 && body_end - 2 >= i {
            let (c1, c2) = if le { (0x0Du8, 0x00u8) } else { (0x00u8, 0x0Du8) };
            if chunk_bytes[body_end - 2] == c1 && chunk_bytes[body_end - 1] == c2 {
                body_end -= 2;
            }
        }
        if body_end > i {
            let body = &chunk_bytes[i..body_end];
            let text = crate::engine::decode::decode_bytes(body, encoding);
            let ts: &str = &text;
            let scan_one = |s: usize, e: usize, hits: &mut Vec<Hit>, total: &mut u64| {
                if e == s {
                    return;
                }
                if let Some((ss, ee)) = scope {
                    let gp = base_byte + i as u64 + s as u64;
                    if gp < ss || gp >= ee {
                        return;
                    }
                }
                *total += 1;
                if hits.len() >= search::effective_max_hits() {
                    return; // count-only past the cap
                }
                hits.push(Hit {
                    line: line_no,
                    byte: base_byte + i as u64,
                    col_start: s as u32,
                    col_end: e as u32,
                });
            };
            if let Some(f) = finder {
                if case_sensitive {
                    for m in f.find_iter(ts.as_bytes()) {
                        scan_one(m, m + needle_len, &mut hits, &mut total);
                    }
                } else {
                    // Full-Unicode CI: fold kedua sisi, scan memchr.
                    let hay = ts.to_lowercase();
                    let nl = String::from_utf8_lossy(f.needle()).to_lowercase();
                    let mut from = 0usize;
                    while let Some(rel) = find_sub_ci(&hay, &nl, from) {
                        scan_one(rel, rel + nl.len(), &mut hits, &mut total);
                        from = rel + nl.len().max(1);
                    }
                }
            } else if let Some(re) = re {
                for m in re.find_iter(ts.as_bytes()) {
                    scan_one(m.start(), m.end(), &mut hits, &mut total);
                }
            } else if let Some(ac) = ac {
                for m in ac.find_iter(ts.as_bytes()) {
                    scan_one(m.start(), m.end(), &mut hits, &mut total);
                }
            }
        }
        if line_end + 2 <= chunk_bytes.len() {
            i = line_end + 2;
        } else {
            i = chunk_bytes.len();
        }
        line_no += 1;
    }
    (hits, total)
}

/// Case-insensitive substring find on already-lowercased text (memchr).
fn find_sub_ci(hay_lower: &str, needle_lower: &str, from: usize) -> Option<usize> {
    if needle_lower.is_empty() {
        return None;
    }
    let h = hay_lower.as_bytes();
    let n = needle_lower.as_bytes();
    if from >= h.len() {
        return None;
    }
    memchr::memmem::Finder::new(n).find(&h[from..]).map(|m| from + m)
}

/// True when a stripped wide line still has content (not just padding 00).
fn body_nonempty(lb: &[u8]) -> bool {
    // All-zero body (e.g. from odd padding) decodes to NULs â€” skip those.
    !lb.is_empty() && lb.iter().any(|&b| b != 0)
}

/// Ordered slot helpers for the streaming rayon emission (P0-3).
/// Each slot holds (stored hits, exact chunk match total): the display set
/// stays capped while the grand total stays exact after one pass.
trait HitSlots {
    fn slots_set(&mut self, idx: usize, hits: Vec<Hit>, chunk_total: u64);
    fn slots_ready(&self, idx: usize) -> bool;
    fn slots_take(&mut self, idx: usize) -> (Vec<Hit>, u64);
}

impl HitSlots for Vec<Option<(Vec<Hit>, u64)>> {
    fn slots_set(&mut self, idx: usize, hits: Vec<Hit>, chunk_total: u64) {
        if let Some(slot) = self.get_mut(idx) {
            *slot = Some((hits, chunk_total));
        }
    }

    fn slots_ready(&self, idx: usize) -> bool {
        self.get(idx).map(|s| s.is_some()).unwrap_or(false)
    }

    fn slots_take(&mut self, idx: usize) -> (Vec<Hit>, u64) {
        self.get_mut(idx)
            .and_then(|s| s.take())
            .unwrap_or_default()
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_search(
    params: SearchJobParams,
    query: String,
    regex_on: bool,
    case_sensitive: bool,
    scope: Option<(u64, u64)>,
    // Tail refresh: start scanning at (byte, 1-based line) instead of 0,
    // so follow-append re-scans only new bytes. None = full scan.
    seek_to: Option<(u64, u64)>,
    encoding: Encoding,
    bom_len: usize,
) {    std::thread::spawn(move || {
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
            grand_total: 0,
            });
            return;
        }
        // Compile regex once (bytes).
        // Fast engine first; complex patterns (look-around, backrefs)
        // route to the sequential fancy worker instead of erroring out.
        // (Rebuilds `params` for the handoff; other arms keep ownership.)
        if regex_on {
            match crate::engine::search::compile_regex_auto(&query, case_sensitive) {
                Ok(crate::engine::search::CompiledRegex::Fancy(_)) => {
                    let params = SearchJobParams { path, gen, gen_shared, tx, cancel };
                    spawn_fancy_search(params, query, case_sensitive, scope, seek_to, encoding, bom_len);
                    return;
                }
                Ok(crate::engine::search::CompiledRegex::Fast(_)) => {}
                Err(e) => {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: true,
                        truncated: false,
                        error: Some(format!("Regex tidak valid: {}", e)),
                        scanned: 0,
                        total: 0,
                    grand_total: 0,
                    });
                    return;
                }
            }
        }
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
                    grand_total: 0,
                    });
                    return;
                }
            }
        } else if !case_sensitive {
            // Literal insensitif = regex Unicode-CI atas needle yang
            // di-escape: TANPA salinan lowercase 4 MiB per chunk + fold
            // Unicode penuh (É/É, Cyrillic) — paritas klaim engine.
            // (Dulu: Finder di atas ASCII-fold — salah untuk non-ASCII.)
            let pat = regex::escape(&query);
            let mut b = regex::bytes::RegexBuilder::new(&pat);
            b.case_insensitive(true);
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
                    grand_total: 0,
                    });
                    return;
                }
            }
        } else {
            None
        };
        // Finder byte-exact hanya untuk literal sensitif; semua jalur
        // insensitif lewat cabang regex di atas.
        let needle_cmp: Vec<u8> = if !regex_on && case_sensitive {
            query.as_bytes().to_vec()
        } else {
            Vec::new()
        };
        let finder = if !regex_on && case_sensitive {
            Some(memchr::memmem::Finder::new(&needle_cmp))
        } else {
            None
        };

        // C-D2: Prefilter / matcher Aho-Corasick untuk query regex berpola alternasi literal (A|B|C).
        let pure_literal_ac = if regex_on {
            extract_literal_alternations(&query).and_then(|lits| {
                aho_corasick::AhoCorasick::builder()
                    .ascii_case_insensitive(!case_sensitive)
                    .match_kind(aho_corasick::MatchKind::LeftmostFirst)
                    .build(&lits)
                    .ok()
            })
        } else {
            None
        };

        // C-D1: Paralelisasi Rayon untuk Chunk-Level Search via memory map.
        // Jalur UTF-16 (wide) di-decode per-chunk ke UTF-8 dulu, lalu
        // dipindai dengan matcher teks yang sama (nomor baris dipetakan
        // dari newline wide di chunk sumber).
        if let Ok(m) = crate::engine::mmap::open_mmap(&path) {
            let total_size = m.len() as u64;
            if total_size == 0 {
                let _ = tx.send(SearchBatchMsg {
                    gen,
                    batch: Vec::new(),
                    done: true,
                    truncated: false,
                    error: None,
                    scanned: 0,
                    total: 0,
                grand_total: 0,
                });
                return;
            }

            let (start_offset, initial_line) = match seek_to {
                Some((sb, sl)) if sb < total_size => (sb as usize, sl.max(1)),
                Some(_) => {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: true,
                        truncated: false,
                        error: None,
                        scanned: total_size,
                        total: total_size,
                    grand_total: 0,
                    });
                    return;
                }
                None => (0usize, 1u64),
            };

            let wide = encoding.is_wide();
            let le = encoding == Encoding::Utf16Le;
            let effective_bom = if start_offset == 0 { bom_len } else { 0 };

            // Bangun chunk per BARIS (wide: split di unit newline wide;
            // narrow: extend ke \n berikutnya seperti sebelumnya).
            let slice = &m[start_offset..];
            let chunk_target = search::effective_chunk_bytes();
            let mut chunk_ranges: Vec<(usize, usize)> = Vec::new();
            let mut chunk_start_lines: Vec<u64> = Vec::new();
            {
                let mut pos = 0usize;
                let mut cur_line = initial_line;
                while pos < slice.len() {
                    let mut next = (pos + chunk_target).min(slice.len());
                    if next < slice.len() {
                        if wide {
                            let p = next + (next & 1); // snap even
                            if p + 1 >= slice.len() {
                                next = slice.len();
                            } else {
                                // cari wide newline dari p
                                match find_nl_wide(slice, p, le) {
                                    Some(i) => next = i + 2,
                                    None => next = slice.len(),
                                }
                            }
                        } else if let Some(nl) = memchr::memchr(b'\n', &slice[next..]) {
                            next += nl + 1;
                        } else {
                            next = slice.len();
                        }
                    }
                    // Baris awal chunk = baris pada pos (checkpoint-style
                    // hitung mundur: newline antara posisi-pos sebelumnya
                    // sudah terhitung di iterasi loop ini).
                    chunk_ranges.push((pos, next));
                    chunk_start_lines.push(cur_line);
                    // Hitung newline di chunk baru (wide: unit 2-byte).
                    let nls = if wide {
                        count_nl_wide(&slice[pos..next], le)
                    } else {
                        memchr::memchr_iter(b'\n', &slice[pos..next]).count() as u64
                    };
                    cur_line += nls;
                    pos = next;
                }
            }

            let finder_ref = finder.as_ref();
            let re_ref = re.as_ref();
            let ac_ref = pure_literal_ac.as_ref();
            let needle_len = needle_cmp.len();

            // P0-3: streaming emission â€” setiap chunk yang selesai dikirim
            // segera (batch 500, progres = akhir chunk). Arsitektur: emitter
            // thread membaca slot berurut (prefix-ready) dan mengirim ke
            // channel; par_iter mengisi slot. Tidak ada clone mmap.
            let (ctx_tx, ctx_rx) = std::sync::mpsc::channel::<(usize, Vec<Hit>, u64)>();
            let total_chunks = chunk_ranges.len();
            {
                let n = total_chunks;
                let next_emit = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let results: std::sync::Arc<std::sync::Mutex<Vec<Option<(Vec<Hit>, u64)>>>> =
                    std::sync::Arc::new(std::sync::Mutex::new(
                        (0..n).map(|_| None).collect::<Vec<_>>(),
                    ));
                let stale_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let done_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                // Emitter thread: kirim slot berurut begitu siap.
                {
                    let results = results.clone();
                    let ctx_tx = ctx_tx.clone();
                    let stale_flag = stale_flag.clone();
                    let done_flag = done_flag.clone();
                    let next_emit = next_emit.clone();
                    std::thread::spawn(move || {
                        loop {
                            let cur = next_emit.load(Ordering::Acquire);
                            if cur >= n {
                                break;
                            }
                            let take = {
                                let mut slots = match results.lock() {
                                    Ok(g) => g,
                                    Err(p) => p.into_inner(),
                                };
                                if slots.slots_ready(cur) {
                                    next_emit.fetch_add(1, Ordering::AcqRel);
                                    Some(slots.slots_take(cur))
                                } else {
                                    None
                                }
                            };
                            if let Some((hits, chunk_total)) = take {
                                if ctx_tx.send((cur, hits, chunk_total)).is_err() {
                                    break; // penerima pergi (stale).
                                }
                            } else if done_flag.load(Ordering::Acquire) {
                                // Par_iter selesai tapi slot cur tak terisi
                                // (stale abort): berhenti, jangan hang.
                                break;
                            } else if stale_flag.load(Ordering::Relaxed) {
                                break;
                            } else {
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            }
                        }
                    });
                }
                let chunk_ranges = &chunk_ranges;
                let chunk_start_lines = &chunk_start_lines;
                let finder_ref = finder_ref;
                let re_ref = re_ref;
                let ac_ref = ac_ref;
                let slice = slice;
                (0..n).into_par_iter().for_each(|i| {
                    if job_stale(&gen_shared, gen, &cancel) {
                        stale_flag.store(true, Ordering::Relaxed);
                        return;
                    }
                    if stale_flag.load(Ordering::Relaxed) {
                        return;
                    }
                    let (start, end) = chunk_ranges[i];
                    let st_line = chunk_start_lines[i];
                    let (hits, chunk_total) = if wide {
                        scan_wide_chunk(
                            &slice[start..end],
                            (start_offset + start) as u64,
                            st_line,
                            encoding,
                            effective_bom,
                            i == 0,
                            finder_ref,
                            needle_len,
                            re_ref,
                            ac_ref,
                            case_sensitive,
                            scope,
                        )
                    } else {
                        search_chunk_bytes(
                            &slice[start..end],
                            (start_offset + start) as u64,
                            st_line,
                            finder_ref,
                            needle_len,
                            re_ref,
                            ac_ref,
                            case_sensitive,
                            scope,
                        )
                    };
                    match results.lock() {
                        Ok(mut slots) => slots.slots_set(i, hits, chunk_total),
                        Err(p) => p.into_inner().slots_set(i, hits, chunk_total),
                    }
                });
                done_flag.store(true, Ordering::Release);
            }
            drop(ctx_tx);

            // Emission diurutkan di sini: terima (idx, hits, chunk_total)
            // dan batch-kan. grand_total = jumlah eksak semua chunk (satu
            // pass; chunk yang datang setelah cap global tetap dijumlah).
            let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
            let mut total_found = 0usize;
            let mut grand_total: u64 = 0;
            let mut truncated = false;
            let mut expect_idx = 0usize;
            for (idx, hits, chunk_total) in ctx_rx.iter() {
                if job_stale(&gen_shared, gen, &cancel) {
                    return;
                }
                if idx != expect_idx {
                    // Tak mungkin (emitter berurut); guard tetap.
                    continue;
                }
                expect_idx = idx + 1;
                grand_total += chunk_total;
                let (_, end) = chunk_ranges[idx];
                let scanned_bytes = (start_offset + end) as u64;
                for hit in hits {
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
                            scanned: scanned_bytes,
                            total: total_size,
                            grand_total,
                        });
                    }
                    if total_found >= search::effective_max_hits() {
                        truncated = true;
                        break;
                    }
                }
                if truncated {
                    break;
                }
                // Flush kecil per chunk agar progres terlihat live.
                if !pending.is_empty() {
                    let b = std::mem::take(&mut pending);
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: b,
                        done: false,
                        truncated: false,
                        error: None,
                        scanned: scanned_bytes,
                        total: total_size,
                        grand_total,
                    });
                }
            }
            // Chunks sisa bila truncated menghentikan iterasi lebih awal:
            // drain channel agar thread bantu tidak deadlock — sambil
            // menjumlah total eksak (sudah dihitung tiap chunk).
            if truncated {
                for (_, _, chunk_total) in ctx_rx.iter() {
                    grand_total += chunk_total;
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
                grand_total,
            });
            let _ = total_chunks;
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
                grand_total: 0,
                });
                return;
            }
        };
        let total_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // P0-4: wide (UTF-16) tanpa mmap â€” jalur per-baris (paritas fancy
        // worker, matcher literal/regex tetap cepat per baris).
        if encoding.is_wide() {
            let le = encoding == Encoding::Utf16Le;
            let mut reader = WideLineReader::new(
                std::io::BufReader::with_capacity(1024 * 1024, file),
                le,
            );
            let mut line_no: u64 = 1;
            let mut byte_off: u64 = 0;
            let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
            let mut total_found: usize = 0;
            let mut grand_total: u64 = 0;
            let mut truncated = false;
            let mut buf: Vec<u8> = Vec::new();
            let mut first = true;
            if let Some((sb, sl)) = seek_to {
                if reader.seek_start(sb).is_ok() {
                    byte_off = sb;
                    line_no = sl.max(1);
                    first = sb == 0;
                }
            }
            loop {
                if job_stale(&gen_shared, gen, &cancel) {
                    return;
                }
                buf.clear();
                let n = reader.read_line(&mut buf);
                if n == 0 && buf.is_empty() {
                    break;
                }
                let line_start = byte_off;
                byte_off += if n > 0 { n } else { buf.len() as u64 };
                let mut lb = &buf[..];
                let (n1, n2) = if le { (0x0Au8, 0x00u8) } else { (0x00u8, 0x0Au8) };
                if lb.len() >= 2 && lb[lb.len() - 2] == n1 && lb[lb.len() - 1] == n2 {
                    lb = &lb[..lb.len() - 2];
                    let (c1, c2) = if le { (0x0Du8, 0x00u8) } else { (0x00u8, 0x0Du8) };
                    if lb.len() >= 2 && lb[lb.len() - 2] == c1 && lb[lb.len() - 1] == c2 {
                        lb = &lb[..lb.len() - 2];
                    }
                }
                if first {
                    first = false;
                    if bom_len > 0 && lb.len() >= bom_len {
                        lb = &lb[bom_len..];
                    }
                }
                if body_nonempty(lb) {
                    let text = crate::engine::decode::decode_bytes(lb, encoding);
                    let ts: &str = &text;
                    let add_hit = |s: usize,
                                     e: usize,
                                     pending: &mut Vec<Hit>,
                                     total_found: &mut usize,
                                     grand_total: &mut u64| {
                        if e == s {
                            return;
                        }
                        if let Some((ss, ee)) = scope {
                            let gp = line_start + s as u64;
                            if gp < ss || gp >= ee {
                                return;
                            }
                        }
                        *grand_total += 1;
                        if *total_found >= search::effective_max_hits() {
                            return; // count-only past the cap
                        }
                        pending.push(Hit {
                            line: line_no,
                            byte: line_start,
                            col_start: s as u32,
                            col_end: e as u32,
                        });
                        *total_found += 1;
                    };
                    if let Some(f) = finder.as_ref() {
                        if case_sensitive {
                            for m in f.find_iter(ts.as_bytes()) {
                                add_hit(
                                    m,
                                    m + needle_cmp.len(),
                                    &mut pending,
                                    &mut total_found,
                                    &mut grand_total,
                                );
                            }
                        } else {
                            let hay = ts.to_lowercase();
                            let nl = String::from_utf8_lossy(&needle_cmp).to_lowercase();
                            let mut from = 0usize;
                            while let Some(rel) = find_sub_ci(&hay, &nl, from) {
                                add_hit(
                                    rel,
                                    rel + nl.len(),
                                    &mut pending,
                                    &mut total_found,
                                    &mut grand_total,
                                );
                                from = rel + nl.len().max(1);
                            }
                        }
                    } else if let Some(re) = re.as_ref() {
                        for m in re.find_iter(ts.as_bytes()) {
                            add_hit(
                                m.start(),
                                m.end(),
                                &mut pending,
                                &mut total_found,
                                &mut grand_total,
                            );
                        }
                    } else if let Some(ac) = pure_literal_ac.as_ref() {
                        for m in ac.find_iter(ts.as_bytes()) {
                            add_hit(
                                m.start(),
                                m.end(),
                                &mut pending,
                                &mut total_found,
                                &mut grand_total,
                            );
                        }
                    }
                    if pending.len() >= search::SEARCH_BATCH {
                        let b = std::mem::take(&mut pending);
                        let _ = tx.send(SearchBatchMsg {
                            gen,
                            batch: b,
                            done: false,
                            truncated: false,
                            error: None,
                            scanned: byte_off,
                            total: total_size,
                            grand_total,
                        });
                    }
                    if !truncated && total_found >= search::effective_max_hits() {
                        truncated = true;
                    }
                    // Count-only tail: keep progress alive every 16k lines.
                    if truncated && line_no % 16384 == 0 {
                        let _ = tx.send(SearchBatchMsg {
                            gen,
                            batch: Vec::new(),
                            done: false,
                            truncated: false,
                            error: None,
                            scanned: byte_off,
                            total: total_size,
                            grand_total,
                        });
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
                scanned: total_size,
                total: total_size,
                grand_total,
            });
            return;
        }
        let mut reader = std::io::BufReader::with_capacity(4 * 1024 * 1024, file);
        let chunk_size = search::effective_chunk_bytes();
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
        let mut grand_total: u64 = 0;
        let mut truncated = false;
        // Track whether previous chunk ended mid-line to fix line numbers:
        // line_no always counts lines started. carry holds tail bytes of prev chunk
        // (up to overlap) that may contain a partial line; we handle by scanning
        // combined = carry + chunk for matches but only report matches starting
        // at >= carry_len - overlap_guard? Simplify: report matches in combined
        // whose start >= carry.len() except first chunk, plus handle cross-boundary
        // by overlap = needle.len(). Finder (literal sensitif) butuh
        // overlap sepanjang needle; cabang regex (termasuk
        // literal-insensitif CI) memakai ekor 8 KiB yang sound untuk
        // needle berapa pun (match yang menyentuh byte fresh selalu
        // dilaporkan; yang penuh di carry sudah dilaporkan).
        let overlap = if finder.is_some() {
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
                        grand_total,
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
            // Search in combined. Cabang literal-byte hanya bila Finder
            // ada (literal sensitif); insensitif + regex lewat mesin CI.
            if let Some(f) = finder.as_ref() {
                let hay: &[u8] = &combined;
                // Incremental line mapping: `find_iter` yields matches in
                // ascending order, so line(m) = line(prev) + newlines in
                // hay[prev_m..m]. One memchr pass total per chunk, O(1) per
                // hit â€” no line table, no binary search. Anchored at
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
                    grand_total += 1;
                    if total_found < search::effective_max_hits() {
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
                                grand_total,
                            });
                        }
                    } else if !truncated {
                        truncated = true;
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
                        grand_total,
                    });
                }
                // Count-only tail: no stored hits left, but keep exact
                // progress (one message per chunk is cheap).
                if truncated {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: false,
                        truncated: false,
                        error: None,
                        scanned: global_offset,
                        total: total_size,
                        grand_total,
                    });
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
                    grand_total += 1;
                    if total_found < search::effective_max_hits() {
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
                                grand_total,
                            });
                        }
                    } else if !truncated {
                        truncated = true;
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
                        grand_total,
                    });
                }
                // Count-only tail: keep exact progress (one message/chunk).
                if truncated {
                    let _ = tx.send(SearchBatchMsg {
                        gen,
                        batch: Vec::new(),
                        done: false,
                        truncated: false,
                        error: None,
                        scanned: global_offset,
                        total: total_size,
                        grand_total,
                    });
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
            grand_total,
        });
    });
}

/// Worker regex kompleks (fancy-regex backtracking): pindai sekuensial
/// per baris terdecode, batch 500 hit + progres byte, hormati `search_gen`
/// seperti worker literal. Hanya untuk pola yang ditolak mesin cepat;
/// pola biasa tetap di jalur rayon paralel (jangan perlambat fast path).
/// Mendukung semua encoding termasuk UTF-16 (decode per baris).
pub(crate) fn spawn_fancy_search(
    params: SearchJobParams,
    query: String,
    case_sensitive: bool,
    scope: Option<(u64, u64)>,
    seek_to: Option<(u64, u64)>,
    encoding: Encoding,
    bom_len: usize,
) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let SearchJobParams { path, gen, gen_shared, tx, cancel } = params;
        let mut fb = fancy_regex::RegexBuilder::new(&query);
        fb.case_insensitive(!case_sensitive);
        // Explicit default: pathological lines error instead of hanging.
        fb.backtrack_limit(1_000_000);
        let re = match fb.build() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(SearchBatchMsg {
                    gen,
                    batch: Vec::new(),
                    done: true,
                    truncated: false,
                    error: Some(format!("Regex tidak valid: {}", e)),
                    scanned: 0,
                    total: 0,
                grand_total: 0,
                });
                return;
            }
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
                grand_total: 0,
                });
                return;
            }
        };
        let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
        // P0: required-literal prefilter — skip lines that cannot match
        // before decode + backtracking. Narrow: raw bytes (lossy decode
        // preserves ASCII). Wide: decoded text below (UTF-16 bytes can't
        // carry ASCII literals).
        let wide = encoding.is_wide();
        let pre =
            crate::engine::fancypre::FancyPrefilter::new(&query, case_sensitive);
        let mut line_no: u64 = 1;
        let mut byte_off: u64 = 0;
        let mut scanned: u64 = 0;
        let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
        let mut total_found: usize = 0;
        let mut grand_total: u64 = 0;
        let mut truncated = false;
        let mut buf: Vec<u8> = Vec::new();
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
                        grand_total,
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
            if !bytes.is_empty() && bytes[bytes.len() - 1] == b'\r' && !encoding.is_wide() {
                bytes = &bytes[..bytes.len() - 1];
            }
            if !wide && !pre.passes(bytes) {
                line_no += 1;
                continue;
            }
            // Cakupan byte: samakan dengan jalur cepat (filter by match byte).
            let text = crate::engine::decode::decode_bytes(bytes, encoding);
            if wide && !pre.passes(text.as_bytes()) {
                line_no += 1;
                continue;
            }
            // Bound backtracking work per line (display caps far lower;
            // copy/export stay full; misses past the cap documented).
            let ts: &str = {
                let cap = crate::engine::fancypre::FANCY_LINE_CAP;
                if text.len() > cap {
                    let mut i = cap;
                    while !text.is_char_boundary(i) {
                        i -= 1;
                    }
                    &text[..i]
                } else {
                    &text[..]
                }
            };
            let mut iter = re.find_iter(ts);
            // Backtrack meledak di baris ganas: lewati baris ini,
            // lanjutkan file (pekerja tak boleh hang).
            while let Some(Ok(m)) = iter.next() {
                let (s, e) = (m.start(), m.end());
                if e == s {
                    continue;
                }
                if let Some((ss, ee)) = scope {
                    let gp = line_start + s as u64;
                    if gp < ss || gp >= ee {
                        continue;
                    }
                }
                grand_total += 1;
                if total_found < search::effective_max_hits() {
                    pending.push(Hit {
                        line: line_no,
                        byte: line_start,
                        col_start: s as u32,
                        col_end: e as u32,
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
                            grand_total,
                        });
                    }
                } else if !truncated {
                    truncated = true;
                }
            }
            // Count-only tail: exact total continues, progress stays alive.
            if truncated && line_no % 16384 == 0 {
                let _ = tx.send(SearchBatchMsg {
                    gen,
                    batch: Vec::new(),
                    done: false,
                    truncated: false,
                    error: None,
                    scanned,
                    total,
                    grand_total,
                });
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
            grand_total,
        });
    });
}

/// Worker pencarian boolean: evaluasi AST per baris terdecode.
/// UTF-16 didukung: splitter baris wide menyatukan unit 2-byte
/// (0A 00 / 00 0A) sehingga decode per baris tetap benar.
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
        let wide = encoding.is_wide();
        let le = encoding == Encoding::Utf16Le;
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
                grand_total: 0,
                });
                return;
            }
        };
        let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // Wide: pembelah baris ber-carry (tanpa byte hilang antar-baris).
        // Narrow: BufReader biasa dengan read_until(b'\n').
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
        let terms: Vec<String> = ast
            .positive_terms()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let term_refs: Vec<&str> = terms.iter().map(|s| s.as_str()).collect();
        // Byte prefilter plan (engine/query.rs): union automaton, required
        // Finder, or both â€” chosen so every skipped line provably cannot
        // match. Insensitive queries fold each line for the Finder, so the
        // union (native ASCII case-fold, zero-alloc) rejects first there.
        let plan = ast.prefilter_plan(case_sensitive);
        // ASCII-fold prefilters are EXACT only for ASCII terms: an
        // insensitive "éclair" must NOT be rejected by an ASCII-folded
        // automaton (false negative!). Non-ASCII terms skip the byte
        // prefilter and take the exact Unicode path per line.
        let terms_ascii = terms.iter().all(|t| t.is_ascii());
        let use_union = matches!(
            plan,
            crate::engine::query::PrefilterPlan::UnionThenRequired
                | crate::engine::query::PrefilterPlan::UnionOnly
        ) && (case_sensitive || terms_ascii);
        let use_required = matches!(
            plan,
            crate::engine::query::PrefilterPlan::UnionThenRequired
                | crate::engine::query::PrefilterPlan::RequiredOnly
        );
        // Prefilter berjalan pada teks terdecode (UTF-16 included); pattern
        // byte di-lowercase ASCII untuk Finder tetap valid pada teks UTF-8
        // selama term-nya ASCII (dijamin filter di atas).
        let prefilter: Option<aho_corasick::AhoCorasick> = if use_union && !terms.is_empty() {
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
        // Insensitif: pilih required term ASCII terpanjang (fold ASCII
        // eksak di sana); bila tak ada, prefilter mati — jalur eksak Unicode.
        let required_pat: Option<Vec<u8>> = if use_required {
            ast.required_terms()
                .into_iter()
                .filter(|t| !t.is_empty() && (case_sensitive || t.is_ascii()))
                .max_by_key(|t| t.len())
                .map(|t| {
                    if case_sensitive {
                        t.as_bytes().to_vec()
                    } else {
                        t.as_bytes().to_ascii_lowercase()
                    }
                })
                .filter(|p| !p.is_empty())
        } else {
            None
        };
        let required_finder: Option<memchr::memmem::Finder> =
            required_pat.as_deref().map(memchr::memmem::Finder::new);
        // Scratch fold buffer (reused per line, no realloc churn).
        let mut fold_buf: Vec<u8> = Vec::new();
        let mut line_no: u64 = 1;
        let mut byte_off: u64 = 0;
        let mut scanned: u64 = 0;
        let mut pending: Vec<Hit> = Vec::with_capacity(search::SEARCH_BATCH);
        let mut total_found: usize = 0;
        let mut grand_total: u64 = 0;
        let mut truncated = false;
        let mut buf: Vec<u8> = Vec::new();
        // Lewati BOM pada baris pertama (kecuali seek melewatinya).
        let mut first = true;
        if let Some((sb, sl)) = seek_to {
            use std::io::Seek;
            let seek_ok = if let Some(wr) = wide_reader.as_mut() {
                wr.seek_start(sb).is_ok()
            } else if let Some(nr) = narrow_reader.as_mut() {
                nr.seek(std::io::SeekFrom::Start(sb)).is_ok()
            } else {
                false
            };
            if seek_ok {
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
            if n == 0 {
                // EOF on a trailing partial line: process what's in buf.
                scanned += buf.len() as u64;
            }
            let line_start = byte_off;
            byte_off += if n > 0 { n } else { buf.len() as u64 };
            if n > 0 {
                scanned += n;
            }
            let mut lb = &buf[..];
            if wide {
                // strip wide newline (2 byte) + wide CR bila ada.
                if lb.len() >= 2 {
                    let (n1, n2) = if le { (0x0A, 0x00) } else { (0x00, 0x0A) };
                    if lb[lb.len() - 2] == n1 && lb[lb.len() - 1] == n2 {
                        lb = &lb[..lb.len() - 2];
                        // CR sebelum LF: 0D 00 / 00 0D.
                        let (c1, c2) = if le { (0x0D, 0x00) } else { (0x00, 0x0D) };
                        if lb.len() >= 2 && lb[lb.len() - 2] == c1 && lb[lb.len() - 1] == c2 {
                            lb = &lb[..lb.len() - 2];
                        }
                    }
                }
            } else if lb.ends_with(b"\n") {
                lb = &lb[..lb.len() - 1];
            }
            if first {
                first = false;
                if bom_len > 0 && lb.len() >= bom_len {
                    lb = &lb[bom_len..];
                }
            }
            let mut bytes = lb;
            if !bytes.is_empty() && bytes[bytes.len() - 1] == b'\r' && !wide {
                bytes = &bytes[..bytes.len() - 1];
            }
            // Cakupan baris: lewati decode/match di luar interval.
            if let Some((lo, hi)) = scope_lines {
                if line_no < lo || line_no > hi {
                    line_no += 1;
                    continue;
                }
            }
            // Prefilter hierarchy per plan: union rejects first when present
            // (cheap, zero-alloc), required Finder second; exact path last.
            // UnionThenRequired keeps both sound: union only runs under the
            // safety gate, required is sound for any shape. All prefilters
            // operate on the DECODED text so UTF-16 lines take the same
            // fast path as byte-oriented ones.
            let text;
            let text_ref: &str = if use_union || use_required {
                text = crate::engine::decode::decode_bytes(bytes, encoding);
                if use_union {
                    if let Some(ac) = prefilter.as_ref() {
                        // Byte prefilter: lines without any positive term cannot match.
                        if !ac.is_match(text.as_bytes()) {
                            line_no += 1;
                            continue;
                        }
                    }
                }
                if use_required {
                    if let Some(f) = required_finder.as_ref() {
                        let hit = if case_sensitive {
                            f.find(text.as_bytes()).is_some()
                        } else {
                            fold_buf.clear();
                            fold_buf.extend_from_slice(text.as_bytes());
                            fold_buf.make_ascii_lowercase();
                            f.find(&fold_buf).is_some()
                        };
                        if !hit {
                            line_no += 1;
                            continue;
                        }
                    }
                }
                &text
            } else {
                text = crate::engine::decode::decode_bytes(bytes, encoding);
                &text
            };
            if ast.matches(text_ref, case_sensitive) {
                let (cs, ce) =
                    crate::engine::query::first_match_span(text_ref, &term_refs, case_sensitive);
                grand_total += 1;
                if total_found < search::effective_max_hits() {
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
                            grand_total,
                        });
                    }
                } else if !truncated {
                    truncated = true;
                }
            }
            // Count-only tail: exact total continues, progress stays alive.
            if truncated && line_no % 16384 == 0 {
                let _ = tx.send(SearchBatchMsg {
                    gen,
                    batch: Vec::new(),
                    done: false,
                    truncated: false,
                    error: None,
                    scanned,
                    total,
                    grand_total,
                });
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
            grand_total,
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
        run_worker_full(data, query, regex_on, case_sensitive, scope).0
    }

    /// Full worker result: (stored hits, truncated, exact grand total).
    fn run_worker_full(
        data: &[u8],
        query: &str,
        regex_on: bool,
        case_sensitive: bool,
        scope: Option<(u64, u64)>,
    ) -> (Vec<Hit>, bool, u64) {
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
              Encoding::Utf8,
              0,
          );
          let mut out = Vec::new();
        let mut truncated = false;
        let mut grand_total = 0u64;
        for msg in rx {
            assert!(msg.error.is_none(), "worker error: {:?}", msg.error);
            out.extend(msg.batch);
            grand_total = msg.grand_total;
            if msg.done {
                truncated = msg.truncated;
                break;
            }
        }
        (out, truncated, grand_total)
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
        assert!(v.len() > 2 * search::effective_chunk_bytes());
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
    fn worker_fancy_lookaround_end_to_end() {
        // Routes through spawn_search (compile auto-detects Fancy) and
        // returns exact lines + spans, like the fast path contract.
        let data = b"order 111 approved\norder 222 denied\norder 333 approved\n";
        let got = run_worker(data, r"order \d+(?= approved)", true, true, None);
        let lines: Vec<u64> = got.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![1, 3]);
        // Byte columns point at the match ("order 111" starts col 0).
        assert_eq!((got[0].col_start, got[0].col_end), (0, 9));
        // Backreference pattern also routes to fancy.
        let got2 = run_worker(data, r"(order) \d+ \1", false, false, None);
        assert!(got2.is_empty(), "no repeated word here, just routing check");
        let data2 = b"foo foo\nbar bar\n";
        let got3 = run_worker(data2, r"(\w+) \1", true, true, None);
        assert_eq!(got3.iter().map(|h| h.line).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn worker_chunk_boundary_no_dup_no_miss() {
        let c = search::effective_chunk_bytes();
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

    /// RAII guard: shrink the global max-hits cap for a truncation test,
    /// restore defaults on drop (even on assert panic). Chunk/cache args
    /// stay default (0) so concurrent chunk-size-dependent tests are
    /// unaffected; only sub-10k-match tests run alongside, which never
    /// truncate under either cap.
    struct CapGuard;
    impl CapGuard {
        fn capped() -> Self {
            search::set_search_limits(10_000, 0, 0);
            CapGuard
        }
    }
    impl Drop for CapGuard {
        fn drop(&mut self) {
            search::set_search_limits(0, 0, 0);
        }
    }

    #[test]
    fn worker_grand_total_exact_when_truncated() {
        let _guard = CapGuard::capped();
        assert_eq!(search::effective_max_hits(), 10_000);
        // 30k matches: stored set capped at 10k, grand total exact, one pass.
        let mut data = Vec::new();
        for i in 0..30_000u32 {
            data.extend_from_slice(format!("2026-09-04 ERROR id={:05}\n", i).as_bytes());
        }
        let (hits, truncated, grand) = run_worker_full(&data, "ERROR", false, true, None);
        assert!(truncated, "30k matches must truncate at the 10k cap");
        assert_eq!(hits.len(), 10_000);
        assert_eq!(grand, 30_000, "grand total must be exact, got {}", grand);
        // First-N order preserved (earliest lines stored).
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[9999].line, 10_000);
        // Regex path agrees on the same exact total.
        let (_, t2, g2) = run_worker_full(&data, "ERROR", true, true, None);
        assert!(t2);
        assert_eq!(g2, 30_000);
    }

    /// Drive the boolean worker to completion.
    fn run_bool_worker(data: &[u8], query: &str, case_sensitive: bool) -> Vec<Hit> {        let dir = tempfile::tempdir().unwrap();
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
        // unprefilterable query (union unsafe + required empty). Case
        // variants added for every prefilter plan: RequiredOnly (both
        // modes), UnionOnly, UnionThenRequired (insensitive reorder).
        for (q, cs) in [
            ("error timeout", false),
            ("ERROR -DEBUG", true),
            ("ERROR OR -DEBUG", true),
            ("ERROR timeout", false),
            ("ERROR OR WARN", false),
            ("ERROR OR -DEBUG", false),
            ("OutOfMemoryError heap", false),
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

    #[test]
    fn test_extract_literal_alternations() {
        assert_eq!(
            extract_literal_alternations("WARN|ERROR"),
            Some(vec!["WARN".to_string(), "ERROR".to_string()])
        );
        assert_eq!(
            extract_literal_alternations("(INFO|DEBUG|TRACE)"),
            Some(vec!["INFO".to_string(), "DEBUG".to_string(), "TRACE".to_string()])
        );
        assert_eq!(extract_literal_alternations("foo.*bar"), None);
        assert_eq!(extract_literal_alternations("ERROR"), None);
        assert_eq!(extract_literal_alternations(""), None);
    }

    /// UTF-16LE fixture: manual encode (BOM + interleaved LE units).
    /// encoding_rs's `encode` shape differs; manual is unambiguous.
    fn utf16le_bytes(lines: &[&str]) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE]; // BOM
        for l in lines {
            for unit in format!("{}\n", l).encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
        out
    }

    /// Drive a worker with explicit encoding; returns hits.
    fn run_worker_enc(
        data: &[u8],
        query: &str,
        regex_on: bool,
        case_sensitive: bool,
        encoding: Encoding,
        bom_len: usize,
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
            None,
            None,
            encoding,
            bom_len,
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

    /// P0-4: literal search over UTF-16LE files (mmap path).
    #[test]
    fn worker_utf16_literal_lines_and_cols() {
        let lines = [
            "INFO start",
            "ERROR boom one",
            "INFO mid",
            "ERROR boom two",
            "INFO end",
        ];
        let data = utf16le_bytes(&lines);
        let hits = run_worker_enc(&data, "ERROR", false, true, Encoding::Utf16Le, 2);
        let got: Vec<(u64, u32)> = hits.iter().map(|h| (h.line, h.col_start)).collect();
        assert_eq!(got, vec![(2, 0), (4, 0)], "UTF-16LE literal must find both");
        // Case-insensitive (full Unicode fold).
        let hits = run_worker_enc(&data, "error", false, false, Encoding::Utf16Le, 2);
        assert_eq!(hits.len(), 2);
    }

    /// P0-4: regex + boolean over UTF-16LE.
    #[test]
    fn worker_utf16_regex_and_boolean() {
        let lines = ["WARN one", "ERROR two", "INFO three"];
        let data = utf16le_bytes(&lines);
        let hits = run_worker_enc(&data, "WARN|ERROR", true, true, Encoding::Utf16Le, 2);
        let got: Vec<u64> = hits.iter().map(|h| h.line).collect();
        assert_eq!(got, vec![1, 2]);
        // Boolean via the bool worker.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b16.log");
        std::fs::write(&path, &data).unwrap();
        let ast = crate::engine::query::parse_query("ERROR OR WARN").unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_bool_search(
            SearchJobParams {
                path,
                gen: 1,
                gen_shared: Arc::new(AtomicU64::new(1)),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            ast,
            Encoding::Utf16Le,
            2,
            false,
            None,
            None,
        );
        let mut out = Vec::new();
        for msg in rx {
            assert!(msg.error.is_none(), "bool worker error: {:?}", msg.error);
            out.extend(msg.batch);
            if msg.done {
                break;
            }
        }
        assert_eq!(out.len(), 2, "boolean search must work on UTF-16");
    }

    /// P0-3: streaming emission â€” progress messages arrive before done.
    #[test]
    fn worker_streaming_progress_before_done() {
        // Big-ish data across many chunks: batches + progress must flow
        // while done=false at least once (mmap path, rayon streaming).
        let data = worker_data();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.log");
        std::fs::write(&path, &data).unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_search(
            SearchJobParams {
                path,
                gen: 1,
                gen_shared: Arc::new(AtomicU64::new(1)),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            "ERROR".to_string(),
            false,
            true,
            None,
            None,
            Encoding::Utf8,
            0,
        );
        let mut saw_progress_before_done = false;
        let mut total = 0usize;
        let mut done = false;
        for msg in rx {
            if msg.done {
                done = true;
            } else if !msg.batch.is_empty() || msg.scanned > 0 {
                saw_progress_before_done = true;
            }
            total += msg.batch.len();
            if done {
                break;
            }
        }
        assert!(done);
        assert!(saw_progress_before_done, "streaming must emit before done");
        assert!(total > 0);
    }

    /// Literal-insensitif Unicode end-to-end (jalur rayon mmap, tanpa
    /// salinan lowercase): ÉCLAIR cocok dengan "éclair", tapi "eclair"
    /// polos TIDAK dicocokkan (fold presisi, bukan aproksimasi).
    #[test]
    fn worker_literal_insensitive_full_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("u.log");
        std::fs::write(&path, "w1 ÉCLAIR x\nw2 éclair y\nw3 plain\n".as_bytes()).unwrap();
        let run = |q: &str| {
            let (tx, rx) = mpsc::channel();
            spawn_search(
                SearchJobParams {
                    path: path.clone(),
                    gen: 1,
                    gen_shared: Arc::new(AtomicU64::new(1)),
                    tx,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
                q.to_string(),
                false,
                false,
                None,
                None,
                Encoding::Utf8,
                0,
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
        };
        let hits = run("éclair");
        assert_eq!(hits.len(), 2, "both folded lines must match");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[1].line, 2);
        assert!(run("eclair").is_empty(), "ASCII needle must not match accented text");
    }

    /// Boolean-insensitif non-ASCII: prefilter ASCII tak boleh menggugurkan
    /// baris yang cocok lewat fold Unicode (regresi false-negative).
    #[test]
    fn worker_bool_insensitive_unicode_no_false_negative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ub.log");
        std::fs::write(&path, "w1 ÉCLAIR x\nw2 plain\n".as_bytes()).unwrap();
        let ast = crate::engine::query::parse_query("éclair").unwrap();
        let (tx, rx) = mpsc::channel();
        spawn_bool_search(
            SearchJobParams {
                path,
                gen: 1,
                gen_shared: Arc::new(AtomicU64::new(1)),
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            ast,
            Encoding::Utf8,
            0,
            false,
            None,
            None,
        );
        let mut out = Vec::new();
        for msg in rx {
            assert!(msg.error.is_none(), "bool worker error: {:?}", msg.error);
            out.extend(msg.batch);
            if msg.done {
                break;
            }
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, 1);
    }
}