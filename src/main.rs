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

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = args
        .iter()
        .any(|a| a == "--version" || a == "-V" || a == "--help" || a == "-h");
    #[cfg(windows)]
    if cli {
        attach_parent_console();
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("AsisLog {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("AsisLog {} - penampil log portabel untuk file sangat besar", env!("CARGO_PKG_VERSION"));
        println!();
        println!("Penggunaan:");
        println!("  asislog [FILE]...        buka file log langsung sebagai tab");
        println!("  asislog --version        tampilkan versi lalu keluar");
        println!("  asislog --help           tampilkan bantuan ini lalu keluar");
        return Ok(());
    }
    let files: Vec<std::path::PathBuf> = args.iter().map(std::path::PathBuf::from).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_maximized(true)
            .with_title("AsisLog"),
        ..Default::default()
    };
    eframe::run_native(
        "AsisLog",
        options,
        Box::new(|cc| {
            let mut app = AsisLogApp::new(cc);
            app.open_files(files);
            Ok(Box::new(app))
        }),
    )
}
