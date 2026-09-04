// English comments: scratchpad transforms (pure functions, tested).
// Base64/JWT decode, JSON pretty, light SQL tidy, text stats.

/// Decode standard or URL-safe base64 (whitespace tolerated).
pub fn b64_decode(s: &str) -> Result<String, String> {
    use base64::Engine as _;
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    for engine in [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
    ] {
        if let Ok(bytes) = engine.decode(&clean) {
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    Err(String::from("Bukan base64 yang valid."))
}

/// Decode JWT `header.payload[.sig]` into pretty JSON (header + payload).
pub fn jwt_decode(s: &str) -> Result<String, String> {
    let t = s.trim();
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(String::from("JWT harus berbentuk header.payload[.sig]."));
    }
    let mut out = String::new();
    for (i, label) in ["header", "payload"].iter().enumerate() {
        let raw = b64_decode(parts[i])
            .map_err(|_| format!("Segmen {} bukan base64url valid.", label))?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| format!("Segmen {} bukan JSON.", label))?;
        let pretty =
            serde_json::to_string_pretty(&v).map_err(|e| format!("Gagal merapikan: {}", e))?;
        out.push_str(&format!("== {} ==\n{}\n", label, pretty));
    }
    Ok(out)
}

/// Pretty-print JSON (2 spaces). Passes input through on scalar roots.
pub fn json_pretty(s: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(s.trim()).map_err(|e| format!("Bukan JSON valid: {}", e))?;
    serde_json::to_string_pretty(&v).map_err(|e| format!("Gagal merapikan: {}", e))
}

/// Light SQL tidy: uppercase major keywords, newlines before clauses.
/// Heuristic only (no full parser); keeps string literals intact-ish by
/// skipping replacements inside single quotes.
pub fn sql_tidy(s: &str) -> String {
    const KW: &[&str] = &[
        "SELECT", "INSERT INTO", "UPDATE", "DELETE FROM", "VALUES", "FROM", "WHERE",
        "GROUP BY", "ORDER BY", "HAVING", "JOIN", "LEFT JOIN", "RIGHT JOIN", "INNER JOIN",
        "ON", "UNION", "SET",
    ];
    // Split into quoted / unquoted spans.
    let mut spans: Vec<(bool, String)> = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in s.chars() {
        if c == '\'' {
            if in_q {
                cur.push(c);
                spans.push((true, std::mem::take(&mut cur)));
                in_q = false;
            } else {
                if !cur.is_empty() {
                    spans.push((false, std::mem::take(&mut cur)));
                }
                cur.push(c);
                in_q = true;
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        spans.push((in_q, cur));
    }
    let mut out = String::new();
    for (quoted, seg) in spans {
        if quoted {
            out.push_str(&seg);
            continue;
        }
        let mut seg = seg;
        // Longest keywords first.
        let mut kws: Vec<&str> = KW.to_vec();
        kws.sort_by_key(|k| std::cmp::Reverse(k.len()));
        for k in kws {
            let re = regex::RegexBuilder::new(&format!(r"(?i)\b{}\b", regex::escape(k)))
                .build()
                .unwrap();
            seg = re.replace_all(&seg, k).into_owned();
        }
        out.push_str(&seg);
    }
    // Newlines before major clause starts (outside quotes: re-split cheaply by
    // processing line by line is overkill; apply on the keyword boundaries).
    let mut res = out;
    for k in ["FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "VALUES", "UNION", "JOIN"] {
        let re = regex::RegexBuilder::new(&format!(r"[ \t]+{}\b", regex::escape(k)))
            .build()
            .unwrap();
        res = re.replace_all(&res, format!("\n{}", k)).into_owned();
    }
    // Collapse 3+ newlines, trim trailing space per line.
    let lines: Vec<String> = res.lines().map(|l| l.trim_end().to_string()).collect();
    let mut tidy = lines.join("\n");
    while tidy.contains("\n\n\n") {
        tidy = tidy.replace("\n\n\n", "\n\n");
    }
    tidy.trim().to_string()
}

/// (chars, words, lines) of scratch text.
pub fn stats(s: &str) -> (usize, usize, usize) {
    let chars = s.chars().count();
    let words = s.split_whitespace().count();
    let lines = if s.is_empty() { 0 } else { s.lines().count() };
    (chars, words, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_variants() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), "hello");
        assert!(b64_decode("!!!").is_err());
    }

    #[test]
    fn jwt_parts() {
        // {"alg":"none"} . {"sub":"42"}
        let t = "eyJhbGciOiJub25lIn0.eyJzdWIiOiI0MiJ9.";
        let out = jwt_decode(t).unwrap();
        assert!(out.contains("\"sub\": \"42\""));
        assert!(jwt_decode("satu").is_err());
    }

    #[test]
    fn pretty_and_stats() {
        assert!(json_pretty(r#"{"a":1}"#).unwrap().contains('\n'));
        assert!(json_pretty("bukan").is_err());
        assert_eq!(stats("a b\nc"), (5, 3, 2));
    }

    #[test]
    fn tidy_uppercases_keywords() {
        let out = sql_tidy("select a from t where a=1");
        assert!(out.contains("SELECT") && out.contains("FROM") && out.contains("WHERE"));
        // Quoted text untouched.
        let out = sql_tidy("select 'fromage' from t");
        assert!(out.contains("'fromage'"));
    }
}
