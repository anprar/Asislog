// English comments: open archives (zip/tar/tar.gz/gz) by extracting the
// most relevant text entry to temp, then viewing it like a plain file.
// Pure-Rust decoders only (portable, no system libraries).

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArchiveKind {
    Zip,
    TarGz,
    Tar,
    Gzip,
}

/// Detect by extension (case-insensitive).
pub fn detect(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else if name.ends_with(".gz") {
        Some(ArchiveKind::Gzip)
    } else {
        None
    }
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
    let mut d = std::env::temp_dir();
    d.push("asislog-arc");
    d.push(format!(
        "{}-{}",
        sanitize(stem),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_millis())
            .unwrap_or(0)
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
            Ok(OpenedFile {
                path: out.clone(),
                temp: Some(out),
                note: format!("Dari gzip: {}", path.display()),
            })
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
        ArchiveKind::Tar | ArchiveKind::TarGz => {
            let f = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka tar: {}", e))?;
            let mut cands: Vec<(String, u64)> = Vec::new();
            if kind == ArchiveKind::TarGz {
                let dec = flate2::read::GzDecoder::new(f);
                let mut ar = tar::Archive::new(dec);
                collect_tar(&mut ar, &mut cands)?;
            } else {
                let mut ar = tar::Archive::new(f);
                collect_tar(&mut ar, &mut cands)?;
            }
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
            // Second pass: extract the pick.
            let f2 = std::fs::File::open(path)
                .map_err(|e| format!("Gagal membuka tar: {}", e))?;
            let dir = temp_dir_for(&stem)?;
            let leaf = Path::new(&pick)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| String::from("isi.log"));
            let out = dir.join(sanitize(&leaf));
            let mut found = false;
            if kind == ArchiveKind::TarGz {
                let dec = flate2::read::GzDecoder::new(f2);
                let mut ar = tar::Archive::new(dec);
                found = extract_tar(&mut ar, &pick, &out)?;
            } else {
                let mut ar = tar::Archive::new(f2);
                found = extract_tar(&mut ar, &pick, &out)?;
            }
            if !found {
                return Err(String::from("Entri tar tidak ditemukan."));
            }
            Ok(OpenedFile { path: out.clone(), temp: Some(out), note })
        }
    }
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
            let low = name.to_ascii_lowercase();
            if [".png", ".jpg", ".exe", ".dll", ".so", ".bin", ".gz", ".zip"]
                .iter()
                .any(|x| low.ends_with(x))
            {
                continue;
            }
            out.push((name, e.size()));
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
}
