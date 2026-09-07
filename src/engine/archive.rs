// English comments: open archives (zip/tar/tar.gz/gz/bz2/xz/7z and the
// tar+bz2 / tar+xz combos) by extracting the most relevant text entry to
// temp, then viewing it like a plain file.
// Pure-Rust decoders only (portable, no system libraries).

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArchiveKind {
    Zip,
    TarGz,
    Tar,
    Gzip,
    Bzip2,
    TarBz2,
    Xz,
    TarXz,
    SevenZ,
}

/// Detect by extension (case-insensitive), falling back to magic bytes so
/// misnamed/extensionless archives still open (klogg parity). Extension
/// wins when present: only the magic path cannot tell `.tar.gz` apart
/// from plain `.gz` (same gzip magic), so an extensionless tar.gz opens
/// as the raw tar stream instead — rename to `.tgz` for entry picking.
pub fn detect(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") || name.ends_with(".tbz") {
        Some(ArchiveKind::TarBz2)
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") || name.ends_with(".tlz") {
        Some(ArchiveKind::TarXz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else if name.ends_with(".gz") {
        Some(ArchiveKind::Gzip)
    } else if name.ends_with(".bz2") || name.ends_with(".bz") {
        Some(ArchiveKind::Bzip2)
    } else if name.ends_with(".xz") {
        Some(ArchiveKind::Xz)
    } else if name.ends_with(".7z") {
        Some(ArchiveKind::SevenZ)
    } else {
        detect_magic(path)
    }
}

/// Magic-byte sniffing for extensionless/misnamed files. Reads at most the
/// first 512 bytes (tar's ustar marker lives at offset 257).
fn detect_magic(path: &Path) -> Option<ArchiveKind> {
    let mut buf = [0u8; 512];
    let n = std::fs::File::open(path)
        .ok()
        .map(|mut f| {
            use std::io::Read;
            let mut total = 0;
            while total < buf.len() {
                match f.read(&mut buf[total..]) {
                    Ok(0) => break,
                    Ok(k) => total += k,
                    Err(_) => break,
                }
            }
            total
        })
        .unwrap_or(0);
    if n >= 4 && buf[0] == b'P' && buf[1] == b'K' && buf[2] == 3 && buf[3] == 4 {
        return Some(ArchiveKind::Zip);
    }
    if n >= 2 && buf[0] == 0x1f && buf[1] == 0x8b {
        return Some(ArchiveKind::Gzip);
    }
    if n >= 3 && buf[0] == b'B' && buf[1] == b'Z' && buf[2] == b'h' {
        return Some(ArchiveKind::Bzip2);
    }
    if n >= 6 && buf[0] == 0xFD && &buf[1..6] == b"7zXZ\0" {
        return Some(ArchiveKind::Xz);
    }
    if n >= 6 && &buf[0..6] == b"7z\xBC\xAF\x27\x1C" {
        return Some(ArchiveKind::SevenZ);
    }
    if n >= 262 && &buf[257..262] == b"ustar" {
        return Some(ArchiveKind::Tar);
    }
    None
}

pub struct OpenedFile {
    /// Real path to mmap (original, or extracted temp copy).
    pub path: PathBuf,
    /// Temp file to delete when the tab closes (None = original file).
    pub temp: Option<PathBuf>,
    /// Indonesian note for the status bar ("" when not an archive).
    pub note: String,
}

fn temp_dir_for(stem: &str) -> Result<PathBuf, String> {
    // Uniqueness must NOT rely on wall-clock millis alone: two tabs (or two
    // tests) extracting same-stem archives within one millisecond would
    // share a dir AND output filename and corrupt each other. pid + an
    // atomic sequence make collisions impossible in-process and across runs.
    static ARC_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut d = std::env::temp_dir();
    d.push("asislog-arc");
    d.push(format!(
        "{}-{}-{}-{}",
        sanitize(stem),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_millis())
            .unwrap_or(0),
        ARC_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&d).map_err(|e| format!("Gagal membuat temp: {}", e))?;
    Ok(d)
}

fn sanitize(s: &str) -> String {
    let o: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(60)
        .collect();
    if o.is_empty() {
        String::from("arsip")
    } else {
        o
    }
}

/// Rank text entries: preferred extensions first, then larger size.
fn entry_score(name: &str, size: u64) -> (u8, u64) {
    let low = name.to_ascii_lowercase();
    let ext = if [".log", ".txt", ".out", ".err", ".json", ".csv", ".sql"]
        .iter()
        .any(|e| low.ends_with(e))
    {
        0u8
    } else {
        1u8
    };
    (ext, size)
}

/// Open directly, or extract the best text entry to temp first.
pub fn open_maybe_archive(path: &Path) -> Result<OpenedFile, String> {
    let Some(kind) = detect(path) else {
        return Ok(OpenedFile { path: path.to_path_buf(), temp: None, note: String::new() });
    };
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("arsip"));
    match kind {
        ArchiveKind::Gzip => {
            let f = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka gzip: {}", e))?;
            let mut dec = flate2::read::GzDecoder::new(f);
            let dir = temp_dir_for(&stem)?;
            let out_name = stem.strip_suffix(".tar").unwrap_or(&stem);
            let out = dir.join(sanitize(out_name));
            let mut w = std::fs::File::create(&out)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            std::io::copy(&mut dec, &mut w)
                .map_err(|e| format!("Gagal mengekstrak gzip: {}", e))?;
            let note = format!("Dari gzip: {}", path.display());
            redirect_tar_if_needed(&out, &note)
        }
        ArchiveKind::Bzip2 => {
            let f = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka bz2: {}", e))?;
            let mut dec = bzip2::read::BzDecoder::new(f);
            let dir = temp_dir_for(&stem)?;
            // a.tar.bz2 -> file_stem "a.tar" -> strip to "a".
            let base = stem.strip_suffix(".tar").unwrap_or(&stem);
            let out = dir.join(sanitize(base));
            let mut w = std::fs::File::create(&out)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            std::io::copy(&mut dec, &mut w)
                .map_err(|e| format!("Bz2 tidak valid: {}", e))?;
            let note = format!("Dari bz2: {}", path.display());
            redirect_tar_if_needed(&out, &note)
        }
        ArchiveKind::Xz => {
            let f = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka xz: {}", e))?;
            let mut rdr = std::io::BufReader::new(f);
            let dir = temp_dir_for(&stem)?;
            let base = stem.strip_suffix(".tar").unwrap_or(&stem);
            let out = dir.join(sanitize(base));
            let mut w = std::fs::File::create(&out)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            lzma_rs::xz_decompress(&mut rdr, &mut w)
                .map_err(|e| format!("Xz tidak valid: {}", e))?;
            let note = format!("Dari xz: {}", path.display());
            redirect_tar_if_needed(&out, &note)
        }
        ArchiveKind::Zip => {
            let f = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka zip: {}", e))?;
            let mut arc =
                zip::ZipArchive::new(f).map_err(|e| format!("Zip tidak valid: {}", e))?;
            // Collect candidates (read-only pass first).
            let mut cands: Vec<(String, u64)> = Vec::new();
            for i in 0..arc.len() {
                let e = arc.by_index(i).map_err(|e| format!("Gagal membaca zip: {}", e))?;
                if !e.is_dir() {
                    cands.push((e.name().to_string(), e.size()));
                }
            }
            if cands.is_empty() {
                return Err(String::from("Zip kosong."));
            }
            cands.sort_by_key(|(n, s)| entry_score(n, *s));
            // entry_score ranks (0=preferred) ascending, but larger size should
            // win within a rank: re-sort stably by (ext, Reverse(size)).
            cands.sort_by(|a, b| {
                entry_score(&a.0, a.1)
                    .0
                    .cmp(&entry_score(&b.0, b.1).0)
                    .then(b.1.cmp(&a.1))
            });
            let (pick, _) = cands[0].clone();
            let note = format!("Dari zip: {} ({} entri)", pick, cands.len());
            let mut entry = arc
                .by_name(&pick)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            let dir = temp_dir_for(&stem)?;
            let leaf = Path::new(&pick)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| String::from("isi.log"));
            let out = dir.join(sanitize(&leaf));
            let mut w = std::fs::File::create(&out)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            std::io::copy(&mut entry, &mut w)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            Ok(OpenedFile { path: out.clone(), temp: Some(out), note })
        }
        ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarBz2 => {
            open_tar_source(&stem, || {
                let f = std::fs::File::open(path)
                    .map_err(|e| format!("Gagal membuka tar: {}", e))?;
                if kind == ArchiveKind::TarGz {
                    Ok(Box::new(flate2::read::GzDecoder::new(f)) as Box<dyn std::io::Read>)
                } else if kind == ArchiveKind::TarBz2 {
                    Ok(Box::new(bzip2::read::BzDecoder::new(f)) as Box<dyn std::io::Read>)
                } else {
                    Ok(Box::new(f) as Box<dyn std::io::Read>)
                }
            })
        }
        ArchiveKind::TarXz => {
            // lzma-rs exposes no streaming Read adapter here, so decode to
            // a temp .tar first (disk-bounded), then reuse the plain flow.
            // The intermediate is deleted right after extraction.
            let f = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka xz: {}", e))?;
            let mut rdr = std::io::BufReader::new(f);
            let dir = temp_dir_for(&stem)?;
            let tmp_tar = dir.join(sanitize(&format!("{}.tar", stem)));
            {
                let mut w = std::fs::File::create(&tmp_tar)
                    .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
                lzma_rs::xz_decompress(&mut rdr, &mut w)
                    .map_err(|e| format!("Xz tidak valid: {}", e))?;
            }
            let opened = open_tar_source(&stem, || {
                let f = std::fs::File::open(&tmp_tar)
                    .map_err(|e| format!("Gagal membuka tar: {}", e))?;
                Ok(Box::new(f) as Box<dyn std::io::Read>)
            });
            let _ = std::fs::remove_file(&tmp_tar);
            opened
        }
        ArchiveKind::SevenZ => open_sevenz(path, &stem),
    }
}

/// Shared collect → rank → extract flow for tar sources (plain/gz/bz2).
fn open_tar_source(
    stem: &str,
    open: impl Fn() -> Result<Box<dyn std::io::Read>, String>,
) -> Result<OpenedFile, String> {
    let mut cands: Vec<(String, u64)> = Vec::new();
    collect_tar(&mut tar::Archive::new(open()?), &mut cands)?;
    if cands.is_empty() {
        return Err(String::from("Tar kosong / tanpa file teks."));
    }
    cands.sort_by(|a, b| {
        entry_score(&a.0, a.1)
            .0
            .cmp(&entry_score(&b.0, b.1).0)
            .then(b.1.cmp(&a.1))
    });
    let (pick, _) = cands[0].clone();
    let note = format!("Dari tar: {} ({} entri)", pick, cands.len());
    let dir = temp_dir_for(stem)?;
    let leaf = Path::new(&pick)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("isi.log"));
    let out = dir.join(sanitize(&leaf));
    if !extract_tar(&mut tar::Archive::new(open()?), &pick, &out)? {
        return Err(String::from("Entri tar tidak ditemukan."));
    }
    Ok(OpenedFile { path: out.clone(), temp: Some(out), note })
}

fn collect_tar<R: std::io::Read>(
    ar: &mut tar::Archive<R>,
    out: &mut Vec<(String, u64)>,
) -> Result<(), String> {
    let entries = ar.entries().map_err(|e| format!("Tar tidak valid: {}", e))?;
    for e in entries {
        let e = e.map_err(|e| format!("Gagal membaca tar: {}", e))?;
        if e.header().entry_type().is_file() {
            let name = e
                .path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Only plausible text entries (skip obvious binaries by extension).
            if is_skip_name(&name.to_ascii_lowercase()) {
                continue;
            }
            out.push((name, e.size()));
        }
    }
    Ok(())
}

/// Compressed-blob extensions never worth opening as the "best text entry".
fn is_skip_name(low: &str) -> bool {
    [
        ".png", ".jpg", ".exe", ".dll", ".so", ".bin", ".gz", ".zip", ".bz2",
        ".xz", ".7z", ".zst", ".lzma",
    ]
    .iter()
    .any(|x| low.ends_with(x))
}

/// A decoded single stream that quacks like tar (ustar at offset 257) is
/// really a misnamed `.tar.gz`/`.tar.bz2`/`.tar.xz`: run entry picking on
/// the decoded file instead of showing the raw tar blob.
fn is_tar_stream(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    if f.seek(SeekFrom::Start(257)).is_err() {
        return false;
    }
    let mut magic = [0u8; 5];
    matches!(f.read_exact(&mut magic), Ok(())) && &magic == b"ustar"
}

fn redirect_tar_if_needed(out: &Path, note: &str) -> Result<OpenedFile, String> {
    if !is_tar_stream(out) {
        return Ok(OpenedFile { path: out.to_path_buf(), temp: Some(out.to_path_buf()), note: note.to_string() });
    }
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("arsip"));
    let mut opened = open_tar_source(&stem, || {
        let f = std::fs::File::open(out).map_err(|e| format!("Gagal membuka tar: {}", e))?;
        Ok(Box::new(f) as Box<dyn std::io::Read>)
    })?;
    // The decoded blob served its purpose; the picked entry is tracked.
    // Note stays the tar one ("Dari tar: …"): chaining both hops would
    // break the "Dari {}: {}" display template with mixed languages.
    let _ = std::fs::remove_file(out);
    opened.temp = Some(opened.path.clone());
    Ok(opened)
}

/// 7z archives: extract everything to a scratch dir (sevenz-rust has no
/// entry-level streaming), rank text files like tar/zip, move the winner
/// to a flat temp dir and wipe the scratch dir.
fn open_sevenz(path: &Path, stem: &str) -> Result<OpenedFile, String> {
    let dest = temp_dir_for(&format!("{}-7z", stem))?;
    sevenz_rust::decompress_file(path, &dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&dest);
        let msg = format!("{e}");
        if msg.to_ascii_lowercase().contains("password") {
            return String::from("7z butuh kata sandi (tidak didukung).");
        }
        format!("7z tidak valid: {}", e)
    })?;
    let mut cands: Vec<(String, u64, PathBuf)> = Vec::new();
    walk_text_files(&dest, &mut cands)
        .inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dest);
        })?;
    if cands.is_empty() {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(String::from("7z kosong / tanpa file teks."));
    }
    cands.sort_by(|a, b| {
        entry_score(&a.0, a.1)
            .0
            .cmp(&entry_score(&b.0, b.1).0)
            .then(b.1.cmp(&a.1))
    });
    let (pick, _, full) = cands[0].clone();
    let note = format!("Dari 7z: {} ({} entri)", pick, cands.len());
    let dir = temp_dir_for(stem)?;
    let leaf = Path::new(&pick)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("isi.log"));
    let out = dir.join(sanitize(&leaf));
    // Move (same volume: rename) else copy, then wipe the scratch dir so
    // tab-close cleanup (one file + parent dir) stays sufficient.
    if std::fs::rename(&full, &out).is_err() {
        std::fs::copy(&full, &out).map_err(|e| format!("Gagal mengekstrak: {}", e))?;
    }
    let _ = std::fs::remove_dir_all(&dest);
    Ok(OpenedFile { path: out.clone(), temp: Some(out), note })
}

fn walk_text_files(dir: &Path, out: &mut Vec<(String, u64, PathBuf)>) -> Result<(), String> {
    walk_text_files_rel(dir, dir, out)
}

/// `display` is archive-relative ("logs/app.log"), so status notes stay
/// short and never leak temp paths; `full` is the real file to move.
fn walk_text_files_rel(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, u64, PathBuf)>,
) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("Gagal membaca 7z: {}", e))?;
    for e in rd {
        let e = e.map_err(|e| format!("Gagal membaca 7z: {}", e))?;
        let p = e.path();
        let ft = e.file_type().map_err(|e| format!("Gagal membaca 7z: {}", e))?;
        if ft.is_dir() {
            walk_text_files_rel(root, &p, out)?;
        } else if ft.is_file() {
            let display = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string_lossy().into_owned());
            if is_skip_name(&display.to_ascii_lowercase()) {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((display, size, p));
        }
    }
    Ok(())
}

fn extract_tar<R: std::io::Read>(
    ar: &mut tar::Archive<R>,
    pick: &str,
    out: &Path,
) -> Result<bool, String> {
    let entries = ar.entries().map_err(|e| format!("Tar tidak valid: {}", e))?;
    for e in entries {
        let mut e = e.map_err(|e| format!("Gagal membaca tar: {}", e))?;
        let name = e
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == pick {
            let mut w = std::fs::File::create(out)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            std::io::copy(&mut e, &mut w)
                .map_err(|e| format!("Gagal mengekstrak: {}", e))?;
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detect_by_extension() {
        assert_eq!(detect(Path::new("a.ZIP")), Some(ArchiveKind::Zip));
        assert_eq!(detect(Path::new("a.tar.gz")), Some(ArchiveKind::TarGz));
        assert_eq!(detect(Path::new("a.tgz")), Some(ArchiveKind::TarGz));
        assert_eq!(detect(Path::new("a.tar")), Some(ArchiveKind::Tar));
        assert_eq!(detect(Path::new("a.gz")), Some(ArchiveKind::Gzip));
        assert_eq!(detect(Path::new("a.log")), None);
    }

    #[test]
    fn zip_picks_largest_log() {
        let dir = std::env::temp_dir().join(format!("asislog-arctest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zp = dir.join("t.zip");
        {
            let f = std::fs::File::create(&zp).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("readme.txt", opts).unwrap();
            w.write_all(b"hi").unwrap();
            w.start_file("big/catalina.out", opts).unwrap();
            w.write_all(b"line1\nline2\n").unwrap();
            w.finish().unwrap();
        }
        let opened = open_maybe_archive(&zp).unwrap();
        assert!(opened.temp.is_some());
        let text = std::fs::read_to_string(&opened.path).unwrap();
        assert!(text.contains("line1"));
        assert!(opened.note.contains("catalina.out"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gzip_roundtrip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let dir = std::env::temp_dir().join(format!("asislog-gztest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gp = dir.join("a.log.gz");
        {
            let f = std::fs::File::create(&gp).unwrap();
            let mut e = GzEncoder::new(f, Compression::fast());
            e.write_all(b"hello log\n").unwrap();
            e.finish().unwrap();
        }
        let opened = open_maybe_archive(&gp).unwrap();
        assert_eq!(
            std::fs::read_to_string(&opened.path).unwrap(),
            "hello log\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bz2_roundtrip() {
        use bzip2::Compression;
        let dir = std::env::temp_dir().join(format!("asislog-bz2test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bp = dir.join("a.log.bz2");
        {
            let f = std::fs::File::create(&bp).unwrap();
            let mut e = bzip2::write::BzEncoder::new(f, Compression::new(1));
            e.write_all(b"hello bz2\n").unwrap();
            e.finish().unwrap();
        }
        let opened = open_maybe_archive(&bp).unwrap();
        assert_eq!(
            std::fs::read_to_string(&opened.path).unwrap(),
            "hello bz2\n"
        );
        assert!(opened.note.contains("bz2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn xz_roundtrip() {
        let dir = std::env::temp_dir().join(format!("asislog-xztest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let xp = dir.join("a.log.xz");
        {
            let f = std::fs::File::create(&xp).unwrap();
            let mut src: &[u8] = b"hello xz\n";
            let mut w = f;
            lzma_rs::xz_compress(&mut src, &mut w).unwrap();
        }
        let opened = open_maybe_archive(&xp).unwrap();
        assert_eq!(
            std::fs::read_to_string(&opened.path).unwrap(),
            "hello xz\n"
        );
        assert!(opened.note.contains("xz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_bz2_and_tar_xz_pick_log() {
        // Build a small tar in memory, then wrap it both ways.
        fn make_tar_bytes() -> Vec<u8> {
            let mut buf = Vec::new();
            {
                let mut ar = tar::Builder::new(&mut buf);
                let mut h = tar::Header::new_gnu();
                h.set_size(12);
                h.set_mode(0o644);
                h.set_cksum();
                ar.append_data(&mut h, "big/catalina.out", &b"line1\nline2\n"[..]).unwrap();
                let mut h2 = tar::Header::new_gnu();
                h2.set_size(2);
                h2.set_mode(0o644);
                h2.set_cksum();
                ar.append_data(&mut h2, "readme.txt", &b"hi"[..]).unwrap();
                ar.finish().unwrap();
            }
            buf
        }
        let dir = std::env::temp_dir().join(format!("asislog-tartest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tar_bytes = make_tar_bytes();
        // .tar.bz2 via streaming encoder.
        let tbz = dir.join("a.tar.bz2");
        {
            use bzip2::Compression;
            let f = std::fs::File::create(&tbz).unwrap();
            let mut e = bzip2::write::BzEncoder::new(f, Compression::new(1));
            e.write_all(&tar_bytes).unwrap();
            e.finish().unwrap();
        }
        let opened = open_maybe_archive(&tbz).unwrap();
        assert!(std::fs::read_to_string(&opened.path).unwrap().contains("line1"));
        assert!(opened.note.contains("catalina.out"));
        // .txz via temp-file decode path.
        let txz = dir.join("a.txz");
        {
            let f = std::fs::File::create(&txz).unwrap();
            let mut src: &[u8] = &tar_bytes;
            let mut w = f;
            lzma_rs::xz_compress(&mut src, &mut w).unwrap();
        }
        let opened = open_maybe_archive(&txz).unwrap();
        assert!(std::fs::read_to_string(&opened.path).unwrap().contains("line1"));
        assert!(opened.note.contains("catalina.out"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sevenz_roundtrip_and_invalid() {
        let dir = std::env::temp_dir().join(format!("asislog-7ztest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("app.log");
        std::fs::write(&src, "hello 7z\n").unwrap();
        let zp = dir.join("a.7z");
        sevenz_rust::compress_to_path(&src, &zp).unwrap();
        let opened = open_maybe_archive(&zp).unwrap();
        assert_eq!(
            std::fs::read_to_string(&opened.path).unwrap(),
            "hello 7z\n"
        );
        assert!(opened.note.contains("7z"));
        // Garbage with a .7z name: honest error, never a panic.
        let bad = dir.join("bad.7z");
        std::fs::write(&bad, b"not a 7z file at all!!!!!!!!").unwrap();
        assert!(open_maybe_archive(&bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }


    // Real LZMA2/xz bytes (preset 6, generated once with CPython lzma).
    // lzma-rs's own xz_compress is a *dumb* stored-only encoder for
    // decoder testing, so a fixture built by it proves framing only —
    // THESE blobs prove the read path on genuine LZMA2 ranges.
    const REAL_XZ: &[u8] = &[253, 55, 122, 88, 90, 0, 0, 4, 230, 214, 180, 70, 2, 0, 33, 1, 22, 0, 0, 0, 116, 47, 229, 163, 224, 10, 45, 0, 198, 93, 0, 25, 12, 2, 146, 178, 169, 82, 53, 123, 9, 153, 233, 7, 110, 234, 190, 3, 19, 133, 249, 97, 11, 99, 21, 64, 1, 154, 232, 88, 127, 20, 186, 214, 107, 243, 84, 17, 236, 57, 148, 250, 105, 250, 160, 162, 148, 173, 97, 130, 46, 100, 238, 128, 208, 56, 209, 99, 158, 113, 3, 41, 191, 68, 81, 216, 8, 30, 161, 57, 1, 3, 45, 55, 43, 18, 245, 121, 103, 29, 55, 48, 121, 49, 50, 7, 54, 37, 224, 129, 27, 72, 128, 227, 127, 46, 144, 32, 28, 195, 71, 115, 253, 192, 40, 116, 227, 152, 118, 43, 221, 87, 122, 145, 129, 99, 200, 76, 61, 69, 132, 151, 182, 122, 89, 183, 237, 156, 188, 115, 71, 44, 1, 129, 70, 37, 170, 106, 211, 201, 101, 141, 5, 74, 231, 214, 188, 221, 15, 223, 5, 68, 247, 103, 232, 18, 239, 165, 143, 6, 16, 188, 2, 230, 140, 136, 142, 7, 166, 188, 222, 75, 61, 23, 226, 42, 33, 50, 24, 12, 4, 254, 164, 182, 199, 36, 118, 228, 156, 202, 173, 21, 109, 51, 114, 70, 205, 125, 128, 0, 0, 0, 164, 251, 95, 237, 23, 107, 33, 166, 0, 1, 226, 1, 174, 20, 0, 0, 126, 34, 34, 19, 177, 196, 103, 251, 2, 0, 0, 0, 0, 4, 89, 90];
    const REAL_TXZ: &[u8] = &[253, 55, 122, 88, 90, 0, 0, 4, 230, 214, 180, 70, 2, 0, 33, 1, 22, 0, 0, 0, 116, 47, 229, 163, 224, 39, 255, 0, 138, 93, 0, 49, 26, 73, 21, 156, 34, 98, 32, 2, 96, 209, 114, 4, 231, 83, 239, 238, 235, 47, 20, 149, 144, 172, 112, 166, 43, 3, 184, 58, 66, 159, 238, 159, 231, 91, 132, 74, 186, 195, 237, 10, 0, 231, 143, 254, 113, 255, 80, 72, 173, 6, 214, 22, 41, 228, 30, 177, 97, 132, 15, 63, 15, 26, 58, 140, 172, 172, 41, 113, 84, 166, 252, 68, 8, 162, 204, 214, 26, 73, 167, 106, 98, 113, 166, 183, 241, 159, 141, 46, 47, 126, 105, 171, 19, 188, 18, 31, 160, 176, 160, 12, 101, 8, 152, 69, 190, 152, 195, 8, 252, 134, 160, 164, 8, 83, 94, 235, 140, 88, 108, 12, 26, 255, 12, 164, 53, 172, 55, 104, 249, 140, 100, 4, 32, 114, 132, 77, 71, 0, 0, 0, 255, 168, 159, 193, 4, 243, 217, 214, 0, 1, 166, 1, 128, 80, 0, 0, 21, 151, 149, 176, 177, 196, 103, 251, 2, 0, 0, 0, 0, 4, 89, 90];

    fn real_xz_log_text() -> String {
        let mut s = String::new();
        for i in 0..30 {
            if i % 10 == 0 {
                s.push_str(&format!(
                    "2026-09-04 10:00:01 ERROR OrderService - timeout on request id={} after 30000ms\n", i));
            } else {
                s.push_str(&format!(
                    "2026-09-04 10:00:01 INFO  OrderService - request id={} user=andi total=125000 status=OK\n", i));
            }
        }
        s
    }

    #[test]
    fn xz_real_lzma2_decodes() {
        let dir = std::env::temp_dir().join(format!("asislog-xzreal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let xp = dir.join("real.log.xz");
        std::fs::write(&xp, REAL_XZ).unwrap();
        let opened = open_maybe_archive(&xp).unwrap();
        assert_eq!(std::fs::read_to_string(&opened.path).unwrap(), real_xz_log_text());
        assert!(opened.note.contains("xz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_xz_real_decodes_and_picks_log() {
        let dir = std::env::temp_dir().join(format!("asislog-txzreal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tp = dir.join("real.txz");
        std::fs::write(&tp, REAL_TXZ).unwrap();
        let opened = open_maybe_archive(&tp).unwrap();
        assert_eq!(std::fs::read_to_string(&opened.path).unwrap(), "line1\nline2\n");
        assert!(opened.note.contains("catalina.out"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_magic_and_new_extensions() {
        assert_eq!(detect(Path::new("a.BZ2")), Some(ArchiveKind::Bzip2));
        assert_eq!(detect(Path::new("a.tbz2")), Some(ArchiveKind::TarBz2));
        assert_eq!(detect(Path::new("a.txz")), Some(ArchiveKind::TarXz));
        assert_eq!(detect(Path::new("a.7z")), Some(ArchiveKind::SevenZ));
        assert_eq!(detect(Path::new("a.XZ")), Some(ArchiveKind::Xz));
        // Magic fallback for extensionless files.
        let dir = std::env::temp_dir().join(format!("asislog-magtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let noext = dir.join("mystery");
        std::fs::write(&noext, b"BZh91AY&SY definitely bz2 magic").unwrap();
        assert_eq!(detect(&noext), Some(ArchiveKind::Bzip2));
        std::fs::write(&noext, b"7z\xBC\xAF\x27\x1C....").unwrap();
        assert_eq!(detect(&noext), Some(ArchiveKind::SevenZ));
        std::fs::write(&noext, b"\xFD7zXZ\x00....").unwrap();
        assert_eq!(detect(&noext), Some(ArchiveKind::Xz));
        std::fs::write(&noext, b"plain log line\n").unwrap();
        assert_eq!(detect(&noext), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
