// English comments: Required-literal prefilter for the fancy (backtracking)
// regex path. A line that cannot contain a match is skipped BEFORE decoding
// and backtracking. Soundness (never skip a real match) is locked by an
// oracle test against `fancy_regex` itself; effectiveness (it must actually
// filter) is asserted on a selective lookbehind pattern.
//
// Why ASCII-only literals: byte-level case-insensitive matching folds A-Z
// exactly; Unicode folds (U+0130, U+017F, ...) could hide an ASCII literal
// from the byte scan while the Unicode-aware engine still matches. Guard:
// case-insensitive checks pass non-ASCII lines through unfiltered.

/// Max bytes of one line handed to the backtracking engine. Display already
/// caps at HEAD_BYTES; copy/export stay full. Bounds worst-case work per
/// line; matches past the cap are out of scope (same honesty rule as the
/// clipboard cap).
pub const FANCY_LINE_CAP: usize = 256 * 1024;
/// Longest literal runs win; extras are dropped (weaker filter = still sound).
const MAX_LITERALS: usize = 16;
/// Runs shorter than this pass nearly every line (wasted scan).
const MIN_LITERAL_LEN: usize = 2;
/// Recursion cap for nested groups (deeper = give up filtering, stay sound).
const MAX_DEPTH: usize = 32;

/// Extract required single literals (CNF with one clause per literal: a
/// matching line contains every returned literal, ASCII-folded when
/// case-insensitive). Empty = no usable filter (never "no match").
pub fn extract_required_literals(pattern: &str) -> Vec<String> {
    let b = pattern.as_bytes();
    let mut must = parse_top(b, &mut 0);
    // Longest first, dedupe, cap. Dropping literals only weakens the filter.
    must.sort_by_key(|s: &String| std::cmp::Reverse(s.len()));
    must.dedup();
    must.into_iter()
        .filter(|s| s.len() >= MIN_LITERAL_LEN)
        .take(MAX_LITERALS)
        .collect()
}

/// If `b[pos]` opens a valid quantifier `{n}`, `{n,}`, `{n,m}` (Rust/fancy
/// syntax: first number mandatory), return (bytes to consume, min repeats).
/// A lone `{x}` / `{,m}` is literal text, NOT a quantifier.
fn brace_quant(b: &[u8], pos: usize) -> Option<(usize, usize)> {
    if b.get(pos) != Some(&b'{') {
        return None;
    }
    let mut j = pos + 1;
    let mut n: usize = 0;
    let mut digits = 0;
    while j < b.len() && b[j].is_ascii_digit() {
        n = n.saturating_mul(10).saturating_add((b[j] - b'0') as usize);
        digits += 1;
        j += 1;
    }
    if digits == 0 {
        return None;
    }
    if j < b.len() && b[j] == b',' {
        j += 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
    }
    if j < b.len() && b[j] == b'}' {
        return Some((j + 1 - pos, n));
    }
    None
}

/// Top level: `|`-separated branches with no closing paren and no trailing
/// quantifier. A bare `a|b` requires only what EVERY branch requires.
fn parse_top(b: &[u8], pos: &mut usize) -> Vec<String> {
    let mut branches: Vec<Vec<String>> = Vec::new();
    loop {
        branches.push(parse_seq(b, pos, 0));
        if *pos < b.len() && b[*pos] == b'|' {
            *pos += 1;
            continue;
        }
        break;
    }
    intersect_all(branches)
}

/// Intersection across branches (a literal required by every branch is
/// required by the alternation). Single branch passes through.
fn intersect_all(mut branches: Vec<Vec<String>>) -> Vec<String> {
    if branches.len() == 1 {
        return branches.pop().unwrap();
    }
    let mut inter = branches.pop().unwrap();
    inter.retain(|lit| branches.iter().all(|br| br.contains(lit)));
    inter
}
fn parse_seq(b: &[u8], pos: &mut usize, depth: usize) -> Vec<String> {
    let mut must: Vec<String> = Vec::new();
    let mut run: Vec<u8> = Vec::new();
    // Flush the current run. A trailing ? * {0..} binds ONE atom (the last
    // char), so only the last char becomes optional.
    macro_rules! flush {
        () => {{
            if run.len() >= MIN_LITERAL_LEN {
                if let Ok(s) = std::str::from_utf8(&run) {
                    must.push(s.to_string());
                }
            }
            run.clear();
        }};
    }
    macro_rules! flush_quant {
        () => {{
            let mut optional = false;
            let mut consume = 0usize;
            if *pos < b.len() {
                match b[*pos] {
                    b'?' | b'*' => {
                        optional = true;
                        consume = 1;
                    }
                    b'{' => {
                        if let Some((len, n)) = brace_quant(b, *pos) {
                            optional = n == 0;
                            consume = len;
                        }
                    }
                    _ => {}
                }
            }
            if optional && !run.is_empty() {
                run.pop();
            }
            flush!();
            *pos += consume;
        }};
    }
    while *pos < b.len() {
        let c = b[*pos];
        match c {
            b'|' | b')' => break, // handled by caller
            b'\\' => {
                flush!();
                *pos += 1;
                if *pos < b.len() {
                    if b[*pos] == b'Q' {
                        // \Q...\E quoted literal: runs inside are required.
                        *pos += 1;
                        while *pos + 1 < b.len()
                            && !(b[*pos] == b'\\' && b[*pos + 1] == b'E')
                        {
                            if b[*pos].is_ascii_alphanumeric() {
                                run.push(b[*pos]);
                            } else {
                                flush!();
                            }
                            *pos += 1;
                        }
                        flush!();
                        *pos += 2; // skip \E (or run past end, harmless)
                    } else {
                        // \d \s \b \1 \. ... : never a required literal
                        // (a literal char after \ is handled by the next
                        // loop pass as a fresh run start — e.g. `\ ` ends
                        // nothing; `\Q` handled above).
                        *pos += 1;
                    }
                }
            }
            b'[' => {
                flush!();
                skip_class(b, pos);
            }
            b'(' => {
                flush!();
                *pos += 1;
                let grp = parse_group(b, pos, depth + 1);
                must.extend(grp);
            }
            b'^' | b'$' => {
                // Anchors constrain position, not content.
                *pos += 1;
            }
            b'.' | b'+' => {
                // Standalone: '.' matches anything, '+' repeats the previous
                // atom (already accounted). End any run.
                flush!();
                *pos += 1;
            }
            b'?' | b'*' => {
                // Lone quantifier: binds the previous atom (group / class /
                // escape), whose contribution was already decided. Skip.
                flush!();
                *pos += 1;
            }
            b'{' | b'}' => {
                // Lone '{n..}' after a class/escape = its quantifier: skip
                // the whole shape so digits never become fake runs. A lone
                // '{x}' is literal text: consume only '{'.
                if c == b'{' {
                    if let Some((len, _)) = brace_quant(b, *pos) {
                        flush!();
                        *pos += len;
                    } else {
                        flush!();
                        *pos += 1;
                    }
                } else {
                    flush!();
                    *pos += 1;
                }
            }
            _ if c.is_ascii_alphanumeric() => {
                run.push(c);
                *pos += 1;
                // If the run ends here (next is not alnum), apply quantifier.
                if *pos >= b.len() || !b[*pos].is_ascii_alphanumeric() {
                    // But '(' or '|' or ')' etc. end the run plainly;
                    // only ? * { strip the last char.
                if *pos < b.len()
                    && (b[*pos] == b'?' || b[*pos] == b'*' || b[*pos] == b'{')
                {
                    // flush_quant eats a trailing quantifier when present;
                    // when it eats nothing the char is literal (or an
                    // already-handled lone ?/*) — step over exactly one.
                    let before = *pos;
                    flush_quant!();
                    if *pos == before {
                        *pos += 1;
                    }
                } else {
                    flush!();
                }
                }
            }
            _ => {
                // Any other byte (space, punctuation) ends the run.
                flush!();
                *pos += 1;
            }
        }
        if depth > MAX_DEPTH {
            return Vec::new();
        }
    }
    flush!();
    must
}

/// Parse after `(`. Returns literals required by the group. Handles flags,
/// lookarounds (negative contents skipped), comments, verbs, and `|`
/// branches via intersection (a literal required by EVERY branch is
/// required by the group).
fn parse_group(b: &[u8], pos: &mut usize, depth: usize) -> Vec<String> {
    if depth > MAX_DEPTH {
        // Skip to matching ')' to keep the outer parse aligned.
        skip_balanced(b, pos);
        return Vec::new();
    }
    if *pos < b.len() && b[*pos] == b'?' {
        *pos += 1;
        if *pos >= b.len() {
            return Vec::new();
        }
        match b[*pos] {
            b'=' | b'<' if *pos + 1 < b.len() && b[*pos] == b'<' && b[*pos + 1] == b'!' => {
                // (?<!...) negative lookbehind: contents NOT required.
                *pos += 2;
                skip_balanced(b, pos);
                return Vec::new();
            }
            b'<' => {
                // (?<=...) positive lookbehind or (?<name>...) group.
                *pos += 1;
                if *pos < b.len() && b[*pos] == b'=' {
                    *pos += 1;
                    let inner = parse_seq(b, pos, depth);
                    expect_close(b, pos);
                    return inner; // lookbehind contents ARE required
                }
                // (?<name>...): skip the name (it never matches text),
                // then parse branches transparently.
                skip_group_name(b, pos);
                return parse_branches(b, pos, depth);
            }
            b'!' => {
                // (?!...) negative lookahead: contents NOT required.
                *pos += 1;
                skip_balanced(b, pos);
                return Vec::new();
            }
            b'=' => {
                // (?=...) positive lookahead: contents required.
                *pos += 1;
                let inner = parse_seq(b, pos, depth);
                expect_close(b, pos);
                return inner;
            }
            b'#' => {
                // (?#comment)
                while *pos < b.len() && b[*pos] != b')' {
                    *pos += 1;
                }
                expect_close(b, pos);
                return Vec::new();
            }
            b'P' => {
                // (?P<name>...) or (?P=name) backreference.
                *pos += 1;
                if *pos < b.len() && b[*pos] == b'<' {
                    *pos += 1;
                    skip_group_name(b, pos);
                    return parse_branches(b, pos, depth);
                }
                // (?P=name): backreference, nothing required.
                while *pos < b.len() && b[*pos] != b')' {
                    *pos += 1;
                }
                expect_close(b, pos);
                return Vec::new();
            }
            b'>' => {
                // (?>...) atomic: transparent for required literals.
                *pos += 1;
                return parse_branches(b, pos, depth);
            }
            b':' => {
                *pos += 1;
                return parse_branches(b, pos, depth);
            }
            _ => {
                // (?i) (?i:...) (?-i) flags: zero-width for our purpose.
                // Caller sniffs "(?i" separately to force CI matching.
                while *pos < b.len() && b[*pos] != b')' && b[*pos] != b':' {
                    *pos += 1;
                }
                if *pos < b.len() && b[*pos] == b':' {
                    *pos += 1;
                    return parse_branches(b, pos, depth);
                }
                expect_close(b, pos);
                return Vec::new();
            }
        }
    }
    parse_branches(b, pos, depth)
}

/// Parse group branches split by top-level `|`. Result = intersection of
/// branch requirements (sound: required by every branch = required).
fn parse_branches(b: &[u8], pos: &mut usize, depth: usize) -> Vec<String> {
    let mut branches: Vec<Vec<String>> = Vec::new();
    loop {
        branches.push(parse_seq(b, pos, depth));
        if *pos < b.len() && b[*pos] == b'|' {
            *pos += 1;
            continue;
        }
        break;
    }
    expect_close(b, pos);
    // Trailing quantifier on the GROUP applies to the whole atom:
    // ? * {0..} make it optional (contributes nothing); + {n} {n,} keep it.
    // A non-quantifier char is left for the caller.
    let optional = if *pos < b.len() {
        match b[*pos] {
            b'?' | b'*' => {
                *pos += 1;
                true
            }
            b'{' => match brace_quant(b, *pos) {
                Some((len, n)) => {
                    *pos += len;
                    n == 0
                }
                None => false, // literal '{', leave it
            },
            _ => false,
        }
    } else {
        false
    };
    if optional {
        return Vec::new();
    }
    intersect_all(branches)
}

/// Skip a group name after `(?<` / `(?P<` up to and including `>`.
/// Names never match text; parsing them as literals would be UNSOUND.
fn skip_group_name(b: &[u8], pos: &mut usize) {
    while *pos < b.len() && b[*pos] != b'>' {
        *pos += 1;
    }
    if *pos < b.len() {
        *pos += 1;
    }
}

fn expect_close(b: &[u8], pos: &mut usize) {
    if *pos < b.len() && b[*pos] == b')' {
        *pos += 1;
    }
}

/// Skip `[...]` class (handles `\]`, `[^]`, `[]...]`).
fn skip_class(b: &[u8], pos: &mut usize) {
    debug_assert!(b[*pos] == b'[');
    *pos += 1;
    if *pos < b.len() && b[*pos] == b'^' {
        *pos += 1;
    }
    if *pos < b.len() && b[*pos] == b']' {
        *pos += 1; // literal ] first
    }
    while *pos < b.len() {
        if b[*pos] == b'\\' {
            *pos += 2;
            continue;
        }
        if b[*pos] == b']' {
            *pos += 1;
            return;
        }
        *pos += 1;
    }
}

/// Skip to the matching `)` of the current group (depth-aware, escape-aware).
fn skip_balanced(b: &[u8], pos: &mut usize) {
    let mut depth = 0usize;
    while *pos < b.len() {
        match b[*pos] {
            b'\\' => *pos += 2,
            b'[' => {
                skip_class(b, pos);
                continue;
            }
            b'(' => {
                depth += 1;
                *pos += 1;
            }
            b')' => {
                if depth == 0 {
                    *pos += 1;
                    return;
                }
                depth -= 1;
                *pos += 1;
            }
            _ => *pos += 1,
        }
    }
}

/// Byte-level CNF prefilter: a line passes when it contains every literal.
/// Literals are ASCII-alnum runs, so byte matching is exact for
/// case-sensitive mode; case-insensitive mode folds ASCII only and passes
/// non-ASCII lines through unfiltered (Unicode folds could otherwise hide a
/// literal from the byte scan while the engine still matches).
#[derive(Clone, Debug, Default)]
pub struct FancyPrefilter {
    lits: Vec<Vec<u8>>,
    ac: Option<aho_corasick::AhoCorasick>,
    /// True = ASCII-folded matching (worker case-insensitive or inline (?i)).
    fold_ascii: bool,
}

impl FancyPrefilter {
    /// Build for `pattern`. `case_sensitive` = worker flag; inline `(?i)`
    /// forces case-insensitive matching (weaker filter = still sound).
    /// Empty (no literals) = passes everything.
    pub fn new(pattern: &str, case_sensitive: bool) -> Self {
        let ci = !case_sensitive || pattern.contains("(?i");
        let lits: Vec<Vec<u8>> = extract_required_literals(pattern)
            .into_iter()
            .map(|s| s.into_bytes())
            .collect();
        let ac = if lits.is_empty() {
            None
        } else {
            aho_corasick::AhoCorasick::builder()
                .ascii_case_insensitive(ci)
                .build(&lits)
                .ok()
        };
        Self { lits, ac, fold_ascii: ci }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ac.is_none()
    }

    pub fn len(&self) -> usize {
        self.lits.len()
    }

    /// Necessary-condition check: false = line cannot match, skip it.
    pub fn passes(&self, line: &[u8]) -> bool {
        let Some(ac) = &self.ac else {
            return true;
        };
        if !self.fold_ascii {
            // Exact bytes: every literal must occur.
            for lit in &self.lits {
                if memchr::memmem::find(line, lit).is_none() {
                    return false;
                }
            }
            return true;
        }
        // ASCII-folded: non-ASCII lines bypass (Unicode-fold safety gate).
        if !line.is_ascii() {
            return true;
        }
        // All-patterns-must-hit via bitmask (literals are few by construction).
        let need = self.lits.len().min(64);
        let mut seen = vec![false; need];
        let mut found = 0usize;
        for m in ac.find_iter(line) {
            let idx = m.pattern().as_usize();
            if idx < seen.len() && !seen[idx] {
                seen[idx] = true;
                found += 1;
                if found >= need {
                    return true;
                }
            }
        }
        found >= need
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must(pat: &str) -> Vec<String> {
        extract_required_literals(pat)
    }

    #[test]
    fn lookbehind_literals_required() {
        let m = must("(?<=ERROR).*timeout");
        assert!(m.contains(&"ERROR".to_string()), "got {:?}", m);
        assert!(m.contains(&"timeout".to_string()), "got {:?}", m);
    }

    #[test]
    fn negative_lookaround_skipped() {
        // Only "timeout" is required; "DEBUG" must NOT be required.
        let m = must("(?!.*DEBUG).*timeout");
        assert!(!m.iter().any(|s| s == "DEBUG"), "got {:?}", m);
        assert!(m.contains(&"timeout".to_string()), "got {:?}", m);
        let m2 = must("(?<!DEBUG)ERROR");
        assert!(!m2.iter().any(|s| s == "DEBUG"), "got {:?}", m2);
        assert!(m2.contains(&"ERROR".to_string()), "got {:?}", m2);
    }

    #[test]
    fn alternation_intersects() {
        // No single literal is guaranteed here.
        let m = must("(ERROR|FATAL): \\d+");
        assert!(!m.iter().any(|s| s == "ERROR" || s == "FATAL"), "got {:?}", m);
        // ...but a shared tail is.
        let m2 = must("(ERROR x|FATAL x) timeout");
        assert!(m2.contains(&"timeout".to_string()), "got {:?}", m2);
    }

    #[test]
    fn optional_quantifier_strips() {
        // "abc?" requires "ab", not "abc".
        assert_eq!(must("abc?"), vec!["ab".to_string()]);
        assert_eq!(must("abc*"), vec!["ab".to_string()]);
        assert_eq!(must("abc+"), vec!["abc".to_string()]);
        assert_eq!(must("abc{2}"), vec!["abc".to_string()]);
        assert_eq!(must("abc{0,2}"), vec!["ab".to_string()]);
        // Optional group contributes nothing.
        assert!(must("(ERROR)? timeout").contains(&"timeout".to_string()));
        assert!(!must("(ERROR)? timeout").iter().any(|s| s == "ERROR"));
        assert!(must("(ERROR)+ timeout").contains(&"ERROR".to_string()));
    }

    #[test]
    fn classes_escapes_quotes() {
        assert!(must("id=\\d+").contains(&"id".to_string()));
        assert!(!must("[ERROR]+").iter().any(|s| s == "ERROR"));
        assert!(must("\\QERROR.FATAL\\E.*x").contains(&"ERROR".to_string()));
        assert!(must("\\QERROR.FATAL\\E.*x").contains(&"FATAL".to_string()));
        // Anchors don't break runs.
        assert!(must("^ERROR$").contains(&"ERROR".to_string()));
        assert!(must("\\bERROR\\b").contains(&"ERROR".to_string()));
    }

    #[test]
    fn degenerate_patterns_filter_nothing() {
        assert!(must(".*").is_empty());
        assert!(must("(a|b)").is_empty()); // single chars < MIN len
        assert!(must("").is_empty());
        assert!(must("(?#comment)").is_empty());
    }

    #[test]
    fn prefilter_passes_and_skips() {
        let f = FancyPrefilter::new("(?<=ERROR).*timeout", true);
        assert!(!f.is_empty());
        assert!(f.passes(b"2026 ERROR timeout waiting"));
        assert!(!f.passes(b"2026 INFO all good"));
        // Missing one literal -> skip.
        assert!(!f.passes(b"2026 timeout waiting"));
        assert!(!f.passes(b"2026 ERROR all good"));
        // Case-insensitive folds ASCII, gates non-ASCII.
        let ci = FancyPrefilter::new("(?<=ERROR).*timeout", false);
        assert!(ci.passes("2026 error TIMEOUT x".as_bytes()));
        assert!(ci.passes("2026 errör timeout x".as_bytes())); // gate
        assert!(!ci.passes(b"2026 info ok"));
        // Empty filter passes everything.
        assert!(FancyPrefilter::disabled().passes(b""));
    }

    /// Load-bearing soundness proof: whenever the prefilter rejects a line,
    /// the real fancy engine must also reject it. If this fails, the
    /// prefilter is UNSOUND (drops real matches) — fix, don't weaken asserts.
    #[test]
    fn oracle_never_drops_real_matches() {
        let patterns = [
            "(?<=ERROR).*timeout",
            "(?<!DEBUG)ERROR",
            "(?!.*DEBUG).*ERROR.*",
            "(ERROR|FATAL).*timeout",
            "\\b\\w+Exception\\b.*at\\s+\\S+",
            "(?i)outofmemoryerror.*heap",
            "id=\\d+.*(timeout|refused)",
            "(?P<lvl>ERROR|WARN): (?P<msg>.*)",
            "a(?=b)c",
            "Caused by:\\s+\\S+",
            "(?:ERROR|Error)\\s+\\d+",
            "timeout(ing)?",
            "\\Qa.b\\E.*c",
            "^2026-09-04 \\d+ ERROR",
            "x{2,4}y+z?",
        ];
        let lines = [
            "2026-09-04 10:00:01 ERROR timeout waiting for DB id=7",
            "2026-09-04 10:00:01 ERROR all good here",
            "2026-09-04 10:00:01 INFO timeout waiting",
            "2026-09-04 10:00:01 DEBUG ERROR hidden",
            "2026-09-04 10:00:01 FATAL timeout now",
            "2026-09-04 10:00:01 NullPointerException at com.erp.Main",
            "2026-09-04 10:00:01 OutOfMemoryError heap exhausted",
            "2026-09-04 10:00:01 outofmemoryerror HEAP blown",
            "2026-09-04 10:00:01 ERROR: disk full",
            "2026-09-04 10:00:01 WARN: slow query",
            "2026-09-04 10:00:01 abc here",
            "2026-09-04 10:00:01 Caused by: java.io.IOException",
            "2026-09-04 10:00:01 Error 42 happened",
            "2026-09-04 10:00:01 timing out slowly",
            "2026-09-04 10:00:01 a.b seen c",
            "2026-09-04 10:00:01 xxy plus z",
            "2026-09-04 10:00:01 xyyy z",
            "",
            "short",
            "2026-09-04 10:00:01 errör timeout ünïcodé",
            "2026-09-04 10:00:01 ERROR timeout ünïcodé",
            "plain line without anything",
            "ERRORERRORERROR",
            "timeouttimeout",
        ];
        for pat in patterns {
            let mut fb = fancy_regex::RegexBuilder::new(pat);
            fb.case_insensitive(false);
            let Ok(re_cs) = fb.build() else { continue };
            let mut fb2 = fancy_regex::RegexBuilder::new(pat);
            fb2.case_insensitive(true);
            let Ok(re_ci) = fb2.build() else { continue };
            for cs in [true, false] {
                let pre = FancyPrefilter::new(pat, cs);
                let re = if cs { &re_cs } else { &re_ci };
                for line in lines {
                    if !pre.passes(line.as_bytes()) {
                        let hit = re.is_match(line).unwrap_or(false);
                        assert!(
                            !hit,
                            "UNSOUND: pattern {:?} (cs={}) prefilter skipped but engine matches: {:?}",
                            pat, cs, line
                        );
                    }
                }
            }
        }
    }

    /// The filter must also be USEFUL: a selective lookbehind over mixed
    /// lines must skip a clear majority (else this whole module is dead code).
    #[test]
    fn prefilter_is_effective() {
        let corpus = [
            "2026 INFO request ok id=1",
            "2026 INFO request ok id=2",
            "2026 WARN slow id=3",
            "2026 INFO request ok id=4",
            "2026 ERROR timeout waiting id=5",
            "2026 INFO request ok id=6",
            "2026 DEBUG noise",
            "2026 INFO request ok id=8",
        ];
        let pre = FancyPrefilter::new("(?<=ERROR).*timeout", true);
        let skipped = corpus.iter().filter(|l| !pre.passes(l.as_bytes())).count();
        assert!(
            skipped * 100 / corpus.len() >= 75,
            "only skipped {}/{}",
            skipped,
            corpus.len()
        );
    }

    /// Randomized oracle: deterministic PRNG builds pattern/line soup from
    /// real log tokens. Same invariant as above, thousands of combinations.
    /// Catches extractor bugs the hand-written battery misses.
    #[test]
    fn oracle_fuzz_never_drops_real_matches() {
        // xorshift64 (deterministic, no new deps).
        struct Rng(u64);
        impl Rng {
            fn next(&mut self, n: usize) -> usize {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                (self.0 % (n as u64)) as usize
            }
        }
        let mut rng = Rng(0x9E3779B97F4A7C15);
        const WORDS: &[&str] = &[
            "ERROR", "WARN", "INFO", "DEBUG", "timeout", "id", "abc", "xyz", "42", "at",
        ];
        const ATOMS: &[&str] = &[
            "ERROR", "timeout", "id=\\d+", "\\b\\w+\\b", "\\d+", "\\s+", ".", ".*", ".+",
            "[A-Z]+", "[^ ]+", "\\Qa.b\\E", "x?", "y*", "z+", "ab{2}", "q{0,2}",
        ];
        const WRAP: &[&str] = &["", "", "", "(?:", "(", "(?<=ERROR)", "(?!DEBUG)", "(?i)"];
        let mut checked = 0usize;
        let mut filtering = 0usize;
        for _ in 0..1500 {
            // 1-3 atoms joined, optionally wrapped / alternated.
            let n = 1 + rng.next(3);
            let mut pat = String::new();
            let w = WRAP[rng.next(WRAP.len())];
            pat.push_str(w);
            let needs_close = w.starts_with('(');
            for i in 0..n {
                if i > 0 {
                    match rng.next(4) {
                        0 => pat.push('|'),
                        1 => pat.push(' '),
                        _ => {}
                    }
                }
                pat.push_str(ATOMS[rng.next(ATOMS.len())]);
            }
            if needs_close {
                pat.push(')');
            }
            if rng.next(3) == 0 {
                pat.push_str(".*timeout");
            }
            // Random line from words.
            let mut line = String::new();
            for i in 0..(2 + rng.next(5)) {
                if i > 0 {
                    line.push(' ');
                }
                line.push_str(WORDS[rng.next(WORDS.len())]);
            }
            if rng.next(10) == 0 {
                line.push_str(" ünïcodé");
            }
            for cs in [true, false] {
                let mut fb = fancy_regex::RegexBuilder::new(&pat);
                fb.case_insensitive(!cs);
                let Ok(re) = fb.build() else { continue };
                let pre = FancyPrefilter::new(&pat, cs);
                if !pre.is_empty() {
                    filtering += 1;
                }
                if !pre.passes(line.as_bytes()) {
                    checked += 1;
                    assert!(
                        !re.is_match(&line).unwrap_or(false),
                        "FUZZ-UNSOUND: {:?} (cs={}) skipped but matches {:?}",
                        pat,
                        cs,
                        line
                    );
                }
            }
        }
        assert!(filtering > 100, "fuzz produced no filtering patterns");
    }
}
