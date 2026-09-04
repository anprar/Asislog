// English comments: viewport highlight rules (on-screen lines only).
// Colors are theme-aware so light themes stay readable.

/// Fixed row height for the log viewport (matches 14px monospace + padding).
pub const ROW_H: f32 = 20.0;

/// Log level for one visible line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Error,
    Warn,
    Info,
    Normal,
}

/// Classify by substring (case-sensitive, fast path for viewport only).
pub fn classify(line: &str) -> LineKind {
    if line.contains("ERROR")
        || line.contains("FATAL")
        || line.contains("Exception")
        || line.contains("Caused by:")
    {
        LineKind::Error
    } else if line.contains("WARN") || line.contains("WARNING") {
        LineKind::Warn
    } else if line.contains("INFO") {
        LineKind::Info
    } else {
        LineKind::Normal
    }
}

/// egui color for a classified line, theme-aware.
/// `dark == true` follows the dark palettes, otherwise light palettes.
pub fn color_for_theme(kind: LineKind, dark: bool) -> egui::Color32 {
    if dark {
        match kind {
            LineKind::Error => egui::Color32::from_rgb(255, 110, 110),
            LineKind::Warn => egui::Color32::from_rgb(255, 210, 110),
            LineKind::Info => egui::Color32::from_rgb(150, 150, 150),
            LineKind::Normal => egui::Color32::from_rgb(220, 220, 220),
        }
    } else {
        match kind {
            LineKind::Error => egui::Color32::from_rgb(180, 30, 30),
            LineKind::Warn => egui::Color32::from_rgb(150, 100, 0),
            LineKind::Info => egui::Color32::from_rgb(100, 100, 100),
            LineKind::Normal => egui::Color32::from_rgb(30, 30, 30),
        }
    }
}

/// egui color for a classified line. Dark-first palette (legacy default).
pub fn color_for(kind: LineKind) -> egui::Color32 {
    color_for_theme(kind, true)
}

pub fn bg_for_current_match() -> egui::Color32 {
    bg_for_current_match_theme(true)
}

/// Current-match background, theme-aware.
pub fn bg_for_current_match_theme(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(60, 60, 20)
    } else {
        egui::Color32::from_rgb(255, 235, 150)
    }
}

/// Selected-row background, theme-aware.
pub fn bg_for_selection(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(35, 45, 60)
    } else {
        egui::Color32::from_rgb(210, 228, 248)
    }
}

/// Hover-row background (subtle), theme-aware.
pub fn bg_for_hover(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(28, 33, 42)
    } else {
        egui::Color32::from_rgb(233, 239, 246)
    }
}

/// Gutter (line number) color, theme-aware.
pub fn gutter_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::GRAY
    } else {
        egui::Color32::from_rgb(110, 110, 110)
    }
}

/// Dim color for stack-trace continuation lines, theme-aware.
pub fn dim_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(125, 125, 125)
    } else {
        egui::Color32::from_rgb(140, 140, 140)
    }
}

/// Timestamp prefix color (bluish gray), theme-aware.
pub fn time_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(130, 165, 205)
    } else {
        egui::Color32::from_rgb(55, 90, 140)
    }
}

/// True for Java-style stack trace continuation lines (`at com...`, `... 42 more`).
/// Rendered dim so the error line above stays prominent.
pub fn is_stack_trace(line: &str) -> bool {
    let t = line.trim_start_matches([' ', '\t']);
    t.starts_with("at ") || t.starts_with("... ")
}

/// Length of a `YYYY-MM-DD HH:MM:SS`-style timestamp prefix, or 0.
/// Accepts `-`/`/` date separators and ` `/`T` between date and time.
pub fn ts_prefix_len(line: &str) -> usize {
    let b = line.as_bytes();
    if b.len() < 19 {
        return 0;
    }
    let digit = |i: usize| b[i].is_ascii_digit();
    if !(digit(0) && digit(1) && digit(2) && digit(3)) {
        return 0;
    }
    if b[4] != b'-' && b[4] != b'/' {
        return 0;
    }
    if !(digit(5) && digit(6)) {
        return 0;
    }
    if b[7] != b'-' && b[7] != b'/' {
        return 0;
    }
    if !(digit(8) && digit(9)) {
        return 0;
    }
    if b[10] != b' ' && b[10] != b'T' {
        return 0;
    }
    if !(digit(11) && digit(12) && b[13] == b':' && digit(14) && digit(15) && b[16] == b':' && digit(17) && digit(18))
    {
        return 0;
    }
    19
}

/// Keyword spans to highlight inside a visible line: (byte_start, byte_end, kind).
/// Longer keywords first so `WARNING` wins over `WARN`.
pub fn highlight_spans(line: &str) -> Vec<(usize, usize, LineKind)> {
    const TOKENS: &[(&str, LineKind)] = &[
        ("Caused by:", LineKind::Error),
        ("Exception", LineKind::Error),
        ("ERROR", LineKind::Error),
        ("FATAL", LineKind::Error),
        ("WARNING", LineKind::Warn),
        ("WARN", LineKind::Warn),
    ];
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let mut hit: Option<(&str, LineKind)> = None;
        for (tok, kind) in TOKENS {
            if line[i..].starts_with(tok) {
                hit = Some((tok, *kind));
                break;
            }
        }
        if let Some((tok, kind)) = hit {
            out.push((i, i + tok.len(), kind));
            i += tok.len().max(1);
        } else {
            // Advance by one UTF-8 char to keep byte indices valid.
            i += line[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
    }
    out
}

/// One compiled user highlight rule for viewport rendering.
/// Compiled once (app caches by version), evaluated per visible row.
pub struct CompiledRule {
    pub whole_line: bool,
    color_dark: egui::Color32,
    color_light: egui::Color32,
    literal: Option<(String, bool)>,
    regex: Option<regex::Regex>,
}

impl CompiledRule {
    /// Compile from raw fields. None when empty/invalid (caller shows why).
    pub fn compile(
        pattern: &str,
        is_regex: bool,
        case_sensitive: bool,
        color_key: &str,
        whole_line: bool,
    ) -> Option<Self> {
        if pattern.is_empty() {
            return None;
        }
        let (regex, literal) = if is_regex {
            let re = regex::RegexBuilder::new(pattern)
                .case_insensitive(!case_sensitive)
                .build()
                .ok()?;
            (Some(re), None)
        } else {
            (None, Some((pattern.to_string(), case_sensitive)))
        };
        let ((dr, dg, db), (lr, lg, lb)) = crate::store::highlight_palette(color_key);
        Some(Self {
            whole_line,
            color_dark: egui::Color32::from_rgb(dr, dg, db),
            color_light: egui::Color32::from_rgb(lr, lg, lb),
            literal,
            regex,
        })
    }

    pub fn color(&self, dark: bool) -> egui::Color32 {
        if dark {
            self.color_dark
        } else {
            self.color_light
        }
    }

    /// Byte spans of matches in the ORIGINAL text (never panics on slicing:
    /// literal path is ASCII length-preserving, regex yields valid indices).
    pub fn find_spans(&self, text: &str) -> Vec<(usize, usize)> {
        if let Some(re) = &self.regex {
            return re
                .find_iter(text)
                .filter(|m| m.end() > m.start())
                .map(|m| (m.start(), m.end()))
                .collect();
        }
        if let Some((needle, case)) = &self.literal {
            if needle.is_empty() {
                return Vec::new();
            }
            if *case {
                let f = memchr::memmem::Finder::new(needle.as_bytes());
                return f
                    .find_iter(text.as_bytes())
                    .map(|m| (m, m + needle.len()))
                    .collect();
            }
            let h = text.as_bytes().to_ascii_lowercase();
            let n = needle.as_bytes().to_ascii_lowercase();
            let f = memchr::memmem::Finder::new(&n);
            return f.find_iter(&h).map(|m| (m, m + n.len())).collect();
        }
        Vec::new()
    }
}

/// Render one log line with token highlights (visible rows only).
/// Single visual line, no wrap: long lines extend for the horizontal scrollbar.
/// `rules` are user highlight rules (whole-line rules win first).
/// Returns the combined click/hover response of all segments.
pub fn render_log_line(
    ui: &mut egui::Ui,
    text: &str,
    kind: LineKind,
    dark: bool,
    rules: &[CompiledRule],
) -> egui::Response {
    let base = color_for_theme(kind, dark);
    if text.is_empty() {
        return ui.label(" ");
    }
    if is_stack_trace(text) {
        return ui.label(
            egui::RichText::new(text.to_owned())
                .monospace()
                .color(dim_color(dark)),
        );
    }
    let ts = ts_prefix_len(text);
    // Whole-line user rules win over everything (first match).
    for r in rules {
        if r.whole_line && !r.find_spans(text).is_empty() {
            return ui.label(
                egui::RichText::new(text.to_owned())
                    .monospace()
                    .color(r.color(dark)),
            );
        }
    }
    let spans = highlight_spans(if ts > 0 { &text[ts..] } else { text });
    // Merge builtin spans (priority 0) with user match spans (priority 1+).
    // User rules win overlaps; earlier start wins ties.
    let mut all: Vec<(usize, usize, u32, egui::Color32)> = spans
        .into_iter()
        .map(|(s, e, k)| (s + ts, e + ts, 0u32, color_for_theme(k, dark)))
        .collect();
    for (ri, r) in rules.iter().enumerate() {
        if r.whole_line {
            continue;
        }
        for (s, e) in r.find_spans(text) {
            if e <= ts {
                continue; // keep timestamp color
            }
            all.push((s.max(ts), e, 1 + ri as u32, r.color(dark)));
        }
    }
    all.sort_by(|a, b| (a.0, a.2).cmp(&(b.0, b.2)));
    let mut kept: Vec<(usize, usize, u32, egui::Color32)> = Vec::new();
    for (s, e, p, c) in all {
        if let Some(last) = kept.last() {
            if s < last.1 {
                if p > last.2 {
                    kept.pop();
                } else {
                    continue;
                }
            }
        }
        kept.push((s, e, p, c));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut acc: Option<egui::Response> = None;
        let mut push = |ui: &mut egui::Ui, s: &str, c: egui::Color32| {
            let r = ui.label(
                egui::RichText::new(s.to_owned())
                    .monospace()
                    .color(c),
            );
            acc = Some(match acc.take() {
                Some(o) => o.union(r),
                None => r,
            });
        };
        let mut pos = 0;
        if ts > 0 {
            push(ui, &text[..ts], time_color(dark));
            pos = ts;
        }
        for (s, e, _p, c) in kept {
            if s > pos {
                push(ui, &text[pos..s], base);
            }
            push(ui, &text[s..e], c);
            pos = e;
        }
        if pos < text.len() {
            push(ui, &text[pos..], base);
        }
        // `text` non-empty guarantees at least one segment.
        acc.unwrap()
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_rules() {
        assert_eq!(classify("2026 ERROR OrderService boom"), LineKind::Error);
        assert_eq!(classify("Caused by: java.lang.NullPointerException"), LineKind::Error);
        assert_eq!(classify("WARN low disk"), LineKind::Warn);
        assert_eq!(classify("INFO started"), LineKind::Info);
        assert_eq!(classify("plain line"), LineKind::Normal);
    }

    #[test]
    fn theme_colors_differ() {
        assert_ne!(
            color_for_theme(LineKind::Normal, true),
            color_for_theme(LineKind::Normal, false)
        );
        assert_eq!(color_for(LineKind::Error), color_for_theme(LineKind::Error, true));
    }

    #[test]
    fn stack_and_timestamp() {
        assert!(is_stack_trace("    at com.erp.OrderService.run(OrderService.java:42)"));
        assert!(is_stack_trace("... 41 more"));
        assert!(!is_stack_trace("2026-09-03 13:41:02 ERROR boom"));
        assert_eq!(ts_prefix_len("2026-09-03 13:41:02 ERROR boom"), 19);
        assert_eq!(ts_prefix_len("2026/09/03T13:41:02 INFO x"), 19);
        assert_eq!(ts_prefix_len("no timestamp here"), 0);
    }

    #[test]
    fn spans_prefer_longest() {
        let spans = highlight_spans("WARN and WARNING here");
        // First WARN at 0..4, then WARNING at 9..16 (not WARN at 9..13).
        assert!(spans.contains(&(0, 4, LineKind::Warn)));
        assert!(spans.contains(&(9, 16, LineKind::Warn)));
        assert!(!spans.contains(&(9, 13, LineKind::Warn)));
        let e = highlight_spans("NullPointerException boom");
        assert_eq!(e, vec![(11, 20, LineKind::Error)]);
    }

    #[test]
    fn user_rule_spans_ascii_safe() {
        let r = CompiledRule::compile("rollback", false, false, "red", false).unwrap();
        assert_eq!(r.find_spans("ROLLBACK WORK done"), vec![(0, 8)]);
        let cs = CompiledRule::compile("rollback", false, true, "red", false).unwrap();
        assert!(cs.find_spans("ROLLBACK WORK").is_empty());
        let rx = CompiledRule::compile("ERR(O|A)R", true, false, "red", false).unwrap();
        assert_eq!(rx.find_spans("x ERROR y"), vec![(2, 7)]);
        assert!(CompiledRule::compile("", false, false, "red", false).is_none());
        assert!(CompiledRule::compile("([", true, false, "red", false).is_none());
    }
}
