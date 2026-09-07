// English comments: tiny boolean query parser -> AST, evaluated per line.
// Syntax: terms, "quoted phrases", -exclude / NOT x, AND (or whitespace),
// OR / |, parentheses. AND binds tighter than OR. Usable in literal mode;
// regex mode keeps the whole query as one pattern.

/// Boolean query AST. Matching is substring-based (see `matches`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    Term(String),
    Not(Box<Query>),
    And(Vec<Query>),
    Or(Vec<Query>),
}

impl Query {
    /// True when the decoded line satisfies the query.
    /// `case_sensitive=false` folds ASCII case on the fly (no big copy).
    pub fn matches(&self, line: &str, case_sensitive: bool) -> bool {
        match self {
            Query::Term(t) => contains(line, t, case_sensitive),
            Query::Not(q) => !q.matches(line, case_sensitive),
            Query::And(qs) => qs.iter().all(|q| q.matches(line, case_sensitive)),
            Query::Or(qs) => qs.iter().any(|q| q.matches(line, case_sensitive)),
        }
    }

    /// Positive (non-negated) terms, for hit-column placement.
    pub fn positive_terms(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_positive(false, &mut out);
        out
    }

    fn collect_positive<'a>(&'a self, neg: bool, out: &mut Vec<&'a str>) {
        match self {
            Query::Term(t) => {
                if !neg && !t.is_empty() {
                    out.push(t.as_str());
                }
            }
            Query::Not(q) => q.collect_positive(!neg, out),
            Query::And(qs) | Query::Or(qs) => {
                for q in qs {
                    q.collect_positive(neg, out);
                }
            }
        }
    }

    /// True when a byte-level OR over `positive_terms()` is a SOUND prefilter:
    /// every full match must contain at least one positive term, so lines
    /// without any of them can skip decode + AST eval without false negatives.
    /// Holds for negation-free ASTs with non-empty leaves and non-empty
    /// And/Or lists. Counter-example otherwise: `a OR -b` matches lines
    /// containing neither (via `-b`), so a union prefilter would drop them.
    pub fn is_prefilter_safe(&self) -> bool {
        match self {
            Query::Term(t) => !t.is_empty(),
            Query::Not(_) => false,
            Query::And(qs) | Query::Or(qs) => !qs.is_empty() && qs.iter().all(|q| q.is_prefilter_safe()),
        }
    }

    /// Terms that EVERY full match must contain (conjunction core).
    /// Term(t) non-empty at positive polarity -> {t}; And (positive) ->
    /// union of children's; Or/Not-anything-negative -> {}.
    /// Sound by induction on polarity: an And-match implies every conjunct
    /// matches, and a positive Term-match implies containment; Or branches
    /// and anything under Not promise nothing, so they contribute nothing.
    /// The worker prefilters on the longest one (most selective single
    /// literal, plain SIMD memchr).
    pub fn required_terms(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_required(false, &mut out);
        out
    }

    fn collect_required<'a>(&'a self, neg: bool, out: &mut Vec<&'a str>) {
        match self {
            Query::Term(t) => {
                if !neg && !t.is_empty() {
                    out.push(t.as_str());
                }
            }
            Query::Not(q) => q.collect_required(!neg, out),
            Query::And(qs) => {
                if !neg {
                    for q in qs {
                        q.collect_required(neg, out);
                    }
                }
            }
            Query::Or(_) => {}
        }
    }
}

fn contains(hay: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    if case_sensitive {
        hay.contains(needle)
    } else {
        // ASCII fold both sides without allocating the whole haystack twice:
        // memchr-style scan over lowercased needle against folded windows.
        // Simpler correct version: fold hay once per call (lines are short).
        hay.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }
}

/// Byte span of the first positive term in the line (for hit columns).
/// Returns (0, 0) when there is no positive term or no match.
pub fn first_match_span(line: &str, terms: &[&str], case_sensitive: bool) -> (u32, u32) {
    let mut best: Option<(usize, usize)> = None;
    for t in terms {
        if t.is_empty() {
            continue;
        }
        let pos = if case_sensitive {
            memchr::memmem::Finder::new(t.as_bytes())
                .find(line.as_bytes())
        } else {
            let h = line.as_bytes().to_ascii_lowercase();
            let n = t.as_bytes().to_ascii_lowercase();
            memchr::memmem::Finder::new(&n).find(&h)
        };
        if let Some(m) = pos {
            let span = (m, m + t.len());
            if best.map(|b| span.0 < b.0).unwrap_or(true) {
                best = Some(span);
            }
        }
    }
    best.map(|(s, e)| (s as u32, e as u32)).unwrap_or((0, 0))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Term(String),
    And,
    Or,
    Not,
    LParen,
    RParen,
}

/// True when the raw query needs the boolean path (vs fast single literal).
/// Single bare token without specials stays on the SIMD literal path.
pub fn is_boolean_query(q: &str) -> bool {
    let t = q.trim();
    if t.is_empty() {
        return false;
    }
    // Any operator syntax forces boolean evaluation.
    if t.contains('|') || t.contains('(') || t.contains(')') || t.contains('"') {
        return true;
    }
    let up = t.to_ascii_uppercase();
    if up.split_whitespace().any(|w| w == "OR" || w == "AND" || w == "NOT") {
        return true;
    }
    if t.split_whitespace().any(|w| w.starts_with('-') && w.len() > 1) {
        return true;
    }
    // Two or more bare tokens = implicit AND.
    t.split_whitespace().count() > 1
}

fn tokenize(q: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let mut chars = q.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        match c {
            '(' => {
                toks.push(Tok::LParen);
                chars.next();
            }
            ')' => {
                toks.push(Tok::RParen);
                chars.next();
            }
            '|' => {
                toks.push(Tok::Or);
                chars.next();
                // "||" is also OR.
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
            }
            '"' => {
                chars.next(); // opening quote
                let mut s = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    s.push(c);
                }
                if !closed {
                    return Err(String::from("Tanda kutip tidak ditutup."));
                }
                toks.push(Tok::Term(s));
            }
            '-' => {
                // `-` prefix = NOT, but only when glued to a term/paren/quote.
                // Peek ahead: if whitespace/end follows, treat as literal dash term.
                let mut look = chars.clone();
                look.next();
                match look.peek() {
                    Some(nc) if !nc.is_whitespace() => {
                        toks.push(Tok::Not);
                        chars.next();
                    }
                    _ => {
                        toks.push(Tok::Term(collect_bare(&mut chars)));
                    }
                }
            }
            _ => {
                let word = collect_bare(&mut chars);
                let up = word.to_ascii_uppercase();
                if up == "OR" {
                    toks.push(Tok::Or);
                } else if up == "AND" {
                    toks.push(Tok::And);
                } else if up == "NOT" {
                    toks.push(Tok::Not);
                } else {
                    toks.push(Tok::Term(word));
                }
            }
        }
    }
    Ok(toks)
}

fn collect_bare(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == '(' || c == ')' || c == '|' || c == '"' {
            break;
        }
        s.push(c);
        chars.next();
    }
    s
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

/// Parse `q` into an AST. Indonesian errors, never panics.
pub fn parse_query(q: &str) -> Result<Query, String> {
    let toks = tokenize(q)?;
    if toks.is_empty() {
        return Err(String::from("Query kosong."));
    }
    let mut p = Parser { toks, pos: 0 };
    let node = p.parse_or()?;
    if p.pos != p.toks.len() {
        return Err(String::from("Query tidak valid di dekat tanda kurung."));
    }
    Ok(node.simplify())
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    // OR level: and (OR and)*
    fn parse_or(&mut self) -> Result<Query, String> {
        let mut nodes = vec![self.parse_and()?];
        while self.peek() == Some(&Tok::Or) {
            self.pos += 1;
            nodes.push(self.parse_and()?);
        }
        Ok(if nodes.len() == 1 {
            nodes.pop().unwrap()
        } else {
            Query::Or(nodes)
        })
    }

    // AND level: unary ((AND | implicit) unary)*
    fn parse_and(&mut self) -> Result<Query, String> {
        let mut nodes = vec![self.parse_unary()?];
        loop {
            match self.peek() {
                Some(Tok::And) => {
                    self.pos += 1;
                    nodes.push(self.parse_unary()?);
                }
                Some(Tok::Term(_)) | Some(Tok::Not) | Some(Tok::LParen) => {
                    // Implicit AND by juxtaposition.
                    nodes.push(self.parse_unary()?);
                }
                _ => break,
            }
        }
        Ok(if nodes.len() == 1 {
            nodes.pop().unwrap()
        } else {
            Query::And(nodes)
        })
    }

    fn parse_unary(&mut self) -> Result<Query, String> {
        match self.peek() {
            Some(Tok::Not) => {
                self.pos += 1;
                Ok(Query::Not(Box::new(self.parse_unary()?)))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let node = self.parse_or()?;
                match self.peek() {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(node)
                    }
                    _ => Err(String::from("Kurung tutup ) kurang.")),
                }
            }
            Some(Tok::Term(t)) => {
                let t = t.clone();
                self.pos += 1;
                Ok(Query::Term(t))
            }
            Some(_) => Err(String::from("Query tidak valid.")),
            None => Err(String::from("Query berakhir tiba-tiba.")),
        }
    }
}

impl Query {
    /// Flatten single-child And/Or and drop empty terms.
    fn simplify(self) -> Self {
        match self {
            Query::And(mut qs) => {
                let mut flat = Vec::new();
                for q in qs.drain(..) {
                    match q.simplify() {
                        Query::And(inner) => flat.extend(inner),
                        Query::Term(t) if t.is_empty() => {}
                        other => flat.push(other),
                    }
                }
                if flat.len() == 1 {
                    flat.pop().unwrap()
                } else {
                    Query::And(flat)
                }
            }
            Query::Or(mut qs) => {
                let mut flat = Vec::new();
                for q in qs.drain(..) {
                    match q.simplify() {
                        Query::Or(inner) => flat.extend(inner),
                        Query::Term(t) if t.is_empty() => {}
                        other => flat.push(other),
                    }
                }
                if flat.len() == 1 {
                    flat.pop().unwrap()
                } else {
                    Query::Or(flat)
                }
            }
            Query::Not(q) => Query::Not(Box::new(q.simplify())),
            other => other,
        }
    }
}

/// Convert a parsed boolean query to exact filter syntax ("Jadikan filter").
/// Succeeds only when the meaning is preserved 1:1; otherwise returns the
/// Indonesian reason so the UI can warn instead of silently changing meaning.
/// Mapping (polarity-aware, De Morgan for negated OR):
/// Term -> include token; Not(Term) -> `-token`; And -> space-joined;
/// Not(Or(..)) -> AND of negations. Everything else (Or, Not(And),
/// quoted phrases with spaces, `kunci=nilai`, lone `-`) is rejected:
/// OR has no filter counterpart, spaces would split tokens, and `=`
/// means a JSON field predicate in filter (different semantics from a
/// substring on plain logs).
pub fn to_filter_string(q: &Query) -> Result<String, String> {
    fn token(t: &str) -> Result<String, String> {
        if t.contains(char::is_whitespace) {
            return Err(String::from(
                "frasa kutip (ber-spasi) tak punya padanan token filter.",
            ));
        }
        if t.contains('=') {
            return Err(String::from(
                "kunci=nilai berarti predikat JSON di filter, bukan substring.",
            ));
        }
        if t == "-" {
            return Err(String::from("tanda - tunggal diabaikan oleh filter."));
        }
        Ok(t.to_string())
    }
    fn go(q: &Query, neg: bool, out: &mut Vec<String>) -> Result<(), String> {
        match q {
            Query::Term(t) => {
                let mut tok = token(t)?;
                if neg {
                    tok.insert(0, '-');
                }
                out.push(tok);
                Ok(())
            }
            Query::Not(inner) => go(inner, !neg, out),
            Query::And(qs) => {
                if neg {
                    return Err(String::from(
                        "bentuk ini setara OR (De Morgan) yang tak ada di filter.",
                    ));
                }
                if qs.is_empty() {
                    return Err(String::from("konjungsi kosong."));
                }
                for c in qs {
                    go(c, false, out)?;
                }
                Ok(())
            }
            Query::Or(qs) => {
                if !neg {
                    return Err(String::from(
                        "OR tak punya padanan di filter (filter selalu AND).",
                    ));
                }
                if qs.is_empty() {
                    return Err(String::from("disjungsi kosong."));
                }
                for c in qs {
                    go(c, true, out)?;
                }
                Ok(())
            }
        }
    }
    let mut out = Vec::new();
    go(q, false, &mut out)?;
    if out.is_empty() {
        return Err(String::from("tak ada token yang bisa dibawa."));
    }
    Ok(out.join(" "))
}

/// Top-level display spans for the chip builder: (text, byte_start, byte_end).
/// Returns None for complex expressions (OR/parens/quotes at top level);
/// the UI then shows one summary chip instead of per-term chips.
pub fn top_spans(q: &str) -> Option<Vec<(String, usize, usize)>> {
    let toks = tokenize(q).ok()?;
    // Complex when top-level OR/paren/quote tokens exist. Quotes already
    // merged into Term by the tokenizer, so track them separately.
    if toks.iter().any(|t| matches!(t, Tok::Or | Tok::LParen | Tok::RParen)) {
        return None;
    }
    if q.contains('"') {
        return None;
    }
    // Walk raw text, pairing whitespace-separated tokens (skip AND keywords,
    // keep -NOT prefixes glued to their term).
    let bytes = q.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
            i += 1;
        }
        let word = &q[start..i];
        if word.eq_ignore_ascii_case("AND") || word.eq_ignore_ascii_case("NOT") {
            continue;
        }
        out.push((word.to_string(), start, i));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_over_or() {
        // a OR b c  ==  a OR (b AND c)
        let q = parse_query("a OR b c").unwrap();
        assert_eq!(
            q,
            Query::Or(vec![
                Query::Term("a".into()),
                Query::And(vec![Query::Term("b".into()), Query::Term("c".into())]),
            ])
        );
    }

    #[test]
    fn not_and_parens_and_quotes() {
        let q = parse_query("err AND timeout NOT debug").unwrap();
        assert!(matches!(q, Query::And(_)));
        let q = parse_query("(a OR b) -c").unwrap();
        assert_eq!(
            q,
            Query::And(vec![
                Query::Or(vec![Query::Term("a".into()), Query::Term("b".into())]),
                Query::Not(Box::new(Query::Term("c".into()))),
            ])
        );
        let q = parse_query("\"order service\" -debug").unwrap();
        assert_eq!(
            q,
            Query::And(vec![
                Query::Term("order service".into()),
                Query::Not(Box::new(Query::Term("debug".into()))),
            ])
        );
    }

    #[test]
    fn eval_semantics() {
        let q = parse_query("err timeout -debug").unwrap();
        assert!(q.matches("ERR timeout here", false));
        assert!(!q.matches("ERR timeout debug", false));
        assert!(!q.matches("ERR only", false));
        let q = parse_query("a OR b").unwrap();
        assert!(q.matches("has b", false));
        assert!(!q.matches("none", false));
        // case sensitivity
        let q = parse_query("ERROR").unwrap();
        assert!(q.matches("error x", false));
        assert!(!q.matches("error x", true));
    }

    #[test]
    fn invalid_queries_error() {
        assert!(parse_query("").is_err());
        assert!(parse_query("(a OR").is_err());
        assert!(parse_query("\"abc").is_err());
        assert!(parse_query("NOT").is_err());
    }

    #[test]
    fn fast_path_detection() {
        assert!(!is_boolean_query("ERROR"));
        assert!(!is_boolean_query("  timeout  "));
        assert!(is_boolean_query("err timeout"));
        assert!(is_boolean_query("err -debug"));
        assert!(is_boolean_query("a OR b"));
        assert!(is_boolean_query("(a)"));
        assert!(is_boolean_query("\"order service\""));
        assert!(is_boolean_query("a|b"));
    }

    #[test]
    fn spans_for_chips() {
        let s = top_spans("err timeout -debug").unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].0, "err");
        // Removing span 1 ("timeout") leaves valid remainder.
        let q = "err timeout -debug";
        let (_, a, b) = &s[1];
        let rest = format!("{} {}", q[..*a].trim_end(), q[*b..].trim_start());
        assert_eq!(rest, "err -debug");
        // Complex expressions yield no per-term spans.
        assert!(top_spans("a OR b").is_none());
        assert!(top_spans("(a b)").is_none());
    }

    #[test]
    fn first_span_positions() {
        let terms = vec!["timeout", "err"];
        assert_eq!(first_match_span("xx ERR yy", &terms, false), (3, 6));
        assert_eq!(first_match_span("nothing", &terms, false), (0, 0));
    }

    #[test]
    fn prefilter_safe_flag() {
        // Pure-positive shapes: union prefilter is sound.
        for q in ["err timeout", "err OR timeout", "(a b) OR c", "\"a b\" c"] {
            let ast = parse_query(q).unwrap();
            assert!(ast.is_prefilter_safe(), "{}", q);
            assert!(!ast.positive_terms().is_empty());
        }
        // Any negation (or empty leaf) disables the prefilter.
        for q in ["err -debug", "err OR -debug", "-err", "NOT err", "a OR (b NOT c)"] {
            let ast = parse_query(q).unwrap_or(Query::Term(String::new()));
            assert!(!ast.is_prefilter_safe(), "{}", q);
        }
    }

    #[test]
    fn prefilter_sound_on_safe_queries() {
        // Differential: full-match lines must all contain a
        // positive term (union) and every required term.
        let lines = [
            "ERROR timeout on request",
            "error TIMEOUT twice timeout",
            "INFO all good",
            "timeout",
            "nothing here",
            "terror",
            "a b c",
            "DEBUG noise",
        ];
        for q in [
            "err timeout",
            "err OR timeout",
            "(err timeout) OR info",
            "\"all good\"",
            "err -debug",
            "err OR -debug",
            "NOT (err timeout)",
            "NOT NOT err",
        ] {
            let ast = parse_query(q).unwrap();
            let terms = ast.positive_terms();
            let req = ast.required_terms();
            let safe = ast.is_prefilter_safe();
            for ln in lines {
                if ast.matches(ln, false) {
                    let low = ln.to_ascii_lowercase();
                    for r in &req {
                        assert!(
                            low.contains(&r.to_ascii_lowercase()),
                            "query {:?} matched {:?} without required {:?}",
                            q,
                            ln,
                            r
                        );
                    }
                    if safe {
                        assert!(
                            terms.iter().any(|t| low.contains(&t.to_ascii_lowercase())),
                            "query {:?} matched {:?} without any positive term",
                            q,
                            ln
                        );
                    }
                }
            }
        }
        // Shape expectations (parse-dependent, documents the contract).
        assert_eq!(parse_query("err timeout").unwrap().required_terms(), vec!["err", "timeout"]);
        assert!(parse_query("err OR -debug").unwrap().required_terms().is_empty());
        assert_eq!(parse_query("err -debug").unwrap().required_terms(), vec!["err"]);
        assert!(parse_query("-err").unwrap().required_terms().is_empty());
        // Negated conjunction promises nothing (De Morgan): no prefilter.
        assert!(parse_query("NOT (a b)").unwrap().required_terms().is_empty());
        assert_eq!(parse_query("a NOT (b c)").unwrap().required_terms(), vec!["a"]);
        assert_eq!(
            parse_query("NOT NOT x").unwrap().required_terms(),
            vec!["x"]
        );
    }

    fn conv(q: &str) -> Result<String, String> {
        to_filter_string(&parse_query(q).unwrap())
    }

    #[test]
    fn filter_conversion_exact_shapes() {
        assert_eq!(conv("err timeout").unwrap(), "err timeout");
        assert_eq!(conv("err -debug").unwrap(), "err -debug");
        assert_eq!(conv("-debug").unwrap(), "-debug");
        assert_eq!(conv("NOT (a OR b)").unwrap(), "-a -b");
        assert_eq!(conv("(a b) c").unwrap(), "a b c");
        assert_eq!(conv("NOT NOT x").unwrap(), "x");
    }

    #[test]
    fn filter_conversion_rejects_with_reason() {
        // OR / negated-AND change meaning under AND-only filter.
        assert!(conv("a OR b").is_err());
        assert!(conv("NOT (a b)").is_err());
        assert!(conv("(a OR b) c").is_err());
        // Quoted phrase would split into tokens.
        assert!(conv("\"order service\"").is_err());
        assert!(conv("\"a b\" c").is_err());
        // `=` is a JSON field predicate in filter, substring in search.
        assert!(conv("level=ERROR").is_err());
        assert!(conv("id=1 -debug").is_err());
    }

    #[test]
    fn filter_conversion_agrees_with_both_engines() {
        use crate::engine::filter::{line_matches, parse_filter};
        // Differential: converted filter decides exactly like the AST.
        let lines = [
            "ERROR timeout x",
            "ERROR debug y",
            "INFO timeout z",
            "debug only",
            "nothing",
        ];
        for q in ["err timeout", "err -debug", "-debug", "NOT (a OR b)"] {
            let ast = parse_query(q).unwrap();
            let f = parse_filter(&conv(q).unwrap(), false);
            for ln in lines {
                assert_eq!(
                    ast.matches(ln, false),
                    line_matches(ln, &f),
                    "query {:?} vs filter on {:?}",
                    q,
                    ln
                );
            }
        }
    }
}
