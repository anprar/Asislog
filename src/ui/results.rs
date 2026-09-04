// English comments: results pane helpers.

use crate::engine::format_count;
use crate::ui::viewer::ts_prefix_len;

/// Short preview helper (UI truncates to keep rows cheap).
pub fn preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max_chars).collect();
    s.push('…');
    s
}

/// Row label: `#12  baris 1842291`.
pub fn row_label(idx: usize, hit: &crate::engine::Hit) -> String {
    format!("#{}  baris {}", idx + 1, hit.line)
}

/// Rich single-line result row for scanning:
/// `#12  baris 12.345.667  [2026-08-24 15:48:43] ERROR …teks`.
/// Timestamp (bila ada) dan 90 karakter pertama isi ditampilkan agar
/// konteks (mis. blok SQL multi-baris) bisa dipindai tanpa membuka tiap hit.
pub fn result_row(idx: usize, line: u64, text: &str) -> String {
    let line_s = format_count(line);
    let (ts, body) = if ts_prefix_len(text) == 19 {
        (format!("[{}] ", &text[..19]), text[20.min(text.len())..].trim_start())
    } else {
        (String::new(), text.trim_start())
    };
    let snippet: String = body.chars().take(90).collect();
    let dots = if body.chars().count() > 90 { "…" } else { "" };
    format!("#{}  baris {}  {}{}{}", idx + 1, line_s, ts, snippet, dots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates() {
        assert_eq!(preview("abcdef", 10), "abcdef");
        assert!(preview("abcdefghij12345", 5).ends_with('…'));
    }

    #[test]
    fn rich_row_format() {
        let s = result_row(11, 12_345_667, "2026-08-24 15:48:43 ERROR INSERT INTO T VALUES (1)");
        assert!(s.starts_with("#12  baris 12.345.667  [2026-08-24 15:48:43]"), "got: {}", s);
        assert!(s.contains("ERROR"));
        let plain = result_row(0, 7, "plain line");
        assert_eq!(plain, "#1  baris 7  plain line");
    }
}
