// English comments: Multi-log merge timeline + multi-file search (analyzer).
// Bounded by design: merge view/export scan at most `scan_cap` lines per
// file into a sorted Vec (no external sort yet — documented in UI). Enough
// for incident timelines; full-file merge is future work.

use crate::engine::Doc;

/// One merged timeline row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedRow {
    /// Unix seconds when the line carries a timestamp; None sorts last.
    pub ts: Option<i64>,
    pub ts_raw: String,
    pub file: String,
    pub line: u64,
    pub text: String,
}

/// Per-file coverage for skew report.
#[derive(Clone, Debug)]
pub struct MergeSource {
    pub file: String,
    pub lines_scanned: u64,
    pub rows_kept: usize,
    pub min_ts: Option<i64>,
    pub min_raw: String,
    pub max_ts: Option<i64>,
    pub max_raw: String,
    pub ts_count: usize,
}

/// Collect timestamped rows from one Doc (bounded scan, head-decode only).
pub fn collect_rows(
    doc: &mut Doc,
    scan_cap: u64,
    head_bytes: usize,
) -> (Vec<MergedRow>, MergeSource) {
    let total = if doc.index.complete {
        doc.index.total_lines
    } else {
        doc.line_count_estimate()
    };
    let cap = total.min(scan_cap);
    let file = doc.file_name.clone();
    let mut rows = Vec::new();
    let mut src = MergeSource {
        file: file.clone(),
        lines_scanned: cap,
        rows_kept: 0,
        min_ts: None,
        min_raw: String::new(),
        max_ts: None,
        max_raw: String::new(),
        ts_count: 0,
    };
    for ln in 1..=cap {
        let Some((head, _, _)) = doc.get_line_head(ln, head_bytes) else {
            continue;
        };
        // Timestamp: auto-parse first (JSON/SQL/generic), else raw prefix.
        let (ts, raw) = if let Some(r) = crate::engine::parser::auto_parse(&head) {
            (r.ts, r.ts_raw.unwrap_or_default())
        } else {
            (Doc::parse_timestamp_prefix(head.trim()), String::new())
        };
        if let Some(t) = ts {
            src.ts_count += 1;
            if src.min_ts.map(|m| t < m).unwrap_or(true) {
                src.min_ts = Some(t);
                src.min_raw = raw.clone();
            }
            if src.max_ts.map(|m| t > m).unwrap_or(true) {
                src.max_ts = Some(t);
                src.max_raw = raw.clone();
            }
        }
        rows.push(MergedRow {
            ts,
            ts_raw: raw,
            file: file.clone(),
            line: ln,
            text: head,
        });
    }
    src.rows_kept = rows.len();
    (rows, src)
}

/// Sort rows into one timeline: timestamped first (asc), then dateless
/// (stable by file, line).
pub fn sort_timeline(mut rows: Vec<MergedRow>) -> Vec<MergedRow> {
    rows.sort_by(|a, b| match (a.ts, b.ts) {
        (Some(x), Some(y)) => x
            .cmp(&y)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a
            .file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line)),
    });
    rows
}

/// Human skew report (Indonesian): overlap window + warnings.
pub fn skew_report(srcs: &[MergeSource]) -> String {
    if srcs.is_empty() {
        return String::from("Tidak ada sumber.");
    }
    let mut out = String::new();
    for s in srcs {
        out.push_str(&format!(
            "• {} — pindai {} baris, {} ber-cap waktu",
            s.file,
            crate::engine::format_count(s.lines_scanned),
            crate::engine::format_count(s.ts_count as u64),
        ));
        match (s.min_ts, s.max_ts) {
            (Some(_), Some(_)) => {
                out.push_str(&format!(" · {} … {}", s.min_raw, s.max_raw));
            }
            _ => out.push_str(" · tanpa cap waktu terbaca"),
        }
        out.push('\n');
    }
    let stamped: Vec<&MergeSource> = srcs.iter().filter(|s| s.min_ts.is_some()).collect();
    if stamped.len() >= 2 {
        let omin = stamped.iter().map(|s| s.min_ts.unwrap()).max().unwrap();
        let omax = stamped.iter().map(|s| s.max_ts.unwrap()).min().unwrap();
        if omin <= omax {
            out.push_str(&format!(
                "Irisan waktu semua file: {} s ({} rentang).",
                crate::engine::format_count((omax - omin).max(0) as u64),
                stamped.len()
            ));
        } else {
            out.push_str(&format!(
                "PERINGATAN skew: rentang waktu tidak beririsan (gap {} s). \
                 Periksa zona waktu / jam server antar sumber.",
                crate::engine::format_count((omin - omax) as u64)
            ));
        }
        // Pairwise offset hint: earliest vs latest start.
        let starts: Vec<i64> = stamped.iter().map(|s| s.min_ts.unwrap()).collect();
        let span = starts.iter().max().unwrap() - starts.iter().min().unwrap();
        if span > 3600 {
            out.push_str(&format!(
                " (skew awal {} s, >1 jam — curigai beda timezone).",
                crate::engine::format_count(span as u64)
            ));
        }
        out.push('\n');
    } else if stamped.len() == 1 {
        out.push_str("Hanya 1 file ber-cap waktu — gabungan diurutkan apa adanya.\n");
    } else {
        out.push_str("Tanpa cap waktu: gabungan diurut per file/baris.\n");
    }
    out
}

/// Multi-file search: same pattern over every open Doc's bytes.
/// Returns per-file (hits, truncated) or regex error. Bounded per file.
pub fn search_all(
    docs: &[&Doc],
    pattern: &str,
    regex: bool,
    case_sensitive: bool,
    per_file_cap: usize,
) -> Result<Vec<(usize, bool)>, String> {
    let mut out = Vec::new();
    for d in docs {
        let data = d.data();
        if regex {
            let (hits, trunc) =
                crate::engine::search::find_regex(data, pattern, case_sensitive, per_file_cap)?;
            out.push((hits.len(), trunc));
        } else {
            let (hits, trunc) = crate::engine::search::find_literal(
                data,
                pattern.as_bytes(),
                case_sensitive,
                per_file_cap,
            );
            out.push((hits.len(), trunc));
        }
    }
    Ok(out)
}

/// Stream merged rows to a text file (`ts | file:line | text`).
pub fn export_merge(path: &std::path::Path, rows: &[MergedRow]) -> Result<usize, String> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)
        .map_err(|e| format!("Gagal membuat file gabungan: {}", e))?;
    let mut n = 0usize;
    for r in rows {
        let ts = if r.ts_raw.is_empty() { "-" } else { &r.ts_raw };
        writeln!(f, "{} | {}:{} | {}", ts, r.file, r.line, r.text)
            .map_err(|e| format!("Gagal menulis gabungan: {}", e))?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(text: &str) -> Doc {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "asislog-merge-test-{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::write(&p, text).unwrap();
        let mut d = Doc::open(p.clone()).unwrap();
        let full = crate::engine::index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(d.sidecar_path());
        d
    }

    #[test]
    fn merge_orders_by_time() {
        let mut a = make_doc("2026-09-04 10:00:03 INFO a3\n2026-09-04 10:00:01 INFO a1\n");
        let mut b = make_doc("2026-09-04 10:00:02 ERROR b2\nno timestamp here\n");
        let (ra, _) = collect_rows(&mut a, 10_000, 512);
        let (rb, _) = collect_rows(&mut b, 10_000, 512);
        let mut all = ra;
        all.extend(rb);
        let sorted = sort_timeline(all);
        assert_eq!(sorted[0].text, "2026-09-04 10:00:01 INFO a1");
        assert_eq!(sorted[1].text, "2026-09-04 10:00:02 ERROR b2");
        assert_eq!(sorted[2].text, "2026-09-04 10:00:03 INFO a3");
        // Dateless sorts last.
        assert!(sorted[3].ts.is_none());
    }

    #[test]
    fn skew_warns_disjoint() {
        let mut a = make_doc("2026-09-04 10:00:01 INFO a\n");
        let mut b = make_doc("2026-09-05 10:00:01 INFO b\n");
        let (_, sa) = collect_rows(&mut a, 10_000, 512);
        let (_, sb) = collect_rows(&mut b, 10_000, 512);
        let rep = skew_report(&[sa, sb]);
        assert!(rep.contains("PERINGATAN"), "got: {}", rep);
    }

    #[test]
    fn multi_search_counts() {
        let a = make_doc("ERROR one\nINFO ok\n");
        let b = make_doc("INFO ok\nINFO ok\n");
        let docs = vec![&a, &b];
        let res = search_all(&docs, "ERROR", false, false, 1000).unwrap();
        assert_eq!(res[0].0, 1);
        assert_eq!(res[1].0, 0);
        assert!(search_all(&docs, "([a", true, false, 1000).is_err());
    }
}
