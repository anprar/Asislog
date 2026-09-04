// English comments: JSON log lines -> compact inline summary + field access.
// Detection is per line (mixed files work). Only visible rows are parsed.

/// Flattened view of one JSON log line.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonSummary {
    /// Primary timestamp string if a known key exists.
    pub time: Option<String>,
    /// Primary level string if a known key exists.
    pub level: Option<String>,
    /// Primary message string if a known key exists.
    pub msg: Option<String>,
    /// All top-level scalar fields in order (key, display value).
    pub fields: Vec<(String, String)>,
}

fn scalar_display(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => None,
        // Nested containers stringify compactly (one level only).
        arr @ serde_json::Value::Array(_) | arr @ serde_json::Value::Object(_) => {
            serde_json::to_string(arr).ok()
        }
    }
}

fn pick<'a>(fields: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some((_, v)) = fields.iter().find(|(fk, _)| fk.eq_ignore_ascii_case(k)) {
            return Some(v.as_str());
        }
    }
    None
}

/// Summarize when `line` is a JSON object; None otherwise (incl. arrays).
pub fn summarize(line: &str) -> Option<JsonSummary> {
    let t = line.trim();
    if !(t.starts_with('{') && t.ends_with('}')) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(t).ok()?;
    let obj = v.as_object()?;
    let mut fields = Vec::new();
    for (k, val) in obj {
        if let Some(d) = scalar_display(val) {
            fields.push((k.clone(), d));
        }
    }
    if fields.is_empty() {
        return None;
    }
    let time = pick(&fields, &["timestamp", "time", "ts", "@timestamp", "datetime", "date"])
        .map(|s| s.to_string());
    let level = pick(&fields, &["level", "severity", "lvl", "loglevel"]).map(|s| s.to_string());
    let msg = pick(&fields, &["msg", "message", "text", "error", "event"]).map(|s| s.to_string());
    Some(JsonSummary { time, level, msg, fields })
}

/// Compact inline display: `LEVEL msg k=v k=v` (known keys first, no dupes).
/// Falls back to the original line when not JSON.
pub fn display_text(line: &str) -> String {
    let Some(s) = summarize(line) else {
        return line.to_string();
    };
    let mut out = String::new();
    if let Some(t) = &s.time {
        out.push_str(&format!("[{}] ", truncate(t, 32)));
    }
    if let Some(l) = &s.level {
        out.push_str(&format!("{} ", truncate(l, 12)));
    }
    if let Some(m) = &s.msg {
        out.push_str(&truncate(m, 120));
    }
    // Remaining fields as k=v (skip the three already shown, cap count).
    let mut shown = 0;
    for (k, v) in &s.fields {
        if shown >= 6 {
            out.push_str(" …");
            break;
        }
        let kl = k.to_ascii_lowercase();
        let is_primary = ["timestamp", "time", "ts", "@timestamp", "datetime", "date",
            "level", "severity", "lvl", "loglevel",
            "msg", "message", "text", "error", "event"]
            .contains(&kl.as_str());
        if is_primary {
            continue;
        }
        // If msg was missing, first scalar already shown? No: show all non-primary.
        out.push_str(&format!(" {}={}", truncate(k, 24), truncate(v, 48)));
        shown += 1;
    }
    if out.trim().is_empty() {
        line.to_string()
    } else {
        out
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(max).collect();
        o.push('…');
        o
    }
}

/// Field equality for `level=ERROR` filter tokens (case-insensitive both).
pub fn field_equals(line: &str, key: &str, want: &str) -> bool {
    let Some(s) = summarize(line) else {
        return false;
    };
    s.fields
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case(key) && v.eq_ignore_ascii_case(want))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_and_summarizes() {
        let s = summarize(r#"{"timestamp":"2026-08-24 13:00:01","level":"ERROR","msg":"boom","code":500}"#).unwrap();
        assert_eq!(s.time.as_deref(), Some("2026-08-24 13:00:01"));
        assert_eq!(s.level.as_deref(), Some("ERROR"));
        assert_eq!(s.msg.as_deref(), Some("boom"));
        assert!(summarize("plain line").is_none());
        assert!(summarize("[1,2]").is_none());
        assert!(summarize("{not json").is_none());
    }

    #[test]
    fn inline_layout() {
        let d = display_text(r#"{"level":"WARN","msg":"slow","elapsed_ms":1200,"ok":true}"#);
        assert!(d.starts_with("WARN slow"), "got: {}", d);
        assert!(d.contains("elapsed_ms=1200"));
        // Non-JSON passes through.
        assert_eq!(display_text("abc"), "abc");
    }

    #[test]
    fn field_predicates() {
        let line = r#"{"level":"Error","service":"auth"}"#;
        assert!(field_equals(line, "level", "ERROR"));
        assert!(field_equals(line, "LEVEL", "error"));
        assert!(!field_equals(line, "level", "WARN"));
        assert!(!field_equals(line, "missing", "x"));
        assert!(!field_equals("plain", "level", "ERROR"));
    }
}
