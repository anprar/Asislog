// English comments: CLI smoke tests for --version/--help across the four
// launch paths: piped stdout, cmd.exe pipe, redirected file, detached
// (null stdout, like double-click/Run dialog). All must exit 0; detached
// must not panic on the invalid console handle.

use std::process::{Command, Stdio};

fn exe() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_asislog"))
}

#[test]
fn version_piped() {
    let out = Command::new(exe())
        .arg("--version")
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("AsisLog"), "got: {}", text);
}

#[test]
fn help_piped() {
    let out = Command::new(exe())
        .arg("--help")
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Penggunaan"), "got: {}", text);
}

#[test]
fn version_redirected_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("v.txt");
    let f = std::fs::File::create(&log).unwrap();
    let status = Command::new(exe())
        .arg("--version")
        .stdout(Stdio::from(f))
        .status()
        .expect("spawn");
    assert!(status.success());
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("AsisLog"), "got: {}", text);
}

#[test]
fn version_detached_no_console() {
    // Stdout null ~= launched without console: must exit 0, not panic.
    let status = Command::new(exe())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn");
    assert!(status.success());
}

#[test]
#[cfg(windows)]
fn version_via_cmd_pipe() {
    // No manual quoting: Rust quotes the spaced path for CreateProcess;
    // `call` avoids cmd's quote-stripping rule.
    let out = Command::new("cmd")
        .args(["/C", "call", &exe().display().to_string(), "--version"])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("AsisLog"));
}

#[test]
fn cli_count_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sample.log");
    std::fs::write(&file, "Line 1\nLine 2\nLine 3\n").unwrap();

    let out = Command::new(exe())
        .args(["count", file.to_str().unwrap()])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Baris: 3"), "got: {}", text);
    assert!(text.contains("Ukuran:"), "got: {}", text);
}

#[test]
fn cli_grep_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sample.log");
    std::fs::write(&file, "2026-09-04 INFO start\n2026-09-04 ERROR disk full\n2026-09-04 INFO stop\n").unwrap();

    // 1. Found: exit code 0
    let out = Command::new(exe())
        .args(["grep", "ERROR", file.to_str().unwrap()])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ERROR disk full"), "got: {}", text);

    // 2. Line number flag (-n)
    let out_n = Command::new(exe())
        .args(["grep", "-n", "ERROR", file.to_str().unwrap()])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out_n.status.success());
    let text_n = String::from_utf8_lossy(&out_n.stdout);
    assert!(text_n.contains("2:2026-09-04 ERROR disk full"), "got: {}", text_n);

    // 3. Count only flag (-c)
    let out_c = Command::new(exe())
        .args(["grep", "-c", "ERROR", file.to_str().unwrap()])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert!(out_c.status.success());
    let text_c = String::from_utf8_lossy(&out_c.stdout);
    assert_eq!(text_c.trim(), "1");

    // 4. Not found: exit code 1
    let out_none = Command::new(exe())
        .args(["grep", "FATAL", file.to_str().unwrap()])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert_eq!(out_none.status.code(), Some(1));

    // 5. Missing file: exit code 2
    let out_err = Command::new(exe())
        .args(["grep", "ERROR", "nonexistent_file_12345.log"])
        .stdout(Stdio::piped())
        .output()
        .expect("spawn");
    assert_eq!(out_err.status.code(), Some(2));
}

