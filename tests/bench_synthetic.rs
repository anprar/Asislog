// English comments: honest synthetic benchmark harness for AsisLog.
// Default tests use a small (~20 MB) generated log so `cargo test` stays fast.
// Heavy 1 GB tests are #[ignore]d: run locally with
//   cargo test --release -- --ignored bench_1gb
// and paste the numbers into BENCHMARK.md. No performance claim without numbers.

use std::io::Write;
use std::time::Instant;

use asislog::engine::decode::Encoding;
use asislog::engine::index::build_full;
use asislog::engine::search::{find_literal, find_regex};

/// Deterministic catalina.out-like log. ~120 B/line, ERROR every 50 lines,
/// a Java stack trace every 500 lines (multiline `at ...` frames).
fn gen_log(path: &std::path::Path, bytes: u64) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut i: u64 = 0;
    let mut written: u64 = 0;
    while written < bytes {
        let line = if i % 500 == 499 {
            format!(
                "2026-09-04 10:00:{:02} ERROR OrderService - NullPointerException id={}\n\tat com.erp.OrderService.get({}).try(Unknown Source)\nCaused by: java.sql.SQLException: timeout id={}\n",
                i % 60, i, i, i
            )
        } else if i.is_multiple_of(50) {
            format!(
                "2026-09-04 10:00:{:02} ERROR OrderService - timeout on request id={} after 30000ms\n",
                i % 60, i
            )
        } else {
            format!(
                "2026-09-04 10:00:{:02} INFO  OrderService - request id={} user=andi total=125000 status=OK\n",
                i % 60, i
            )
        };
        f.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
        i += 1;
    }
    f.flush().unwrap();
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

#[test]
fn bench_index_20mb() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synth-20mb.log");
    gen_log(&path, 20 * 1_048_576);
    let data = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let idx = build_full(&data, Encoding::Utf8, 0);
    let dt = t.elapsed();
    eprintln!(
        "[bench] index {:.1} MB: {:.2}s ({:.1} MB/s), {} lines, {} checkpoints",
        mb(data.len() as u64),
        dt.as_secs_f64(),
        mb(data.len() as u64) / dt.as_secs_f64().max(1e-9),
        idx.total_lines,
        idx.checkpoints.len()
    );
    assert!(idx.total_lines > 100_000);
    assert!(dt.as_secs() < 120, "indexing 20 MB must not take minutes");
}

#[test]
fn bench_search_literal_20mb() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synth-20mb.log");
    gen_log(&path, 20 * 1_048_576);
    let data = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let (hits, truncated) = find_literal(&data, b"ERROR", false, 1_000_000);
    let dt = t.elapsed();
    eprintln!(
        "[bench] literal 'ERROR' in {:.1} MB: {:.2}s, {} hits (truncated={})",
        mb(data.len() as u64),
        dt.as_secs_f64(),
        hits.len(),
        truncated
    );
    assert!(!hits.is_empty());
    assert!(dt.as_secs() < 120);
}

#[test]
fn bench_search_regex_20mb() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synth-20mb.log");
    gen_log(&path, 20 * 1_048_576);
    let data = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let (hits, _) = find_regex(&data, "Exception|timeout", false, 1_000_000).unwrap();
    let dt = t.elapsed();
    eprintln!(
        "[bench] regex 'Exception|timeout' in {:.1} MB: {:.2}s, {} hits",
        mb(data.len() as u64),
        dt.as_secs_f64(),
        hits.len()
    );
    assert!(!hits.is_empty());
}

// ---------- archive open benchmark (ignored in CI) ----------

/// Measures `open_maybe_archive` (decode + best-entry pick) per format on
/// one shared synthetic log. Run locally:
///   cargo test --release -- --ignored bench_archives --nocapture
/// Debug builds work too; then numbers are a LOWER bound — label them so.
/// `ASISLOG_BENCH_KEEP=<dir>` keeps fixtures for neutral-tool comparison
/// (tar/Expand-Archive) instead of deleting them.
#[test]
#[ignore]
fn bench_archives_open() {
    use asislog::engine::archive::open_maybe_archive;

    // Keep-dir for neutral-tool comparison, else an auto-deleted tempdir.
    // (A static slot keeps the TempDir alive for the whole test.)
    static SLOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let keep_dir: Option<std::path::PathBuf> =
        std::env::var("ASISLOG_BENCH_KEEP").ok().map(std::path::PathBuf::from);
    if let Some(d) = &keep_dir {
        std::fs::create_dir_all(d).unwrap();
    }
    let dir: &std::path::Path = match &keep_dir {
        Some(d) => d,
        None => SLOT.get_or_init(|| tempfile::tempdir().unwrap()).path(),
    };

    let base = dir.join("synth-8mb.log");
    gen_log(&base, 8 * 1_048_576);
    let raw = std::fs::read(&base).unwrap();
    let raw_mb = mb(raw.len() as u64);
    eprintln!(
        "[bench-arch] raw log: {:.1} MB, {} lines",
        raw_mb,
        raw.iter().filter(|&&b| b == b'\n').count()
    );

    // Build one fixture per format from the same bytes (build time excluded
    // from open timings, except the two compress timers printed for context).
    let gz = dir.join("synth.log.gz");
    {
        let f = std::fs::File::create(&gz).unwrap();
        let mut e = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        e.write_all(&raw).unwrap();
        e.finish().unwrap();
    }
    let bz2 = dir.join("synth.log.bz2");
    {
        let f = std::fs::File::create(&bz2).unwrap();
        let mut e = bzip2::write::BzEncoder::new(f, bzip2::Compression::new(1));
        e.write_all(&raw).unwrap();
        e.finish().unwrap();
    }
    let xz = dir.join("synth.log.xz");
    {
        let f = std::fs::File::create(&xz).unwrap();
        let mut src: &[u8] = &raw;
        let mut w = f;
        let t = Instant::now();
        lzma_rs::xz_compress(&mut src, &mut w).unwrap();
        eprintln!("[bench-arch] fixture xz compress: {:.1}s", t.elapsed().as_secs_f64());
    }
    let zip = dir.join("synth.zip");
    {
        let f = std::fs::File::create(&zip).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        w.start_file("synth-8mb.log", opts).unwrap();
        w.write_all(&raw).unwrap();
        w.finish().unwrap();
    }
    let tgz = dir.join("synth.tar.gz");
    {
        let f = std::fs::File::create(&tgz).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        ar.append_path_with_name(&base, "synth-8mb.log").unwrap();
        ar.finish().unwrap();
    }
    let sevenz = dir.join("synth.7z");
    {
        let t = Instant::now();
        sevenz_rust::compress_to_path(&base, &sevenz).unwrap();
        eprintln!("[bench-arch] fixture 7z compress: {:.1}s", t.elapsed().as_secs_f64());
    }

    for (name, path) in [
        ("gz", gz),
        ("bz2", bz2),
        ("xz", xz),
        ("zip", zip),
        ("tar.gz", tgz),
        ("7z", sevenz),
    ] {
        let bytes = std::fs::metadata(&path).unwrap().len();
        let t = Instant::now();
        let opened = open_maybe_archive(&path).expect("archive opens");
        let dt = t.elapsed().as_secs_f64();
        let got = std::fs::read(&opened.path).expect("extracted readable");
        assert_eq!(got.len(), raw.len(), "{}: decoded size differs", name);
        assert_eq!(got, raw, "{}: decoded bytes differ", name);
        eprintln!(
            "[bench-arch] {:6} {:5.1} MB on disk -> {:.1} MB raw in {:6.2}s ({:5.1} MB/s raw) note={}",
            name,
            mb(bytes),
            raw_mb,
            dt,
            raw_mb / dt.max(1e-9),
            opened.note,
        );
        // Drop the extracted temp right away so the disk-spike window stays
        // visible per format (7z extracts everything before we pick one).
        if let Some(tmp) = opened.temp {
            let _ = std::fs::remove_file(&tmp);
            if let Some(parent) = tmp.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    // Genuine LZMA2 fixtures (the harness-built .xz above is framing-only:
    // lzma-rs's own xz_compress stores uncompressed). Pre-generate with a
    // real encoder (e.g. CPython lzma preset 6) as real-8mb.log.xz and
    // real-8mb.tar.xz next to the fixtures; skipped silently when absent.
    // synth.tar.bz2 (real bzip2 tarball) is picked up the same way.
    for (name, fname) in [
        ("xz-real", "real-8mb.log.xz"),
        ("txz-real", "real-8mb.tar.xz"),
        ("tbz2-real", "synth.tar.bz2"),
    ] {
        let path = dir.join(fname);
        if !path.exists() {
            eprintln!("[bench-arch] {:6} skipped (no {})", name, fname);
            continue;
        }
        let bytes = std::fs::metadata(&path).unwrap().len();
        let t = Instant::now();
        let opened = open_maybe_archive(&path).expect("real archive opens");
        let dt = t.elapsed().as_secs_f64();
        let got = std::fs::read(&opened.path).expect("extracted readable");
        assert_eq!(got, raw, "{}: decoded bytes differ", name);
        eprintln!(
            "[bench-arch] {:6} {:5.1} MB on disk -> {:.1} MB raw in {:6.2}s ({:5.1} MB/s raw) note={}",
            name,
            mb(bytes),
            raw_mb,
            dt,
            raw_mb / dt.max(1e-9),
            opened.note,
        );
        if let Some(tmp) = opened.temp {
            let _ = std::fs::remove_file(&tmp);
            if let Some(parent) = tmp.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

#[test]
#[ignore]
fn bench_1gb_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synth-1gb.log");
    gen_log(&path, 1024 * 1_048_576);
    let data = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let idx = build_full(&data, Encoding::Utf8, 0);
    let dt = t.elapsed();
    eprintln!(
        "[bench-1gb] index {:.1} MB: {:.2}s ({:.1} MB/s), {} lines",
        mb(data.len() as u64),
        dt.as_secs_f64(),
        mb(data.len() as u64) / dt.as_secs_f64().max(1e-9),
        idx.total_lines
    );
}

#[test]
#[ignore]
fn bench_1gb_search() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synth-1gb.log");
    gen_log(&path, 1024 * 1_048_576);
    let data = std::fs::read(&path).unwrap();
    for (name, pat) in [("literal", None), ("regex", Some("Exception|timeout"))] {
        let t = Instant::now();
        let n = match pat {
            None => find_literal(&data, b"ERROR", false, 1_000_000).0.len(),
            Some(p) => find_regex(&data, p, false, 1_000_000).unwrap().0.len(),
        };
        eprintln!(
            "[bench-1gb] {} search in {:.1} MB: {:.2}s, {} hits",
            name,
            mb(data.len() as u64),
            t.elapsed().as_secs_f64(),
            n
        );
    }
}
