// English comments: Fast zero-copy parsing of transactional SQL log lines.
// Splits a line into (timestamp, session, action, query) columns if structured.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlCols<'a> {
    pub timestamp: &'a str,
    pub session: &'a str,
    pub action: &'a str,
    pub query: &'a str,
}

/// Known SQL action keywords to recognize.
const SQL_ACTIONS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "COMMIT",
    "ROLLBACK", "CHECKPOINT", "TRANSACTION", "EXECUTE", "CALL",
    "CREATE", "ALTER", "DROP", "TRUNCATE", "BEGIN",
];

/// Attempt to split `line` into 4 logical columns:
/// (timestamp, session, action, query). Returns None if the line does not
/// look like a structured database/transaction log.
pub fn parse_sql_cols<'a>(line: &'a str) -> Option<SqlCols<'a>> {
    let trimmed = line.trim();
    if trimmed.len() < 12 {
        return None;
    }

    // 1. Timestamp candidate: starts with YYYY-MM-DD or [YYYY-MM-DD]
    let (ts, rest) = if let Some(stripped) = trimmed.strip_prefix('[') {
        let end_bracket = stripped.find(']')?;
        let inside = &stripped[..end_bracket];
        if inside.len() >= 10 && inside.chars().take(4).all(|c| c.is_ascii_digit()) {
            (inside, stripped[end_bracket + 1..].trim_start())
        } else {
            return None;
        }
    } else if trimmed.len() >= 19
        && trimmed.chars().take(4).all(|c| c.is_ascii_digit())
        && trimmed.as_bytes().get(4) == Some(&b'-')
    {
        // "YYYY-MM-DD HH:MM:SS" or "YYYY-MM-DD HH:MM:SS.mmm"
        let mut split_pos = 19;
        if trimmed.as_bytes().get(19) == Some(&b'.') {
            let mut i = 20;
            while i < trimmed.len() && trimmed.as_bytes()[i].is_ascii_digit() {
                i += 1;
            }
            split_pos = i;
        }
        let ts = &trimmed[..split_pos];
        let rest = trimmed[split_pos..].trim_start();
        (ts, rest)
    } else {
        return None;
    };

    if rest.is_empty() {
        return None;
    }

    // 2. Session / context candidate: e.g. [sid:123], [SESSION-01], (tid-1), or space token
    let (sess, rest2) = if rest.starts_with('[') {
        if let Some(end_b) = rest.find(']') {
            (&rest[1..end_b], rest[end_b + 1..].trim_start())
        } else {
            ("-", rest)
        }
    } else if rest.starts_with('(') {
        if let Some(end_b) = rest.find(')') {
            (&rest[1..end_b], rest[end_b + 1..].trim_start())
        } else {
            ("-", rest)
        }
    } else {
        // Look for session=... or id=... or next word
        if let Some(pos) = rest.find(char::is_whitespace) {
            let first_word = &rest[..pos];
            if first_word.starts_with("session=") || first_word.starts_with("sid=") || first_word.starts_with("user=") {
                (first_word, rest[pos..].trim_start())
            } else {
                ("-", rest)
            }
        } else {
            ("-", rest)
        }
    };

    // 3. Action candidate: look for SQL keywords
    let mut action = "-";
    let mut query = rest2;

    // Check if rest2 starts with an action or has an action as the first word
    if let Some(space_pos) = rest2.find(char::is_whitespace) {
        let first = &rest2[..space_pos];
        let upper = first.to_ascii_uppercase();
        for &k in SQL_ACTIONS {
            if upper == k {
                action = k;
                query = rest2[space_pos..].trim_start();
                break;
            }
        }
        // If not found in first word, check second word (e.g. "INFO - INSERT INTO ...")
        if action == "-" {
            let rem = rest2[space_pos..].trim_start();
            if let Some(p2) = rem.find(char::is_whitespace) {
                let second = &rem[..p2];
                let upper2 = second.to_ascii_uppercase();
                for &k in SQL_ACTIONS {
                    if upper2 == k {
                        action = k;
                        query = rem[p2..].trim_start();
                        break;
                    }
                }
            }
        }
    } else {
        let upper = rest2.to_ascii_uppercase();
        for &k in SQL_ACTIONS {
            if upper == k {
                action = k;
                query = "";
                break;
            }
        }
    }

    if action != "-" || sess != "-" {
        Some(SqlCols {
            timestamp: ts,
            session: sess,
            action,
            query,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bracketed_sql_log() {
        let line = "[2026-09-04 10:00:01] [SESSION-42] INSERT INTO orders (id, total) VALUES (1, 50000)";
        let cols = parse_sql_cols(line).expect("must parse");
        assert_eq!(cols.timestamp, "2026-09-04 10:00:01");
        assert_eq!(cols.session, "SESSION-42");
        assert_eq!(cols.action, "INSERT");
        assert_eq!(cols.query, "INTO orders (id, total) VALUES (1, 50000)");
    }

    #[test]
    fn parse_plain_timestamp_sql_log() {
        let line = "2026-09-04 10:00:01.123 session=user99 UPDATE accounts SET balance = balance - 100 WHERE id = 1";
        let cols = parse_sql_cols(line).expect("must parse");
        assert_eq!(cols.timestamp, "2026-09-04 10:00:01.123");
        assert_eq!(cols.session, "session=user99");
        assert_eq!(cols.action, "UPDATE");
        assert!(cols.query.starts_with("accounts SET balance"));
    }

    #[test]
    fn non_sql_line_rejected() {
        assert!(parse_sql_cols("random text without timestamp").is_none());
        assert!(parse_sql_cols("").is_none());
    }
}
