// English comments: encoding detection and lossy decoding for visible lines only.

use encoding_rs::WINDOWS_1252;

/// Supported encodings for viewing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8,
    Windows1252,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    /// English: human readable label (UI shows Indonesian wrapper).
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Windows1252 => "Windows-1252",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf8",
            Encoding::Windows1252 => "windows1252",
            Encoding::Utf16Le => "utf16le",
            Encoding::Utf16Be => "utf16be",
        }
    }

    pub fn from_key(s: &str) -> Option<Encoding> {
        match s {
            "utf8" => Some(Encoding::Utf8),
            "windows1252" => Some(Encoding::Windows1252),
            "utf16le" => Some(Encoding::Utf16Le),
            "utf16be" => Some(Encoding::Utf16Be),
            _ => None,
        }
    }

    pub fn all() -> &'static [Encoding] {
        &[
            Encoding::Utf8,
            Encoding::Windows1252,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ]
    }

    /// True for two-byte encodings where newline is a 2-byte unit.
    pub fn is_wide(self) -> bool {
        matches!(self, Encoding::Utf16Le | Encoding::Utf16Be)
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
    if enc == encoding_rs::UTF_8 {
        (Encoding::Utf8, 0)
    } else if enc == encoding_rs::UTF_16LE {
        (Encoding::Utf16Le, 0)
    } else if enc == encoding_rs::UTF_16BE {
        (Encoding::Utf16Be, 0)
    } else {
        // All single-byte guesses (windows-125x, iso-8859-*) share one view.
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
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len() / 2 + 1);
    let mut chunks = bytes.chunks_exact(2);
    for c in &mut chunks {
        let v = if little_endian {
            u16::from_le_bytes([c[0], c[1]])
        } else {
            u16::from_be_bytes([c[0], c[1]])
        };
        units.push(v);
    }
    // Odd trailing byte -> replacement char, never panic.
    if !chunks.remainder().is_empty() {
        units.push(0xFFFD);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
