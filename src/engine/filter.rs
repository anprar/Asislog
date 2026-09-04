// English comments: filter with include tokens, `-exclude` tokens,
// and JSON field predicates (`level=ERROR`, `-level=DEBUG`).

use super::decode::Encoding;
use super::decode::decode_bytes;
use super::jsonlog::field_equals;

/// Parsed filter query.
#[derive(Clone, Debug, Default)]
pub struct ParsedFilter {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    /// (key, value) predicates for JSON lines; case-insensitive compare.
    pub fields: Vec<(String, String)>,
    pub not_fields: Vec<(String, String)>,
    pub case_sensitive: bool,
    pub raw: String,
}

impl ParsedFilter {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.exclude.is_empty()
            && self.fields.is_empty()
            && self.not_fields.is_empty()
    }
}

/// Split `key=value` at the first `=`; None when no usable key.
fn split_field(tok: &str) -> Option<(String, String)> {
    let eq = tok.find('=')?;
    if eq == 0 {
        return None;
    }
    let (k, v) = tok.split_at(eq);
    let v = &v[1..];
    if v.is_empty() {
        return None;
    }
    Some((k.to_string(), v.to_string()))
}

/// Parse e.g. `ERROR -DEBUG level=ERROR -level=DEBUG` into includes,
/// excludes, and JSON field predicates. A lone `-` is ignored.
pub fn parse_filter(query: &str, case_sensitive: bool) -> ParsedFilter {
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    let mut fields = Vec::new();
    let mut not_fields = Vec::new();
    for tok in query.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('-') {
            if rest.is_empty() {
                continue;
            }
            // `--foo` -> exclude `-foo`? Keep simple: strip one dash.
            if let Some((k, v)) = split_field(rest) {
                not_fields.push((k, v));
            } else {
                exclude.push(if case_sensitive {
                    rest.to_string()
                } else {
                    rest.to_lowercase()
                });
            }
        } else if let Some((k, v)) = split_field(tok) {
            fields.push((k, v));
        } else {
            include.push(if case_sensitive {
                tok.to_string()
            } else {
                tok.to_lowercase()
            });
        }
    }
    ParsedFilter {
        include,
        exclude,
        fields,
        not_fields,
        case_sensitive,
        raw: query.to_string(),
    }
}

/// Test a decoded line against the filter.
/// Semantics: every include token present (AND), no exclude token,
/// every field predicate equal (JSON lines only), no excluded field equal.
pub fn line_matches(line: &str, f: &ParsedFilter) -> bool {
    if f.is_empty() {
        return true;
    }
    let hay: String = if f.case_sensitive {
        line.to_string()
    } else {
        line.to_lowercase()
    };
    for inc in &f.include {
        if !hay.contains(inc) {
            return false;
        }
    }
    for exc in &f.exclude {
        if hay.contains(exc) {
            return false;
        }
    }
    for (k, v) in &f.fields {
        if !field_equals(line, k, v) {
            return false;
        }
    }
    for (k, v) in &f.not_fields {
        if field_equals(line, k, v) {
            return false;
        }
    }
    true
}

/// Test raw line bytes (decodes lossily first; only for visible/filter pass).
pub fn line_bytes_match(line_bytes: &[u8], encoding: Encoding, f: &ParsedFilter) -> bool {
    if f.is_empty() {
        return true;
    }
    // Strip trailing \r for matching.
    let mut b = line_bytes;
    if !b.is_empty() && b[b.len() - 1] == b'\r' {
        b = &b[..b.len() - 1];
    }
    let s = decode_bytes(b, encoding);
    line_matches(&s, f)
}

/// Build filter_map (view-row -> original 1-based line) over decoded lines (tests).
pub fn build_filter_map_str(lines: &[String], f: &ParsedFilter) -> Vec<u64> {
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if line_matches(l, f) {
            out.push((i as u64) + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_token() {
        let f = parse_filter("ERROR -DEBUG", true);
        assert_eq!(f.include, vec!["ERROR"]);
        assert_eq!(f.exclude, vec!["DEBUG"]);
        assert!(line_matches("ERROR boom", &f));
        assert!(!line_matches("ERROR DEBUG noise", &f));
        assert!(!line_matches("INFO ok", &f));
    }

    #[test]
    fn empty_filter_matches_all() {
        let f = parse_filter("   ", true);
        assert!(f.is_empty());
        assert!(line_matches("anything", &f));
    }

    #[test]
    fn case_insensitive_filter() {
        let f = parse_filter("error", false);
        assert!(line_matches("ERROR x", &f));
        let f2 = parse_filter("error", true);
        assert!(!line_matches("ERROR x", &f2));
    }

    #[test]
    fn and_semantics_for_includes() {
        let f = parse_filter("ERROR Order", true);
        assert!(line_matches("ERROR Order failed", &f));
        assert!(!line_matches("ERROR other", &f));
    }

    #[test]
    fn json_field_predicates() {
        let f = parse_filter("level=ERROR -level=DEBUG", false);
        assert_eq!(f.fields, vec![("level".to_string(), "ERROR".to_string())]);
        assert_eq!(f.not_fields, vec![("level".to_string(), "DEBUG".to_string())]);
        assert!(line_matches(r#"{"level":"error","msg":"x"}"#, &f));
        assert!(!line_matches(r#"{"level":"debug","msg":"x"}"#, &f));
        // Non-JSON lines fail include-field predicates, pass exclude-only ones.
        assert!(!line_matches("ERROR boom", &f));
        let g = parse_filter("-level=DEBUG", false);
        assert!(line_matches("plain line", &g));
        assert!(!line_matches(r#"{"level":"DEBUG"}"#, &g));
        // Mixed substring + field.
        let h = parse_filter("timeout level=ERROR", false);
        assert!(line_matches(r#"{"level":"ERROR","msg":"timeout hit"}"#, &h));
        assert!(!line_matches(r#"{"level":"ERROR","msg":"ok"}"#, &h));
    }
}
