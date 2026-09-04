// English comments: eframe entry point, window titled AsisLog.

use asislog::app::AsisLogApp;

fn main() -> eframe::Result<()> {
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
        Box::new(|cc| Ok(Box::new(AsisLogApp::new(cc)))),
    )
}
