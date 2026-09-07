// English comments: Top-N aggregation for log anomalies (errors & sessions).
// Single pass with strictly bounded RAM (cap 5,000 distinct entries).

use std::collections::HashMap;
use crate::engine::Doc;

/// Aggregated entity with frequency count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopItem {
    pub label: String,
    pub count: usize,
}

/// Compute top N error signatures across sampled/indexed lines with memory guard.
pub fn top_errors(doc: &Doc, limit: usize) -> Vec<TopItem> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let total_lines = if doc.index.complete {
        doc.index.total_lines
    } else {
        doc.line_count_estimate()
    };
    if total_lines == 0 {
        return Vec::new();
    }

    // Step size to keep analysis under ~2 seconds on huge files
    let step = (total_lines / 50_000).max(1);
    let mut cur = 1u64;

    while cur <= total_lines {
        if let Some((text, _, _)) = doc.get_line_head(cur, 200) {
            let upper = text.to_ascii_uppercase();
            if let Some(pos) = upper
                .find("ERROR")
                .or_else(|| upper.find("EXCEPTION"))
                .or_else(|| upper.find("FATAL"))
            {
                let relevant = &text[pos..];
                // Ambil hingga 10 token setelah keyword error
                let sig: String = relevant
                    .split_whitespace()
                    .take(10)
                    .collect::<Vec<_>>()
                    .join(" ");
                if !sig.is_empty() {
                    let entry = counts.entry(sig).or_insert(0);
                    *entry += 1;
                    if counts.len() > 5000 {
                        break; // Memory guard
                    }
                }
            }
        }
        cur += step;
    }

    let mut list: Vec<TopItem> = counts
        .into_iter()
        .map(|(label, count)| TopItem { label, count })
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.count));
    list.truncate(limit);
    list
}

/// Compute top N session IDs / threads across lines.
pub fn top_sessions(doc: &Doc, limit: usize) -> Vec<TopItem> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let total_lines = if doc.index.complete {
        doc.index.total_lines
    } else {
        doc.line_count_estimate()
    };
    if total_lines == 0 {
        return Vec::new();
    }

    let step = (total_lines / 50_000).max(1);
    let mut cur = 1u64;

    while cur <= total_lines {
        if let Some((text, _, _)) = doc.get_line_head(cur, 200) {
            // Find bracketed token e.g. [SESSION-123] or [thread-4]
            if let Some(start) = text.find('[') {
                if let Some(end) = text[start..].find(']') {
                    let inside = &text[start + 1..start + end];
                    if inside.len() >= 3 && inside.len() <= 40 {
                        let entry = counts.entry(inside.to_string()).or_insert(0);
                        *entry += 1;
                        if counts.len() > 5000 {
                            break;
                        }
                    }
                }
            }
        }
        cur += step;
    }

    let mut list: Vec<TopItem> = counts
        .into_iter()
        .map(|(label, count)| TopItem { label, count })
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.count));
    list.truncate(limit);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(text: &str) -> Doc {
        let mut p = std::env::temp_dir();
        p.push(format!("asislog-topn-test-{}.log", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::write(&p, text).unwrap();
        let doc = Doc::open(p.clone()).unwrap();
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(doc.sidecar_path());
        doc
    }

    #[test]
    fn empty_doc_returns_empty() {
        let text = "2026-09-04 10:00:01 INFO all good\n";
        let mut d = make_doc(text);
        let full = crate::engine::index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        assert!(top_errors(&d, 10).is_empty());
    }

    #[test]
    fn aggregates_errors_correctly() {
        let text = "2026-09-04 10:00:01 ERROR NullPointerException in service\n\
                    2026-09-04 10:00:02 INFO ok\n\
                    2026-09-04 10:00:03 ERROR NullPointerException in service\n\
                    2026-09-04 10:00:04 ERROR TimeoutException DB\n";
        let mut d = make_doc(text);
        let full = crate::engine::index::build_full(d.data(), d.encoding(), d.bom_len);
        d.apply_index(full);
        let res = top_errors(&d, 5);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].count, 2);
        assert!(res[0].label.contains("NullPointerException"));
        assert_eq!(res[1].count, 1);
    }
}
