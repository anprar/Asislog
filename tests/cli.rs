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
