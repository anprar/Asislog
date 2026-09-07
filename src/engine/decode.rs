// English comments: encoding detection and lossy decoding for visible lines only.

use encoding_rs::WINDOWS_1252;

/// Supported encodings for viewing.
/// Byte-oriented variants (everything except UTF-16) share `\n` scanning:
/// 0x0A is always LF in Shift_JIS/EUC/GB/Big5/125x/KOI8/8859 (never a trail
/// byte), so only UTF-16 needs the wide path in the indexer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8,
    Windows1252,
    Utf16Le,
    Utf16Be,
    ShiftJis,
    EucJp,
    EucKr,
    Gb18030,
    Big5,
    Windows1250,
    Windows1251,
    Koi8R,
    Iso88592,
}

impl Encoding {
    /// English: human readable label (UI shows Indonesian wrapper).
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Windows1252 => "Windows-1252",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::ShiftJis => "Shift_JIS",
            Encoding::EucJp => "EUC-JP",
            Encoding::EucKr => "EUC-KR",
            Encoding::Gb18030 => "GB18030",
            Encoding::Big5 => "Big5",
            Encoding::Windows1250 => "Windows-1250",
            Encoding::Windows1251 => "Windows-1251",
            Encoding::Koi8R => "KOI8-R",
            Encoding::Iso88592 => "ISO-8859-2",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf8",
            Encoding::Windows1252 => "windows1252",
            Encoding::Utf16Le => "utf16le",
            Encoding::Utf16Be => "utf16be",
            Encoding::ShiftJis => "shiftjis",
            Encoding::EucJp => "eucjp",
            Encoding::EucKr => "euckr",
            Encoding::Gb18030 => "gb18030",
            Encoding::Big5 => "big5",
            Encoding::Windows1250 => "windows1250",
            Encoding::Windows1251 => "windows1251",
            Encoding::Koi8R => "koi8r",
            Encoding::Iso88592 => "iso88592",
        }
    }

    pub fn from_key(s: &str) -> Option<Encoding> {
        match s {
            "utf8" => Some(Encoding::Utf8),
            "windows1252" => Some(Encoding::Windows1252),
            "utf16le" => Some(Encoding::Utf16Le),
            "utf16be" => Some(Encoding::Utf16Be),
            "shiftjis" | "shift_jis" => Some(Encoding::ShiftJis),
            "eucjp" | "euc-jp" => Some(Encoding::EucJp),
            "euckr" | "euc-kr" => Some(Encoding::EucKr),
            "gb18030" | "gbk" => Some(Encoding::Gb18030),
            "big5" => Some(Encoding::Big5),
            "windows1250" => Some(Encoding::Windows1250),
            "windows1251" => Some(Encoding::Windows1251),
            "koi8r" | "koi8-r" | "koi8u" | "koi8-u" => Some(Encoding::Koi8R),
            "iso88592" | "iso-8859-2" | "latin2" => Some(Encoding::Iso88592),
            _ => None,
        }
    }

    pub fn all() -> &'static [Encoding] {
        &[
            Encoding::Utf8,
            Encoding::Windows1252,
            Encoding::ShiftJis,
            Encoding::EucJp,
            Encoding::EucKr,
            Encoding::Gb18030,
            Encoding::Big5,
            Encoding::Windows1250,
            Encoding::Windows1251,
            Encoding::Koi8R,
            Encoding::Iso88592,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ]
    }

    /// True for two-byte encodings where newline is a 2-byte unit.
    pub fn is_wide(self) -> bool {
        matches!(self, Encoding::Utf16Le | Encoding::Utf16Be)
    }

    /// The underlying `encoding_rs` decoder for this view encoding.
    pub fn rs_encoding(self) -> &'static encoding_rs::Encoding {
        match self {
            Encoding::Utf8 => encoding_rs::UTF_8,
            Encoding::Windows1252 => encoding_rs::WINDOWS_1252,
            Encoding::Utf16Le => encoding_rs::UTF_16LE,
            Encoding::Utf16Be => encoding_rs::UTF_16BE,
            Encoding::ShiftJis => encoding_rs::SHIFT_JIS,
            Encoding::EucJp => encoding_rs::EUC_JP,
            Encoding::EucKr => encoding_rs::EUC_KR,
            Encoding::Gb18030 => encoding_rs::GB18030,
            Encoding::Big5 => encoding_rs::BIG5,
            Encoding::Windows1250 => encoding_rs::WINDOWS_1250,
            Encoding::Windows1251 => encoding_rs::WINDOWS_1251,
            Encoding::Koi8R => encoding_rs::KOI8_R,
            Encoding::Iso88592 => encoding_rs::ISO_8859_2,
        }
    }
}

/// Detect encoding: BOM first, then chardetng (Mozilla detector port)
/// over up to 64 KiB sample. Single-byte guesses map to Windows-1252 view.
/// Returns (encoding, bom_len).
pub fn detect_encoding(sample: &[u8]) -> (Encoding, usize) {
    if sample.len() >= 3 && sample[0] == 0xEF && sample[1] == 0xBB && sample[2] == 0xBF {
        return (Encoding::Utf8, 3);
    }
    if sample.len() >= 2 && sample[0] == 0xFF && sample[1] == 0xFE {
        return (Encoding::Utf16Le, 2);
    }
    if sample.len() >= 2 && sample[0] == 0xFE && sample[1] == 0xFF {
        return (Encoding::Utf16Be, 2);
    }
    let head_len = sample.len().min(64 * 1024);
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(&sample[..head_len], true);
    let enc = det.guess(None, chardetng::Utf8Detection::Allow);
    // Map the detector guess to a real view encoding instead of squashing
    // every single-byte guess into Windows-1252 (klogg parity for CJK +
    // Cyrillic + Central European logs).
    if enc == encoding_rs::UTF_8 {
        (Encoding::Utf8, 0)
    } else if enc == encoding_rs::UTF_16LE {
        (Encoding::Utf16Le, 0)
    } else if enc == encoding_rs::UTF_16BE {
        (Encoding::Utf16Be, 0)
    } else if enc == encoding_rs::SHIFT_JIS {
        (Encoding::ShiftJis, 0)
    } else if enc == encoding_rs::EUC_JP {
        (Encoding::EucJp, 0)
    } else if enc == encoding_rs::EUC_KR {
        (Encoding::EucKr, 0)
    } else if enc == encoding_rs::BIG5 {
        (Encoding::Big5, 0)
    } else if enc == encoding_rs::GB18030 || enc == encoding_rs::GBK {
        (Encoding::Gb18030, 0)
    } else if enc == encoding_rs::WINDOWS_1250 {
        (Encoding::Windows1250, 0)
    } else if enc == encoding_rs::WINDOWS_1251 {
        (Encoding::Windows1251, 0)
    } else if enc == encoding_rs::KOI8_R || enc == encoding_rs::KOI8_U {
        (Encoding::Koi8R, 0)
    } else if enc == encoding_rs::ISO_8859_2 {
        (Encoding::Iso88592, 0)
    } else {
        // All other single-byte guesses (windows-125x, iso-8859-*) share
        // the Windows-1252 view, as before.
        (Encoding::Windows1252, 0)
    }
}

/// Decode a single line's raw bytes (without trailing \n / \r\n) lossily.
/// Never panics on invalid input.
pub fn decode_bytes(bytes: &[u8], enc: Encoding) -> String {
    match enc {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Windows1252 => {
            let (cow, _, _) = WINDOWS_1252.decode(bytes);
            cow.into_owned()
        }
        Encoding::Utf16Le => decode_utf16(bytes, true),
        Encoding::Utf16Be => decode_utf16(bytes, false),
        // CJK / Cyrillic / Central European: decode via encoding_rs
        // (lossy, replacement chars on invalid sequences — never panics).
        other => {
            let (cow, _, _) = other.rs_encoding().decode(bytes);
            cow.into_owned()
        }
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len() / 2 + 1);
    for c in bytes.chunks(2) {
        if c.len() == 2 {
            units.push(if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            });
        } else {
            // Odd trailing byte -> replacement char, never panic.
            units.push(0xFFFD);
        }
    }
    String::from_utf16_lossy(&units)
}

/// Strip trailing \r for display (handles \r\n files).
pub fn strip_cr(mut s: String) -> String {
    if s.ends_with('\r') {
        s.pop();
    }
    s
}

/// Format a slice of raw bytes into classic hex view rows (C-C5: hex peek):
/// [offset (8 hex)]: 16 hex bytes with mid-space | 16 ascii chars (dots for non-printable)
pub fn format_hex_lines(bytes: &[u8], start_offset: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (chunk_idx, chunk) in bytes.chunks(16).enumerate() {
        let offset = start_offset + chunk_idx * 16;
        let mut hex_part = String::with_capacity(50);
        let mut ascii_part = String::with_capacity(18);
        for (i, &b) in chunk.iter().enumerate() {
            if i == 8 {
                hex_part.push(' ');
            }
            use std::fmt::Write;
            let _ = write!(hex_part, "{:02x} ", b);
            if b.is_ascii_graphic() || b == b' ' {
                ascii_part.push(b as char);
            } else {
                ascii_part.push('.');
            }
        }
        if chunk.len() < 16 {
            let missing = 16 - chunk.len();
            for i in 0..missing {
                if chunk.len() + i == 8 {
                    hex_part.push(' ');
                }
                hex_part.push_str("   ");
            }
        }
        out.push(format!("{:08x}: {} |{}|", offset, hex_part, ascii_part));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_lines_format() {
        let raw = b"Hello, World!\x00\x01\x02";
        let lines = format_hex_lines(raw, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("00000000: 48 65 6c 6c 6f 2c 20 57"));
        assert!(lines[0].ends_with("|Hello, World!...|"));
    }

    #[test]
    fn bom_detection() {
        assert_eq!(detect_encoding(&[0xEF, 0xBB, 0xBF, b'a']), (Encoding::Utf8, 3));
        assert_eq!(detect_encoding(&[0xFF, 0xFE, 0x00, 0x00]), (Encoding::Utf16Le, 2));
        assert_eq!(detect_encoding(&[0xFE, 0xFF, 0x00, 0x00]), (Encoding::Utf16Be, 2));
    }

    #[test]
    fn utf8_lossy_never_panics() {
        let s = decode_bytes(&[0xFF, 0xFE, b'a'], Encoding::Utf8);
        assert!(s.contains('\u{FFFD}'));
    }

    #[test]
    fn windows1252_decode() {
        // 0xE9 in windows-1252 is e-acute.
        let s = decode_bytes(&[0xE9], Encoding::Windows1252);
        assert_eq!(s, "é");
    }

    #[test]
    fn utf16le_decode() {
        // "Hi" in UTF-16LE
        let raw = [0x48, 0x00, 0x69, 0x00];
        assert_eq!(decode_bytes(&raw, Encoding::Utf16Le), "Hi");
    }

    #[test]
    fn detector_picks_sides() {
        // Indonesian UTF-8 prose.
        let id = "Laporan transaksi selesai diproses tanpa galat pada sistem pendukung operasional."
            .repeat(40);
        assert_eq!(detect_encoding(id.as_bytes()).0, Encoding::Utf8);
        // Windows-1252 bytes (invalid UTF-8): café résumé naïve repeated.
        let mut latin = Vec::new();
        for _ in 0..200 {
            latin.extend_from_slice(b"caf\xe9 r\xe9sum\xe9 na\xefve \xfc \xdf\n");
        }
        assert_eq!(detect_encoding(&latin).0, Encoding::Windows1252);
    }

    /// Roundtrip each CJK/Cyrillic/CE encoding through encoding_rs itself:
    /// deterministic regardless of what chardetng guesses.
    #[test]
    fn cjk_cyrillic_roundtrip() {
        let cases: &[(Encoding, &str)] = &[
            (Encoding::ShiftJis, "あいうエラー"),
            (Encoding::EucJp, "あいうエラー"),
            (Encoding::EucKr, "한글 오류"),
            (Encoding::Gb18030, "中文错误"),
            (Encoding::Big5, "中文錯誤"),
            (Encoding::Windows1250, "Błąd zażółć"),
            (Encoding::Windows1251, "Ошибка Ж123"),
            (Encoding::Koi8R, "Ошибка Ж123"),
            (Encoding::Iso88592, "Chyba žluť"),
        ];
        for (enc, text) in cases {
            let (cow, _, had_errors) = enc.rs_encoding().encode(text);
            assert!(!had_errors, "fixture must encode in {:?}", enc);
            assert_eq!(decode_bytes(&cow, *enc), *text, "roundtrip {:?}", enc);
            assert!(!enc.is_wide(), "{:?} must stay byte-oriented", enc);
        }
    }

    #[test]
    fn encoding_keys_roundtrip() {
        for e in Encoding::all() {
            assert_eq!(Encoding::from_key(e.key()), Some(*e), "key {}", e.key());
            assert!(!e.label().is_empty());
        }
        // Legacy + alias keys keep loading old sessions.
        assert_eq!(Encoding::from_key("gbk"), Some(Encoding::Gb18030));
        assert_eq!(Encoding::from_key("shift_jis"), Some(Encoding::ShiftJis));
        assert_eq!(Encoding::from_key("latin2"), Some(Encoding::Iso88592));
        assert_eq!(Encoding::from_key("bogus"), None);
        // 13 view encodings: the original 4 plus 9 new ones.
        assert_eq!(Encoding::all().len(), 13);
    }
}
