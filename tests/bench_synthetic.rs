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

// ---------- heavy local-only benchmarks (ignored in CI) ----------

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
