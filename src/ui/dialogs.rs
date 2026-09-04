// English comments: goto dialog parsing (line / percent / timestamp).

/// Parsed goto target.
#[derive(Clone, Debug, PartialEq)]
pub enum GotoTarget {
    Line(u64),
    Percent(f64),
    Timestamp(String),
}

/// Parse Indonesian goto input:
/// - `12345` -> line
/// - `50%` / `50 %` -> percent
/// - `akhir` / `end` -> last line, `awal` / `start` -> first line
/// - otherwise treated as timestamp string when it looks like one,
///   else invalid.
pub fn parse_goto(input: &str) -> Result<GotoTarget, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err(String::from("Masukkan nomor baris, persen (mis. 50%), akhir, atau cap waktu."));
    }
    let low = t.to_lowercase();
    if low == "akhir" || low == "end" {
        return Ok(GotoTarget::Line(u64::MAX));
    }
    if low == "awal" || low == "start" {
        return Ok(GotoTarget::Line(1));
    }
    if let Some(num) = t.strip_suffix('%') {
        let n: f64 = num
            .trim()
            .replace(',', ".")
            .parse()
            .map_err(|_| String::from("Persen tidak valid. Contoh: 50%"))?;
        if !(0.0..=100.0).contains(&n) {
            return Err(String::from("Persen harus 0–100."));
        }
        return Ok(GotoTarget::Percent(n));
    }
    // Pure number (allow dot thousands like 1.842.291).
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let nondigit = t.chars().any(|c| !c.is_ascii_digit() && c != '.' && c != ' ' && c != ',');
    if !nondigit && !digits.is_empty() {
        let n: u64 = digits
            .parse()
            .map_err(|_| String::from("Nomor baris tidak valid."))?;
        if n < 1 {
            return Err(String::from("Nomor baris minimal 1."));
        }
        return Ok(GotoTarget::Line(n));
    }
    // Timestamp-looking? Must contain a digit and '-'/'/'/':'.
    if t.chars().any(|c| c.is_ascii_digit())
        && (t.contains('-') || t.contains('/') || t.contains(':'))
    {
        return Ok(GotoTarget::Timestamp(t.to_string()));
    }
    Err(String::from(
        "Tidak dikenali. Gunakan nomor baris, persen (50%), atau cap waktu.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_line_and_percent() {
        assert_eq!(parse_goto("1842291"), Ok(GotoTarget::Line(1842291)));
        assert_eq!(parse_goto("1.842.291"), Ok(GotoTarget::Line(1842291)));
        assert_eq!(parse_goto("50%"), Ok(GotoTarget::Percent(50.0)));
        assert_eq!(parse_goto("akhir"), Ok(GotoTarget::Line(u64::MAX)));
        assert_eq!(parse_goto("awal"), Ok(GotoTarget::Line(1)));
    }

    #[test]
    fn goto_timestamp() {
        match parse_goto("2026-09-03 13:41:02") {
            Ok(GotoTarget::Timestamp(_)) => {}
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn goto_invalid() {
        assert!(parse_goto("").is_err());
        assert!(parse_goto("abc").is_err());
    }
}
