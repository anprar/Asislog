// English comments: eframe entry point, window titled AsisLog.
// windows_subsystem removes the black console window on Windows;
// --version/--help still work (piped stdout is inherited).
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use asislog::app::AsisLogApp;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
