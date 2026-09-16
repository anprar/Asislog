// English comments: SQL-lite analyzer over parsed log records (no new deps).
// Subset dialect (documented honestly in UI + README, NOT full Transact-SQL):
//   SELECT <cols|*> [WHERE <expr>] [GROUP BY <f>[,<f>...]]
//          [ORDER BY <count|<f>> [ASC|DESC]] [LIMIT <n>]
//   SELECT COUNT(*) [WHERE <expr>] [GROUP BY ...] ...
// WHERE expr: AND/OR/NOT, parens, comparisons:
//   f = 'v' (case-insensitive equality), f != 'v',
//   f ~ 'v' (contains, insensitive), f !~ 'v',
//   f >/</>=/<= 'v' (numeric, else timestamp, else string).
// Bare words may be unquoted when they contain no spaces.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Contains,
    NotContains,
    Gt,
    Lt,
    Ge,
    Le,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    True,
    Cmp {
        field: String,
        op: CmpOp,
        value: String,
    },
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Select {
    All,
    Fields(Vec<String>),
    CountStar,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub select: Select,
    pub where_expr: Expr,
    pub group_by: Vec<String>,
    pub order_by: Option<(String, bool)>, // (key, descending)
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            select: Select::All,
            where_expr: Expr::True,
            group_by: Vec::new(),
            order_by: None,
            limit: 1000,
        }
    }
}

// ---------- lexer ----------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(String),
    Op(String),
    LParen,
    RParen,
    Comma,
    Star,
    End,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            '*' => {
                toks.push(Tok::Star);
                i += 1;
            }
            '\'' | '"' => {
                let q = c;
                let mut j = i + 1;
                let mut lit = String::new();
                let mut closed = false;
                while j < s.len() {
                    let d = b[j] as char;
                    if d == q {
                        // '' escape inside single quotes.
                        if q == '\'' && s[j + 1..].starts_with('\'') {
                            lit.push('\'');
                            j += 2;
                            continue;
                        }
                        closed = true;
                        j += 1;
                        break;
                    }
                    lit.push(d);
                    j += 1;
                }
                if !closed {
                    return Err(String::from("String tidak ditutup (tambah kutip akhir)."));
                }
                toks.push(Tok::Str(lit));
                i = j;
            }
            '!' | '=' | '<' | '>' | '~' => {
                let rest = &s[i..];
                for op in ["!~", "!=", "<>", ">=", "<=", "=", ">", "<", "~"] {
                    if rest.starts_with(op) {
                        toks.push(Tok::Op(op.to_string()));
                        i += op.len();
                        break;
                    }
                }
                if matches!(toks.last(), Some(Tok::Op(_))) {
                    continue;
                }
                return Err(format!("Operator tak dikenal di: {}", rest));
            }
            _ => {
                // Ident / number / bare value.
                let mut j = i;
                while j < s.len() {
                    let d = b[j] as char;
                    if d.is_whitespace() || "()',\"*!<>~= ".contains(d) {
                        break;
                    }
                    j += 1;
                }
                if j == i {
                    return Err(format!("Karakter tak dikenal: '{}'", c));
                }
                let word = s[i..j].to_string();
                if word.chars().all(|c| c.is_ascii_digit()) {
                    toks.push(Tok::Num(word));
                } else {
                    toks.push(Tok::Ident(word));
                }
                i = j;
            }
        }
    }
    toks.push(Tok::End);
    Ok(toks)
}

// ---------- parser ----------

struct P {
    toks: Vec<Tok>,
    pos: usize,
}

impl P {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn kw(&mut self, word: &str) -> bool {
        match self.peek() {
            Tok::Ident(w) if w.eq_ignore_ascii_case(word) => {
                self.next();
                true
            }
            _ => false,
        }
    }
    fn expect_kw(&mut self, word: &str) -> Result<(), String> {
        if self.kw(word) {
            Ok(())
        } else {
            Err(format!("Perlu kata kunci {}, dapat {:?}", word, self.peek()))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Tok::Ident(w) => Ok(w),
            Tok::Str(s) => Ok(s),
            t => Err(format!("Perlu nama kolom, dapat {:?}", t)),
        }
    }
    fn value(&mut self) -> Result<String, String> {
        match self.next() {
            Tok::Str(s) => Ok(s),
            Tok::Ident(w) => Ok(w),
            Tok::Num(n) => Ok(n),
            t => Err(format!("Perlu nilai, dapat {:?}", t)),
        }
    }
}

fn is_reserved(w: &str) -> bool {
    matches!(
        w.to_ascii_uppercase().as_str(),
        "SELECT" | "FROM" | "WHERE" | "GROUP" | "BY" | "ORDER" | "LIMIT" | "AND" | "OR" | "NOT"
            | "ASC" | "DESC" | "COUNT"
    )
}

fn parse_or(p: &mut P) -> Result<Expr, String> {
    let mut e = parse_and(p)?;
    while p.kw("OR") {
        let r = parse_and(p)?;
        e = Expr::Or(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_and(p: &mut P) -> Result<Expr, String> {
    let mut e = parse_not(p)?;
    while p.kw("AND") {
        let r = parse_not(p)?;
        e = Expr::And(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_not(p: &mut P) -> Result<Expr, String> {
    if p.kw("NOT") {
        return Ok(Expr::Not(Box::new(parse_not(p)?)));
    }
    parse_atom(p)
}

fn parse_atom(p: &mut P) -> Result<Expr, String> {
    if matches!(p.peek(), Tok::LParen) {
        p.next();
        let e = parse_or(p)?;
        match p.next() {
            Tok::RParen => Ok(e),
            t => Err(format!("Perlu ), dapat {:?}", t)),
        }
    } else {
        let field = p.ident()?;
        let op = match p.next() {
            Tok::Op(o) => match o.as_str() {
                "=" => CmpOp::Eq,
                "!=" | "<>" => CmpOp::Ne,
                "~" => CmpOp::Contains,
                "!~" => CmpOp::NotContains,
                ">" => CmpOp::Gt,
                "<" => CmpOp::Lt,
                ">=" => CmpOp::Ge,
                "<=" => CmpOp::Le,
                _ => return Err(format!("Operator tak dikenal: {}", o)),
            },
            t => return Err(format!("Perlu operator (=, !=, ~, !~, >, <, >=, <=), dapat {:?}", t)),
        };
        let value = p.value()?;
        Ok(Expr::Cmp { field, op, value })
    }
}

/// Parse full query. Without leading SELECT, the whole string is a WHERE expr
/// with `SELECT *` implied (shorthand for quick filtering).
/// All errors share the `Query tidak valid: {detail}` shape so `tr_status`
/// translates them via the existing template pair.
pub fn parse_query(s: &str) -> Result<Query, String> {
    parse_query_inner(s).map_err(|e| format!("Query tidak valid: {}", e))
}

fn parse_query_inner(s: &str) -> Result<Query, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(String::from("Query kosong."));
    }
    let toks = lex(t)?;
    let mut p = P { toks, pos: 0 };
    // Shorthand: no SELECT -> WHERE-only.
    let is_select = matches!(p.peek(), Tok::Ident(w) if w.eq_ignore_ascii_case("SELECT"));
    if !is_select {
        let e = parse_or(&mut p)?;
        return Ok(Query {
            select: Select::All,
            where_expr: e,
            ..Query::default()
        });
    }
    p.expect_kw("SELECT")?;
    let select = if matches!(p.peek(), Tok::Star) {
        p.next();
        Select::All
    } else if matches!(p.peek(), Tok::Ident(w) if w.eq_ignore_ascii_case("COUNT")) {
        p.next();
        match p.next() {
            Tok::LParen => {}
            t => return Err(format!("COUNT perlu (, dapat {:?}", t)),
        }
        match p.next() {
            Tok::Star => {}
            t => return Err(format!("Hanya COUNT(*) didukung, dapat {:?}", t)),
        }
        match p.next() {
            Tok::RParen => {}
            t => return Err(format!("COUNT(*) perlu ), dapat {:?}", t)),
        }
        Select::CountStar
    } else {
        let mut cols = Vec::new();
        loop {
            let c = p.ident()?;
            if is_reserved(&c) {
                return Err(format!("'{}' kata kunci, bukan kolom.", c));
            }
            cols.push(c);
            if matches!(p.peek(), Tok::Comma) {
                p.next();
                continue;
            }
            break;
        }
        if cols.is_empty() {
            return Err(String::from("SELECT perlu kolom / * / COUNT(*)."));
        }
        Select::Fields(cols)
    };
    // Optional FROM logs (accepted, ignored — single source per run).
    if p.kw("FROM") {
        let _ = p.ident()?;
    }
    let mut q = Query {
        select,
        ..Query::default()
    };
    if p.kw("WHERE") {
        q.where_expr = parse_or(&mut p)?;
    }
    if p.kw("GROUP") {
        p.expect_kw("BY")?;
        loop {
            q.group_by.push(p.ident()?);
            if matches!(p.peek(), Tok::Comma) {
                p.next();
                continue;
            }
            break;
        }
    }
    if p.kw("ORDER") {
        p.expect_kw("BY")?;
        let key = match p.next() {
            Tok::Ident(w) => w,
            t => return Err(format!("ORDER BY perlu kolom/count, dapat {:?}", t)),
        };
        let desc = if p.kw("DESC") {
            true
        } else {
            let _ = p.kw("ASC");
            false
        };
        // Default for grouped output: count DESC unless stated.
        q.order_by = Some((key, desc));
    }
    if p.kw("LIMIT") {
        match p.next() {
            Tok::Num(n) => {
                q.limit = n.parse().unwrap_or(1000).clamp(1, 100_000);
            }
            t => return Err(format!("LIMIT perlu angka, dapat {:?}", t)),
        }
    }
    if !matches!(p.peek(), Tok::End) {
        return Err(format!("Token berlebih: {:?}", p.peek()));
    }
    Ok(q)
}

// ---------- eval ----------

/// One input row for the engine: canonical field map + builtins.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub fields: HashMap<String, String>,
    pub line: u64,
    pub file: String,
    pub ts: Option<i64>,
    pub ts_raw: String,
}

impl Row {
    pub fn get(&self, name: &str) -> Option<&str> {
        let l = name.to_ascii_lowercase();
        match l.as_str() {
            "line" => None, // handled numerically by caller via format
            "file" => Some(self.file.as_str()),
            "ts" | "timestamp" | "time" => {
                if self.ts_raw.is_empty() {
                    self.fields.get(&l).map(|s| s.as_str())
                } else {
                    Some(self.ts_raw.as_str())
                }
            }
            _ => self.fields.get(&l).map(|s| s.as_str()),
        }
    }
}

fn cmp_text(a: &str, b: &str, op: &CmpOp) -> bool {
    match op {
        CmpOp::Eq => a.eq_ignore_ascii_case(b),
        CmpOp::Ne => !a.eq_ignore_ascii_case(b),
        CmpOp::Contains => {
            a.to_ascii_lowercase().contains(&b.to_ascii_lowercase())
        }
        CmpOp::NotContains => {
            !a.to_ascii_lowercase().contains(&b.to_ascii_lowercase())
        }
        CmpOp::Gt | CmpOp::Lt | CmpOp::Ge | CmpOp::Le => {
            // Numeric first.
            if let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                return match op {
                    CmpOp::Gt => x > y,
                    CmpOp::Lt => x < y,
                    CmpOp::Ge => x >= y,
                    CmpOp::Le => x <= y,
                    _ => false,
                };
            }
            // Timestamp prefix compare.
            if let (Some(x), Some(y)) = (
                crate::engine::Doc::parse_timestamp_prefix(a.trim()),
                crate::engine::Doc::parse_timestamp_prefix(b.trim()),
            ) {
                return match op {
                    CmpOp::Gt => x > y,
                    CmpOp::Lt => x < y,
                    CmpOp::Ge => x >= y,
                    CmpOp::Le => x <= y,
                    _ => false,
                };
            }
            let ord = a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
            match op {
                CmpOp::Gt => ord == std::cmp::Ordering::Greater,
                CmpOp::Lt => ord == std::cmp::Ordering::Less,
                CmpOp::Ge => ord != std::cmp::Ordering::Less,
                CmpOp::Le => ord != std::cmp::Ordering::Greater,
                _ => false,
            }
        }
    }
}

pub fn eval_expr(e: &Expr, row: &Row) -> bool {
    match e {
        Expr::True => true,
        Expr::Not(x) => !eval_expr(x, row),
        Expr::And(a, b) => eval_expr(a, row) && eval_expr(b, row),
        Expr::Or(a, b) => eval_expr(a, row) || eval_expr(b, row),
        Expr::Cmp { field, op, value } => {
            if field.eq_ignore_ascii_case("line") {
                if let Ok(n) = value.trim().parse::<f64>() {
                    let ln = row.line as f64;
                    return match op {
                        CmpOp::Eq => ln == n,
                        CmpOp::Ne => ln != n,
                        CmpOp::Gt => ln > n,
                        CmpOp::Lt => ln < n,
                        CmpOp::Ge => ln >= n,
                        CmpOp::Le => ln <= n,
                        CmpOp::Contains | CmpOp::NotContains => {
                            cmp_text(&row.line.to_string(), value, op)
                        }
                    };
                }
                return cmp_text(&row.line.to_string(), value, op);
            }
            let v = row.get(field).unwrap_or("");
            cmp_text(v, value, op)
        }
    }
}

/// Result table for UI + CSV.
#[derive(Clone, Debug, Default)]
pub struct QueryResult {
    pub cols: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub scanned: u64,
    pub matched: u64,
    pub truncated_scan: bool,
    pub truncated_rows: bool,
    pub is_grouped: bool,
}

fn cell_of(row: &Row, col: &str) -> String {
    if col.eq_ignore_ascii_case("line") {
        return row.line.to_string();
    }
    if col.eq_ignore_ascii_case("file") {
        return row.file.clone();
    }
    if col.eq_ignore_ascii_case("count") {
        return String::new();
    }
    row.get(col).unwrap_or("").to_string()
}

/// Execute query over in-memory rows (bounded by caller).
pub fn execute(rows: &[Row], q: &Query) -> QueryResult {
    let scanned_n = rows.len() as u64;
    let matched: Vec<&Row> = rows.iter().filter(|r| eval_expr(&q.where_expr, r)).collect();
    let matched_n = matched.len() as u64;
    if q.select == Select::CountStar && q.group_by.is_empty() {
        return QueryResult {
            cols: vec!["count".to_string()],
            rows: vec![vec![matched_n.to_string()]],
            scanned: scanned_n,
            matched: matched_n,
            ..QueryResult::default()
        };
    }
    if !q.group_by.is_empty() {
        use std::collections::HashMap;
        let mut groups: HashMap<Vec<String>, usize> = HashMap::new();
        for r in matched {
            let key: Vec<String> = q.group_by.iter().map(|c| cell_of(r, c)).collect();
            *groups.entry(key).or_insert(0) += 1;
            if groups.len() > 5000 {
                break; // memory guard (documented)
            }
        }
        let mut grows: Vec<(Vec<String>, usize)> = groups.into_iter().collect();
        // ORDER BY: count default DESC for grouped output.
        match &q.order_by {
            Some((k, desc)) if !k.eq_ignore_ascii_case("count") => {
                let idx = q.group_by.iter().position(|c| c.eq_ignore_ascii_case(k));
                if let Some(ix) = idx {
                    grows.sort_by(|a, b| {
                        let o = a.0[ix].cmp(&b.0[ix]);
                        if *desc { o.reverse() } else { o }
                    });
                } else {
                    grows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
                }
            }
            Some((_, desc)) => {
                if *desc {
                    grows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
                } else {
                    grows.sort_by_key(|(_, c)| *c);
                }
            }
            None => grows.sort_by_key(|(_, c)| std::cmp::Reverse(*c)),
        }
        let mut cols: Vec<String> = q.group_by.clone();
        cols.push("count".to_string());
        let mut out = Vec::new();
        for (k, c) in grows.into_iter().take(q.limit) {
            let mut r = k;
            r.push(c.to_string());
            out.push(r);
        }
        // Distinct group keys may exceed limit.
        let trunc = out.len() >= q.limit;
        return QueryResult {
            cols,
            rows: out,
            scanned: scanned_n,
            matched: matched_n,
            truncated_rows: trunc,
            is_grouped: true,
            ..QueryResult::default()
        };
    }
    // Detail rows.
    let cols: Vec<String> = match &q.select {
        Select::All => vec![
            "line".to_string(),
            "file".to_string(),
            "ts".to_string(),
            "level".to_string(),
            "msg".to_string(),
        ],
        Select::Fields(f) => {
            let mut c = vec!["line".to_string(), "file".to_string()];
            for x in f {
                if !x.eq_ignore_ascii_case("line") && !x.eq_ignore_ascii_case("file") {
                    c.push(x.clone());
                }
            }
            c
        }
        Select::CountStar => vec!["count".to_string()],
    };
    let mut out = Vec::new();
    for r in matched.into_iter().take(q.limit) {
        out.push(cols.iter().map(|c| cell_of(r, c)).collect());
    }
    let trunc = (matched_n as usize) > out.len();
    QueryResult {
        cols,
        rows: out,
        scanned: scanned_n,
        matched: matched_n,
        truncated_rows: trunc,
        ..QueryResult::default()
    }
}

/// Build a Row from a parsed record.
pub fn row_from_record(
    rec: &crate::engine::parser::ParsedRecord,
    line: u64,
    file: &str,
) -> Row {
    let mut fields = HashMap::new();
    for (k, v) in &rec.fields {
        fields.entry(k.to_ascii_lowercase()).or_insert_with(|| v.clone());
    }
    // Canonical shortcuts.
    if let Some(l) = &rec.level {
        fields.entry("level".to_string()).or_insert_with(|| l.clone());
    }
    if let Some(m) = &rec.msg {
        fields.entry("msg".to_string()).or_insert_with(|| m.clone());
    }
    Row {
        fields,
        line,
        file: file.to_string(),
        ts: rec.ts,
        ts_raw: rec.ts_raw.clone().unwrap_or_default(),
    }
}

/// Scan a Doc through the SQL engine (bounded, streaming-decode).
/// `scan_cap_lines`: max lines scanned (0 = min(total, 2_000_000)).
/// `custom`: optional compiled custom parser (wins over auto).
pub const SQL_DEFAULT_SCAN_CAP: u64 = 2_000_000;
pub fn run_on_doc(
    doc: &mut crate::engine::Doc,
    custom: Option<&regex::Regex>,
    q: &Query,
    scan_cap_lines: u64,
    extra_cols: &[String],
) -> QueryResult {
    let total = if doc.index.complete {
        doc.index.total_lines
    } else {
        doc.line_count_estimate()
    };
    let cap = if scan_cap_lines == 0 {
        total.min(SQL_DEFAULT_SCAN_CAP)
    } else {
        total.min(scan_cap_lines)
    };
    let file = doc.file_name.clone();
    // Decode streaming; grouped queries aggregate into `groups` (memory
    // guard), detail queries keep at most `limit` rows.
    use std::collections::HashMap as Map;
    let mut groups: Map<Vec<String>, usize> = Map::new();
    let mut matched: u64 = 0;
    let mut detail: Vec<Row> = Vec::new();
    let grouped = !q.group_by.is_empty();
    for ln in 1..=cap {
        let Some((head, _, _)) = doc.get_line_head(ln, 4096) else {
            continue;
        };
        let rec = if let Some(re) = custom {
            match crate::engine::parser::parse_with(re, &head) {
                Some(r) => r,
                None => continue,
            }
        } else {
            match crate::engine::parser::auto_parse(&head) {
                Some(r) => r,
                None => {
                    // Unparsed lines still participate as {msg} so `~` works.
                    crate::engine::parser::ParsedRecord {
                        fields: vec![("msg".to_string(), head.clone())],
                        msg: Some(head.clone()),
                        ..Default::default()
                    }
                }
            }
        };
        let mut row = row_from_record(&rec, ln, &file);
        // Project requested extra custom columns into the row view later;
        // ensure keys exist (empty when absent).
        for c in extra_cols {
            row.fields.entry(c.to_ascii_lowercase()).or_default();
        }
        if !eval_expr(&q.where_expr, &row) {
            continue;
        }
        matched += 1;
        if grouped {
            let key: Vec<String> = q.group_by.iter().map(|c| cell_of(&row, c)).collect();
            *groups.entry(key).or_insert(0) += 1;
            if groups.len() > 5000 {
                break;
            }
        } else if detail.len() < q.limit {
            detail.push(row);
        }
    }
    let truncated_scan = cap < total;
    if q.select == Select::CountStar && !grouped {
        return QueryResult {
            cols: vec!["count".to_string()],
            rows: vec![vec![matched.to_string()]],
            scanned: cap,
            matched,
            truncated_scan,
            ..QueryResult::default()
        };
    }
    if grouped {
        let mut grows: Vec<(Vec<String>, usize)> = groups.into_iter().collect();
        match &q.order_by {
            Some((k, desc)) if !k.eq_ignore_ascii_case("count") => {
                let idx = q.group_by.iter().position(|c| c.eq_ignore_ascii_case(k));
                if let Some(ix) = idx {
                    grows.sort_by(|a, b| {
                        let o = a.0[ix].cmp(&b.0[ix]);
                        if *desc { o.reverse() } else { o }
                    });
                } else {
                    grows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
                }
            }
            Some((_, desc)) => {
                if *desc {
                    grows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
                } else {
                    grows.sort_by_key(|(_, c)| *c);
                }
            }
            None => grows.sort_by_key(|(_, c)| std::cmp::Reverse(*c)),
        }
        let mut cols: Vec<String> = q.group_by.clone();
        cols.push("count".to_string());
        let mut out = Vec::new();
        for (k, c) in grows.into_iter().take(q.limit) {
            let mut r = k;
            r.push(c.to_string());
            out.push(r);
        }
        let trunc = out.len() >= q.limit;
        return QueryResult {
            cols,
            rows: out,
            scanned: cap,
            matched,
            truncated_scan,
            truncated_rows: trunc,
            is_grouped: true,
        };
    }
    let cols: Vec<String> = match &q.select {
        Select::All => {
            let mut c = vec![
                "line".to_string(),
                "file".to_string(),
                "ts".to_string(),
                "level".to_string(),
                "msg".to_string(),
            ];
            for x in extra_cols {
                if !c.iter().any(|e| e.eq_ignore_ascii_case(x)) {
                    c.push(x.clone());
                }
            }
            c
        }
        Select::Fields(f) => {
            let mut c = vec!["line".to_string(), "file".to_string()];
            for x in f {
                if !x.eq_ignore_ascii_case("line") && !x.eq_ignore_ascii_case("file") {
                    c.push(x.clone());
                }
            }
            c
        }
        Select::CountStar => vec!["count".to_string()],
    };
    let out: Vec<Vec<String>> = detail.iter().map(|r| cols.iter().map(|c| cell_of(r, c)).collect()).collect();
    let trunc = matched as usize > out.len();
    QueryResult {
        cols,
        rows: out,
        scanned: cap,
        matched,
        truncated_scan,
        truncated_rows: trunc,
        ..QueryResult::default()
    }
}

/// CSV with minimal quoting (quote when needed, double inner quotes).
pub fn to_csv(cols: &[String], rows: &[Vec<String>]) -> String {
    fn cell(s: &str) -> String {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
    let mut out = String::new();
    out.push_str(&cols.iter().map(|c| cell(c)).collect::<Vec<_>>().join(","));
    out.push('\n');
    for r in rows {
        out.push_str(&r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

pub fn export_csv(path: &std::path::Path, res: &QueryResult) -> Result<(), String> {
    std::fs::write(path, to_csv(&res.cols, &res.rows))
        .map_err(|e| format!("Gagal menulis CSV: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(line: u64, level: &str, msg: &str, extra: &[(&str, &str)]) -> Row {
        let mut fields = HashMap::new();
        fields.insert("level".to_string(), level.to_string());
        fields.insert("msg".to_string(), msg.to_string());
        for (k, v) in extra {
            fields.insert(k.to_string(), v.to_string());
        }
        Row {
            fields,
            line,
            file: "a.log".to_string(),
            ts: None,
            ts_raw: "2026-09-04 10:00:01".to_string(),
        }
    }

    #[test]
    fn parse_select_where_group() {
        let q = parse_query("SELECT level, msg WHERE level = ERROR GROUP BY level ORDER BY count DESC LIMIT 10").unwrap();
        assert_eq!(q.group_by, vec!["level".to_string()]);
        assert_eq!(q.limit, 10);
        let w = parse_query("level ~ timeout AND line > 5").unwrap();
        assert!(matches!(w.select, Select::All));
    }

    #[test]
    fn where_eval() {
        let r = row(7, "ERROR", "timeout boom", &[]);
        assert!(eval_expr(&parse_query("level = error").unwrap().where_expr, &r));
        assert!(eval_expr(&parse_query("msg ~ TIMEOUT").unwrap().where_expr, &r));
        assert!(!eval_expr(&parse_query("level = WARN").unwrap().where_expr, &r));
        assert!(eval_expr(&parse_query("line >= 7 AND NOT level = WARN").unwrap().where_expr, &r));
        assert!(eval_expr(&parse_query("(level = WARN OR level = ERROR) AND line < 10").unwrap().where_expr, &r));
    }

    #[test]
    fn group_count() {
        let rows = vec![
            row(1, "ERROR", "a", &[]),
            row(2, "ERROR", "b", &[]),
            row(3, "WARN", "c", &[]),
        ];
        let q = parse_query("SELECT COUNT(*) GROUP BY level").unwrap();
        let res = execute(&rows, &q);
        assert!(res.is_grouped);
        assert_eq!(res.cols, vec!["level".to_string(), "count".to_string()]);
        assert_eq!(res.rows[0][1], "2");
    }

    #[test]
    fn csv_quotes() {
        let csv = to_csv(&["a".to_string(), "b".to_string()], &[vec!["x,y".to_string(), "q\"q".to_string()]]);
        assert_eq!(csv, "a,b\n\"x,y\",\"q\"\"q\"\n");
    }

    #[test]
    fn bad_query_errors_id() {
        assert!(parse_query("").is_err());
        assert!(parse_query("SELECT WHERE").is_err());
        assert!(parse_query("level = ").is_err());
    }
}
