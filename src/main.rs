// English comments: eframe entry point, window titled AsisLog.
// windows_subsystem removes the black console window on Windows.
// For CLI flags, attach to the parent console so output is visible
// when run from cmd/PowerShell (no-op when double-clicked).
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
    fn GetStdHandle(nStdHandle: u32) -> isize;
    fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
}

#[cfg(windows)]
fn attach_parent_console() {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // -11
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // -12
    unsafe {
        // Keep an already-valid stdout (pipe / redirect) untouched.
        let cur = GetStdHandle(STD_OUTPUT_HANDLE);
        if cur != 0 && cur != -1 {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // double-clicked from Explorer: no console, harmless
        }
        // GUI-subsystem processes start with invalid standard handles;
        // rebind them to the console so println! is visible.
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            use std::os::windows::io::IntoRawHandle;
            let h = f.into_raw_handle() as isize;
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            SetStdHandle(STD_ERROR_HANDLE, h);
            // Leaked on purpose: lives until process exit.
        }
    }
}

use asislog::app::AsisLogApp;

/// Print CLI output infallibly: detached launches (no console, attach
/// failed) and broken pipes must exit 0, never panic on stdout.
fn say(line: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout(), "{}", line);
}

fn main() -> eframe::Result<()> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    // UI language for CLI: --lang en|id, -L en|id, or ASISLOG_LANG env.
    // GUI persists in config.json; CLI defaults to Indonesian (backward compatible).
    // Portable: no install, no registry, single binary unchanged.
    // Strip language flags FIRST so `asislog --lang en grep ...` still
    // routes to the subcommand instead of launching the GUI.
    let mut lang = std::env::var("ASISLOG_LANG").unwrap_or_default();
    let mut args: Vec<String> = Vec::with_capacity(raw_args.len());
    {
        let mut it = raw_args.iter().peekable();
        while let Some(a) = it.next() {
            if a == "--lang" || a == "-L" {
                if let Some(v) = it.next() {
                    lang = v.clone();
                }
            } else if let Some(v) = a.strip_prefix("--lang=") {
                lang = v.to_string();
            } else {
                args.push(a.clone());
            }
        }
    }
    let cli_lang = asislog::i18n::Lang::from_key(&lang);
    let is_id = cli_lang != asislog::i18n::Lang::En;
    let is_subcommand = !args.is_empty() && (args[0] == "grep" || args[0] == "count");
    let cli = is_subcommand
        || args
            .iter()
            .any(|a| a == "--version" || a == "-V" || a == "--help" || a == "-h");
    #[cfg(windows)]
    if cli {
        attach_parent_console();
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        say(&format!("AsisLog {}", env!("CARGO_PKG_VERSION")));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        if is_id {
            say(&format!(
                "AsisLog {} - penampil log portabel untuk file sangat besar",
                env!("CARGO_PKG_VERSION")
            ));
            say("");
            say("Penggunaan:");
            say("  asislog [FILE]...                  buka file log langsung sebagai tab");
            say("  asislog grep [-n] [-c] <POLA> <F>  cari pola dalam file log (exit 0/1)");
            say("  asislog count <FILE>               hitung total baris dan ukuran file instan");
            say("  asislog --version                  tampilkan versi lalu keluar");
            say("  asislog --help                     tampilkan bantuan ini lalu keluar");
            say("  asislog --lang en ...              tampilkan bantuan dalam English");
        } else {
            say(&format!(
                "AsisLog {} - portable log viewer for very large files",
                env!("CARGO_PKG_VERSION")
            ));
            say("");
            say("Usage:");
            say("  asislog [FILE]...                  open log files directly as tabs");
            say("  asislog grep [-n] [-c] <PAT> <F>   search pattern in log file (exit 0/1)");
            say("  asislog count <FILE>               instantly count lines and file size");
            say("  asislog --version                  print version and exit");
            say("  asislog --help                     print this help and exit");
            say("  asislog --lang id ...              tampilkan bantuan dalam Indonesia");
        }
        return Ok(());
    }

    if !args.is_empty() && args[0] == "count" {
        if args.len() < 2 {
            say(if is_id { "Penggunaan: asislog count <FILE>" } else { "Usage: asislog count <FILE>" });
            std::process::exit(2);
        }
        let path = std::path::Path::new(&args[1]);
        let md = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                if is_id {
                    say(&format!("Gagal membuka file '{}': {}", path.display(), e));
                } else {
                    say(&format!("Failed to open file '{}': {}", path.display(), e));
                }
                std::process::exit(2);
            }
        };
        if !md.is_file() {
            if is_id {
                say(&format!("Bukan file: '{}'", path.display()));
            } else {
                say(&format!("Not a file: '{}'", path.display()));
            }
            std::process::exit(2);
        }
        let total_bytes = md.len();
        let total_lines = if total_bytes == 0 {
            0
        } else {
            match asislog::engine::mmap::open_mmap(path) {
                Ok(m) => {
                    let head_len = (m.len()).min(64 * 1024);
                    let (enc, bom_len) = asislog::engine::decode::detect_encoding(&m[..head_len]);
                    let idx = asislog::engine::index::build_full(&m, enc, bom_len);
                    idx.total_lines
                }
                Err(e) => {
                    if is_id {
                        say(&format!("Gagal memetakan file '{}': {}", path.display(), e));
                    } else {
                        say(&format!("Failed to map file '{}': {}", path.display(), e));
                    }
                    std::process::exit(2);
                }
            }
        };
        if is_id {
            say(&format!("File: {}", path.display()));
            say(&format!("Baris: {}", total_lines));
            say(&format!("Ukuran: {} ({} byte)", asislog::engine::format_size(total_bytes), total_bytes));
        } else {
            say(&format!("File: {}", path.display()));
            say(&format!("Lines: {}", total_lines));
            say(&format!("Size: {} ({} bytes)", asislog::engine::format_size(total_bytes), total_bytes));
        }
        std::process::exit(0);
    }

    if !args.is_empty() && args[0] == "grep" {
        let mut show_line_num = false;
        let mut count_only = false;
        let mut pattern: Option<&str> = None;
        let mut file_path: Option<&str> = None;
        for a in args.iter().skip(1) {
            if a == "-n" {
                show_line_num = true;
            } else if a == "-c" || a == "--count" {
                count_only = true;
            } else if pattern.is_none() {
                pattern = Some(a.as_str());
            } else if file_path.is_none() {
                file_path = Some(a.as_str());
            }
        }
        let (pattern, file_path) = match (pattern, file_path) {
            (Some(p), Some(f)) => (p, f),
            _ => {
                say(if is_id { "Penggunaan: asislog grep [-n] [-c] <POLA> <FILE>" } else { "Usage: asislog grep [-n] [-c] <PATTERN> <FILE>" });
                std::process::exit(2);
            }
        };
        let path = std::path::Path::new(file_path);
        let md = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                if is_id {
                    say(&format!("Gagal membuka file '{}': {}", path.display(), e));
                } else {
                    say(&format!("Failed to open file '{}': {}", path.display(), e));
                }
                std::process::exit(2);
            }
        };
        if !md.is_file() {
            if is_id {
                say(&format!("Bukan file: '{}'", path.display()));
            } else {
                say(&format!("Not a file: '{}'", path.display()));
            }
            std::process::exit(2);
        }
        if md.len() == 0 {
            if count_only {
                say("0");
            }
            std::process::exit(1);
        }
        let m = match asislog::engine::mmap::open_mmap(path) {
            Ok(m) => m,
            Err(e) => {
                if is_id {
                    say(&format!("Gagal memetakan file '{}': {}", path.display(), e));
                } else {
                    say(&format!("Failed to map file '{}': {}", path.display(), e));
                }
                std::process::exit(2);
            }
        };
        let head_len = (m.len()).min(64 * 1024);
        let (enc, bom_len) = asislog::engine::decode::detect_encoding(&m[..head_len]);

        let finder = memchr::memmem::Finder::new(pattern.as_bytes());
        let mut match_count = 0u64;
        let mut line_no = 1u64;
        let mut pos = bom_len;
        while pos < m.len() {
            let next_nl = memchr::memchr(b'\n', &m[pos..])
                .map(|i| pos + i)
                .unwrap_or(m.len());
            let mut line_bytes = &m[pos..next_nl];
            if line_bytes.ends_with(b"\r") {
                line_bytes = &line_bytes[..line_bytes.len() - 1];
            }
            if finder.find(line_bytes).is_some() {
                match_count += 1;
                if !count_only {
                    let text = asislog::engine::decode::decode_bytes(line_bytes, enc);
                    if show_line_num {
                        say(&format!("{}:{}", line_no, text));
                    } else {
                        say(&text);
                    }
                }
            }
            line_no += 1;
            pos = next_nl + 1;
        }

        if count_only {
            say(&format!("{}", match_count));
        }
        if match_count > 0 {
            std::process::exit(0);
        } else {
            std::process::exit(1);
        }
    }
    let files: Vec<std::path::PathBuf> = args.iter().map(std::path::PathBuf::from).collect();
    let icon_data = image::load_from_memory(include_bytes!("../assets/asislog-256.png"))
        .ok()
        .map(|img| {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        });

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1200.0, 800.0])
        .with_maximized(true)
        .with_title("AsisLog");
    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "AsisLog",
        options,
        Box::new(|cc| {
            let mut app = AsisLogApp::new(cc);
            // Native feel from the first frame: OS UI font + chosen mono.
            app.apply_fonts(&cc.egui_ctx);
            app.open_files(files);
            Ok(Box::new(app))
        }),
    )
}
