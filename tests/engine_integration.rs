// English comments: file-level integration tests for the engine.
// These exercise the public API across modules (index + filter + follow +
// search + sidecar) the way the UI workers use it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use asislog::engine::decode::Encoding;
use asislog::engine::filter::{line_matches, parse_filter};
use asislog::engine::follow::{check_follow, load_identity, FollowEvent};
use asislog::engine::index::{
    build_full, byte_offset_of_line, checkpoint_lookup, line_byte_range, load_sidecar,
    save_sidecar,
};
use asislog::engine::search::{find_literal, find_regex, is_stale};

fn catalina_line(i: u64) -> String {
    // Tomcat-style line with a stable timestamp prefix.
    let level = match i % 10 {
        0 => "ERROR",
        1 => "WARN",
        _ => "INFO",
    };
    format!(
        "2026-09-04 10:{:02}:{:02} {} OrderService - request id={} ok\n",
        (i / 60) % 60,
        i % 60,
        level,
        i
    )
}

// ---------- newline indexing ----------

#[test]
fn index_mixed_crlf_lf_and_missing_trailing_newline() {
    let data = b"l1\r\nl2\nl3\r\nlast-without-newline";
    let idx = build_full(data, Encoding::Utf8, 0);
    assert_eq!(idx.total_lines, 4, "checkpoints: {:?}", idx.checkpoints);
    for (line, want) in [(1u64, "l1"), (2, "l2"), (3, "l3"), (4, "last-without-newline")] {
        let (s, e) = line_byte_range(&data[..], &idx.checkpoints, line, Encoding::Utf8, 0)
            .unwrap_or_else(|| panic!("no range for line {}", line));
        let mut got = &data[s as usize..e as usize];
        if !got.is_empty() && got[got.len() - 1] == b'\r' {
            got = &got[..got.len() - 1];
        }
        assert_eq!(got, want.as_bytes(), "line {}", line);
    }
}

#[test]
fn index_empty_and_bom_only() {
    let idx = build_full(b"", Encoding::Utf8, 0);
    assert_eq!(idx.total_lines, 0);
    let bom = b"\xef\xbb\xbf";
    let idx = build_full(bom, Encoding::Utf8, bom.len());
    assert_eq!(idx.total_lines, 0);
}

#[test]
fn checkpoint_lookup_and_goto_line_on_many_lines() {
    // ~5000 lines x ~60 B = ~300 KB: crosses the 64 KiB checkpoint rule.
    let mut data = Vec::new();
    for i in 0..5000u64 {
        data.extend_from_slice(catalina_line(i).as_bytes());
    }
    let idx = build_full(&data, Encoding::Utf8, 0);
    assert_eq!(idx.total_lines, 5000);
    assert!(idx.checkpoints.len() > 1, "expected sparse checkpoints");
    for target in [1u64, 2, 1023, 1024, 1025, 2500, 4999, 5000] {
        let (cp_line, cp_byte) = checkpoint_lookup(&idx.checkpoints, target)
            .unwrap_or_else(|| panic!("no checkpoint for {}", target));
        assert!(cp_line <= target);
        let off = byte_offset_of_line(&data, &idx.checkpoints, target, Encoding::Utf8, 0)
            .unwrap_or_else(|| panic!("no offset for {}", target));
        assert!(off >= cp_byte);
        let (s, e) = line_byte_range(&data, &idx.checkpoints, target, Encoding::Utf8, 0).unwrap();
        assert_eq!(s, off);
        let text = std::str::from_utf8(&data[s as usize..e as usize]).unwrap();
        assert!(text.contains(&format!("id={}", target - 1)), "line {}", target);
    }
    assert!(checkpoint_lookup(&idx.checkpoints, 0).is_none());
}

// ---------- filter ----------

#[test]
fn filter_include_and_exclude_tokens() {
    let f = parse_filter("ERROR -DEBUG", false);
    assert!(line_matches("2026-09-04 ERROR boom", &f));
    assert!(!line_matches("2026-09-04 ERROR DEBUG noise", &f));
    assert!(!line_matches("2026-09-04 INFO ok", &f));
    let empty = parse_filter("", false);
    assert!(line_matches("anything", &empty));
}

#[test]
fn filter_json_field_predicate() {
    let f = parse_filter("level=ERROR -level=DEBUG", false);
    assert!(line_matches(r#"{"level":"ERROR","msg":"x"}"#, &f));
    assert!(!line_matches(r#"{"level":"DEBUG","msg":"x"}"#, &f));
    assert!(!line_matches("plain ERROR line", &f));
}

// ---------- follow: append vs truncate/rotate ----------

#[test]
fn follow_append_then_rotate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.log");
    // Head fingerprint covers the first 4096 bytes, so the append case needs
    // a file bigger than that (the realistic catalina.out case).
    let big: String = (0..500).map(|i| format!("line{:04} pad-pad-pad-pad-pad\n", i)).collect();
    assert!(big.len() > 4096);
    std::fs::write(&path, &big).unwrap();

    let id0 = load_identity(&path).unwrap();
    // Append: size grows, head identical -> Appended.
    std::fs::write(&path, format!("{}line-extra\n", big)).unwrap();
    let id1 = load_identity(&path).unwrap();
    match check_follow(id0.size, id0.head.as_deref(), &id1) {
        FollowEvent::Appended(_) => {}
        e => panic!("expected Appended, got {:?}", e),
    }
    // Truncate: size shrinks -> TruncatedOrRotated.
    std::fs::write(&path, "new\n").unwrap();
    let id2 = load_identity(&path).unwrap();
    assert_eq!(
        check_follow(id1.size, id1.head.as_deref(), &id2),
        FollowEvent::TruncatedOrRotated
    );
    // Same-size overwrite with different head -> rotation, not append.
    std::fs::write(&path, "line1\nline2\n").unwrap();
    let id3 = load_identity(&path).unwrap();
    std::fs::write(&path, "ZZZZZ\nZZZZZ\n").unwrap();
    assert_eq!(id3.size, load_identity(&path).unwrap().size);
    let id4 = load_identity(&path).unwrap();
    assert_eq!(
        check_follow(id3.size, id3.head.as_deref(), &id4),
        FollowEvent::TruncatedOrRotated
    );
    // Untouched file -> Unchanged.
    assert_eq!(
        check_follow(id4.size, id4.head.as_deref(), &id4),
        FollowEvent::Unchanged
    );
}

// ---------- search generation cancel ----------

#[test]
fn search_generation_stale() {
    let gen = Arc::new(AtomicU64::new(7));
    assert!(!is_stale(&gen, 7));
    gen.store(8, Ordering::Relaxed);
    assert!(is_stale(&gen, 7));
}

#[test]
fn search_literal_and_regex_agree_on_simple_needle() {
    let mut data = Vec::new();
    for i in 0..2000u64 {
        data.extend_from_slice(catalina_line(i).as_bytes());
    }
    let (hits, truncated) = find_literal(&data, b"ERROR", false, 100_000);
    assert!(!truncated);
    // Every 10th line is ERROR -> 200 hits.
    assert_eq!(hits.len(), 200);
    assert!(hits.windows(2).all(|w| w[0].line < w[1].line));
    let (rhits, _) = find_regex(&data, "ERROR", false, 100_000).unwrap();
    assert_eq!(rhits.len(), hits.len());
    assert!(find_regex(&data, "([a", false, 100).is_err());
}

// ---------- sidecar ----------

#[test]
fn sidecar_roundtrip_and_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("big.log");
    let mut data = Vec::new();
    for i in 0..3000u64 {
        data.extend_from_slice(catalina_line(i).as_bytes());
    }
    std::fs::write(&log, &data).unwrap();
    let md = std::fs::metadata(&log).unwrap();
    let mtime = md.modified().ok();
    let idx = build_full(&data, Encoding::Utf8, 0);
    let sidecar = dir.path().join("big.log.asisidx");
    save_sidecar(&sidecar, &idx, data.len() as u64, mtime).unwrap();
    let back = load_sidecar(&sidecar, data.len() as u64, mtime).expect("sidecar should load");
    assert_eq!(back.total_lines, idx.total_lines);
    assert_eq!(back.checkpoints, idx.checkpoints);
    // Size mismatch -> rebuild.
    assert!(load_sidecar(&sidecar, data.len() as u64 + 1, mtime).is_none());
}
