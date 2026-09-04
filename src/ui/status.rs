// English comments: Indonesian status bar formatting.

use crate::engine::{format_count, format_size, Doc};

/// `Indeks 100% · 48,2 juta baris · 9,7 GB · 234 hasil · Ikuti mati`
pub fn status_text(doc: &Doc, hit_count: usize) -> String {
    let idx = if doc.index.complete {
        String::from("Indeks 100%")
    } else {
        format!("Indeks {}%", (doc.index.progress * 100.0).round() as u32)
    };
    let lines = if doc.index.complete {
        format!("{} baris", format_count(doc.index.total_lines))
    } else {
        format!("~{} baris", format_count(doc.line_count_estimate()))
    };
    let size = format_size(doc.size);
    let hasil = format!("{} hasil", format_count(hit_count as u64));
    let ikuti = if doc.follow { "Ikuti hidup" } else { "Ikuti mati" };
    let enc = doc.encoding().label();
    format!("{} · {} · {} · {} · {} · {}", idx, lines, size, hasil, ikuti, enc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_contains_indonesian() {
        // Smoke: formatting helpers use comma decimals and dot thousands.
        assert_eq!(format_count(1_000_000), "1.000.000");
        assert!(format_size(1024).contains("KB"));
    }
}
