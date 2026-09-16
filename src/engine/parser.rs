// English comments: Log auto-parser + custom column parsers (analyzer pack).
// Zero-copy-ish per line: parse only sampled/visible/matched lines, never the
// whole file. No new dependencies (uses the existing `regex` crate).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One named parser: a regex with `(?P<field>...)` captures.
/// Shorthand supported in the wizard (expanded before compile):
/// `{name}` -> `(?P<name>\S+)`, `{name:REGEX}` -> `(?P<name>REGEX)`,
/// `{TS}` -> timestamp alternation, `{LVL}` -> level alternation,
/// `{MSG}` -> `(?P<msg>.*)`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LogParser {
    pub name: String,
    pub pattern: String,
}

impl LogParser {
    pub fn validate(&self) -> Result<Vec<String>, String> {
        if self.name.trim().is_empty() {
            return Err(String::from("Nama parser tidak boleh kosong."));
        }
        let expanded = expand_shorthand(&self.pattern)?;
        let re = regex::Regex::new(&expanded)
            .map_err(|e| format!("Pola parser tidak valid: {}", e))?;
        let names: Vec<String> = re
            .capture_names()
            .flatten()
            .map(|s| s.to_string())
            .collect();
        if names.is_empty() {
            return Err(String::from(
                "Pola harus punya minimal 1 grup bernama (?P<nama>...).",
            ));
        }
        Ok(names)
    }

    pub fn compiled(&self) -> Result<regex::Regex, String> {
        let expanded = expand_shorthand(&self.pattern)?;
        regex::Regex::new(&expanded).map_err(|e| format!("Regex parser gagal: {}", e))
    }

    pub fn field_names(&self) -> Vec<String> {
        self.validate().unwrap_or_default()
    }
}

/// Expand `{...}` shorthand into `(?P<...>)` groups. Braces that are regex
/// quantifiers (`a{2,3}`) are left alone: only `{Name}` / `{Name:...}` /
/// `{TS}` / `{LVL}` / `{MSG}` (letters/underscore first) expand.
pub fn expand_shorthand(pattern: &str) -> Result<String, String> {
    const TS_ALT: &str = r"\d{4}[-/]\d{2}[-/]\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?";
    const LVL_ALT: &str = r"TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|FATAL|CRITICAL";
    let mut out = String::with_capacity(pattern.len() + 32);
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < pattern.len() {
        if bytes[i] == b'{' {
            if let Some(end) = pattern[i..].find('}') {
                let inside = &pattern[i + 1..i + end];
                // Name = before first ':' (strict); pattern part after ':'
                // may hold any regex.
                let name_part = inside.split(':').next().unwrap_or("");
                let name_ok = name_part
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphabetic() || c == '_')
                    .unwrap_or(false)
                    && name_part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_');
                if name_ok {
                    if inside == "TS" {
                        out.push_str(&format!("(?P<ts>{})", TS_ALT));
                    } else if inside == "LVL" {
                        out.push_str(&format!("(?P<level>{})", LVL_ALT));
                    } else if inside == "MSG" {
                        out.push_str("(?P<msg>.*)");
                    } else if let Some(colon) = inside.find(':') {
                        let (nm, rx) = inside.split_at(colon);
                        let rx = &rx[1..];
                        if rx.is_empty() {
                            return Err(format!("Grup '{}' tanpa pola.", inside));
                        }
                        out.push_str(&format!("(?P<{}>{})", nm, rx));
                    } else {
                        out.push_str(&format!("(?P<{}>\\S+)", inside));
                    }
                    i += end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

/// One parsed line: ordered fields + best-effort canonical keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedRecord {
    /// All fields in capture order (key, value).
    pub fields: Vec<(String, String)>,
    pub ts_raw: Option<String>,
    pub ts: Option<i64>,
    pub level: Option<String>,
    pub msg: Option<String>,
}

impl ParsedRecord {
    /// Case-insensitive field lookup; canonical aliases included
    /// (`ts`/`timestamp`/`time`, `level`/`severity`, `msg`/`message`).
    pub fn get(&self, name: &str) -> Option<&str> {
        let lname = name.to_ascii_lowercase();
        for (k, v) in &self.fields {
            if k.eq_ignore_ascii_case(name) {
                return Some(v.as_str());
            }
        }
        // Aliases.
        let canon: &[&str] = match lname.as_str() {
            "ts" | "timestamp" | "time" | "datetime" | "date" => {
                &["ts", "timestamp", "time", "datetime", "date"]
            }
            "level" | "severity" | "lvl" => &["level", "severity", "lvl", "loglevel"],
            "msg" | "message" | "text" => &["msg", "message", "text", "error", "event"],
            _ => return None,
        };
        for want in canon {
            for (k, v) in &self.fields {
                if k.eq_ignore_ascii_case(want) {
                    return Some(v.as_str());
                }
            }
        }
        None
    }

    pub fn to_map(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for (k, v) in &self.fields {
            m.entry(k.to_ascii_lowercase()).or_insert_with(|| v.clone());
        }
        m
    }
}

fn pick_level(fields: &[(String, String)]) -> Option<String> {
    for (k, v) in fields {
        if k.eq_ignore_ascii_case("level")
            || k.eq_ignore_ascii_case("severity")
            || k.eq_ignore_ascii_case("lvl")
            || k.eq_ignore_ascii_case("loglevel")
        {
            return Some(v.clone());
        }
    }
    None
}

fn pick_msg(fields: &[(String, String)]) -> Option<String> {
    for (k, v) in fields {
        if k.eq_ignore_ascii_case("msg")
            || k.eq_ignore_ascii_case("message")
            || k.eq_ignore_ascii_case("text")
        {
            return Some(v.clone());
        }
    }
    None
}

fn pick_ts(fields: &[(String, String)]) -> (Option<String>, Option<i64>) {
    for (k, v) in fields {
        let kl = k.to_ascii_lowercase();
        if ["ts", "timestamp", "time", "datetime", "date", "@timestamp"]
            .contains(&kl.as_str())
        {
            let secs = crate::engine::Doc::parse_timestamp_prefix(v.trim());
            return (Some(v.clone()), secs);
        }
    }
    (None, None)
}

/// Parse with an explicit compiled parser.
pub fn parse_with(re: &regex::Regex, line: &str) -> Option<ParsedRecord> {
    let caps = re.captures(line)?;
    let mut fields = Vec::new();
    for name in re.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            fields.push((name.to_string(), m.as_str().to_string()));
        }
    }
    if fields.is_empty() {
        return None;
    }
    let (ts_raw, ts) = pick_ts(&fields);
    Some(ParsedRecord {
        level: pick_level(&fields),
        msg: pick_msg(&fields),
        fields,
        ts_raw,
        ts,
    })
}

/// Generic fallback: `TS [LEVEL] rest` / `TS LEVEL rest` without any parser.
pub fn parse_generic(line: &str) -> Option<ParsedRecord> {
    let t = line.trim();
    if t.len() < 12 {
        return None;
    }
    // Timestamp must be at offset 0 (or [..] wrapped). Take the shortest
    // valid core (19 chars) plus optional millis, and require a separator
    // right after — otherwise a longer slice would swallow the LEVEL word.
    let (ts_raw, rest) = if let Some(s) = t.strip_prefix('[') {
        let e = s.find(']')?;
        let inside = s[..e].trim();
        crate::engine::Doc::parse_timestamp_prefix(inside)?;
        (inside.to_string(), s[e + 1..].trim_start())
    } else {
        // Byte-safe: timestamps are ASCII; bail on non-boundary.
        if t.len() < 19 || !t.is_char_boundary(19) {
            return None;
        }
        crate::engine::Doc::parse_timestamp_prefix(&t[..19])?;
        let mut end = 19;
        if t.len() > 19 && (t.as_bytes()[19] == b'.' || t.as_bytes()[19] == b',') {
            let mut j = 20;
            while j < t.len() && t.as_bytes()[j].is_ascii_digit() {
                j += 1;
            }
            if j == 20 {
                return None; // dangling '.' / ',' — not a timestamp
            }
            end = j;
        }
        // Boundary: end of line or separator (space/tab/bracket).
        if t.len() > end {
            let c = t.as_bytes()[end];
            if c != b' ' && c != b'\t' && c != b'[' {
                return None;
            }
        }
        (
            t[..end].trim().to_string(),
            t[end..].trim_start_matches([' ', '\t']),
        )
    };
    let ts = crate::engine::Doc::parse_timestamp_prefix(&ts_raw);
    // Optional [LEVEL] or LEVEL token next.
    let mut level: Option<String> = None;
    let mut msg = rest;
    let (tok, after) = if let Some(s) = rest.strip_prefix('[') {
        if let Some(e) = s.find(']') {
            (s[..e].trim().to_string(), s[e + 1..].trim_start())
        } else {
            (String::new(), rest)
        }
    } else {
        match rest.find(char::is_whitespace) {
            Some(p) => (rest[..p].to_string(), rest[p..].trim_start()),
            None => (rest.to_string(), ""),
        }
    };
    let up = tok.to_ascii_uppercase();
    const LVLS: &[&str] = &[
        "TRACE", "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "FATAL", "CRITICAL",
    ];
    if LVLS.contains(&up.as_str()) {
        level = Some(tok);
        msg = after;
    } else if tok.is_empty() {
        msg = after;
    }
    let mut fields = vec![("ts".to_string(), ts_raw.clone())];
    if let Some(l) = &level {
        fields.push(("level".to_string(), l.clone()));
    }
    fields.push(("msg".to_string(), msg.to_string()));
    // key=value pairs inside msg become extra columns (bounded).
    for kv in extract_kv(msg, 8) {
        fields.push(kv);
    }
    Some(ParsedRecord {
        fields,
        ts_raw: Some(ts_raw),
        ts,
        level,
        msg: Some(msg.to_string()),
    })
}

/// Extract `k=v` tokens (bounded count) for extra columns.
fn extract_kv(msg: &str, max: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for tok in msg.split_whitespace() {
        if out.len() >= max {
            break;
        }
        if let Some(eq) = tok.find('=') {
            let (k, v) = tok.split_at(eq);
            let v = &v[1..];
            if !k.is_empty()
                && k.len() <= 32
                && !v.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                let v = v.trim_matches(|c| c == '"' || c == '\'');
                out.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

/// Full auto-parse chain for one line:
/// JSON -> SQL-dump cols -> generic TS/LEVEL -> None.
pub fn auto_parse(line: &str) -> Option<ParsedRecord> {
    if let Some(s) = crate::engine::jsonlog::summarize(line) {
        let (ts_raw, ts) = pick_ts(&s.fields);
        return Some(ParsedRecord {
            level: s.level.clone(),
            msg: s.msg.clone(),
            fields: s.fields,
            ts_raw,
            ts,
        });
    }
    if let Some(c) = crate::engine::sqlcols::parse_sql_cols(line) {
        let ts = crate::engine::Doc::parse_timestamp_prefix(c.timestamp.trim());
        let mut fields = vec![
            ("ts".to_string(), c.timestamp.to_string()),
            ("session".to_string(), c.session.to_string()),
            ("action".to_string(), c.action.to_string()),
            ("query".to_string(), c.query.to_string()),
        ];
        for kv in extract_kv(c.query, 6) {
            fields.push(kv);
        }
        return Some(ParsedRecord {
            level: None,
            msg: Some(c.query.to_string()),
            fields,
            ts_raw: Some(c.timestamp.to_string()),
            ts,
        });
    }
    parse_generic(line)
}

/// Detected dominant format over a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectedKind {
    Json,
    Sql,
    Generic,
    Plain,
}

pub fn detect_format(sample: &[String]) -> (DetectedKind, usize, usize) {
    let mut json = 0usize;
    let mut sql = 0usize;
    let mut generic = 0usize;
    let n = sample.len().max(1);
    for line in sample {
        if crate::engine::jsonlog::summarize(line).is_some() {
            json += 1;
        } else if crate::engine::sqlcols::parse_sql_cols(line).is_some() {
            sql += 1;
        } else if parse_generic(line).is_some() {
            generic += 1;
        }
    }
    let pct = |c: usize| c * 100 / n;
    if json * 2 >= sample.len() && json > 0 {
        (DetectedKind::Json, pct(json), n)
    } else if sql * 2 >= sample.len().max(1) && sql > 0 {
        (DetectedKind::Sql, pct(sql), n)
    } else if generic * 2 >= sample.len().max(1) && generic > 0 {
        (DetectedKind::Generic, pct(generic), n)
    } else {
        (DetectedKind::Plain, pct(generic.max(json).max(sql)), n)
    }
}

/// Built-in parser presets (name, pattern with shorthand).
pub fn builtin_parsers() -> Vec<LogParser> {
    vec![
        LogParser {
            name: "Java / Catalina".to_string(),
            pattern: "{TS} {LVL} {MSG}".to_string(),
        },
        LogParser {
            name: "Bracket [ts] [level]".to_string(),
            pattern: "[{ts:.+?}] [{level:.+?}] {MSG}".to_string(),
        },
        LogParser {
            name: "Apache combined".to_string(),
            pattern: "{host} {ident} {user} [{ts:.+?}] \"{request:.+?}\" {status} {size}".to_string(),
        },
        LogParser {
            name: "Syslog".to_string(),
            pattern: "{TS} {host} {proc} {MSG}".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_expands() {
        let e = expand_shorthand("{TS} {LVL} {MSG}").unwrap();
        assert!(e.contains("(?P<ts>"), "got {}", e);
        assert!(e.contains("(?P<level>"), "got {}", e);
        assert!(e.contains("(?P<msg>.*)"), "got {}", e);
        // Quantifiers untouched.
        let q = expand_shorthand(r"x{2,3}").unwrap();
        assert_eq!(q, r"x{2,3}");
    }

    #[test]
    fn custom_field_shorthand() {
        let e = expand_shorthand("{host} [{code:\\d+}]").unwrap();
        assert!(e.contains(r"(?P<host>\S+)"), "got {}", e);
        assert!(e.contains(r"(?P<code>\d+)"), "got {}", e);
    }

    #[test]
    fn validate_requires_named_group() {
        let bad = LogParser { name: "x".into(), pattern: "ERROR.*".into() };
        assert!(bad.validate().is_err());
        let ok = LogParser { name: "x".into(), pattern: "{TS} {LVL} {MSG}".into() };
        let names = ok.validate().unwrap();
        assert!(names.contains(&"ts".to_string()));
        assert!(names.contains(&"level".to_string()));
    }

    #[test]
    fn parse_with_custom() {
        let p = LogParser {
            name: "t".into(),
            pattern: "{TS} {LVL} {MSG}".into(),
        };
        let re = p.compiled().unwrap();
        let r = parse_with(&re, "2026-09-04 10:00:01 ERROR boom id=7").unwrap();
        assert_eq!(r.level.as_deref(), Some("ERROR"));
        assert!(r.ts.is_some());
    }

    #[test]
    fn auto_chain_json_sql_generic() {
        let j = auto_parse(r#"{"level":"ERROR","msg":"x","ts":"2026-09-04 10:00:01"}"#).unwrap();
        assert_eq!(j.level.as_deref(), Some("ERROR"));
        let g = auto_parse("2026-09-04 10:00:01 WARN slow id=3").unwrap();
        assert_eq!(g.level.as_deref(), Some("WARN"));
        assert_eq!(g.get("id"), Some("3"));
        assert!(auto_parse("--- random ---").is_none());
    }

    #[test]
    fn detect_dominant() {
        let sample = vec![
            "2026-09-04 10:00:01 INFO a".to_string(),
            "2026-09-04 10:00:02 ERROR b".to_string(),
            "garbage".to_string(),
        ];
        let (k, _, _) = detect_format(&sample);
        assert_eq!(k, DetectedKind::Generic);
    }
}
