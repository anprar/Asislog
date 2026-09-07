// English comments: UI panels for investigation tools: Top-N aggregation & Hex peek.

use super::AsisLogApp;
use crate::engine::format_count;

impl AsisLogApp {
    /// Panel agregasi Top-N (C-C1).
    pub(crate) fn render_top_n_panel(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        let mut close = false;
        let mut apply_filter: Option<String> = None;
        let tab = &mut self.tabs[cur_idx];

        egui::Window::new(lang.tr("Agregasi Top-N (Error & Sesi)"))
            .default_width(520.0)
            .default_height(340.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(lang.tr("Top Error")).clicked() {
                        let res = crate::engine::top_n::top_errors(&tab.doc, 10);
                        self.top_n_results = res.into_iter().map(|it| (it.label, it.count)).collect();
                    }
                    if ui.button(lang.tr("Top Sesi")).clicked() {
                        let res = crate::engine::top_n::top_sessions(&tab.doc, 10);
                        self.top_n_results = res.into_iter().map(|it| (it.label, it.count)).collect();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();

                if self.top_n_results.is_empty() {
                    ui.label(lang.tr("Klik tombol di atas untuk menganalisis 10 error atau sesi terbanyak."));
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (i, (label, count)) in self.top_n_results.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.strong(format!("#{}. [{}x]", i + 1, format_count(*count as u64)));
                                if ui.small_button(lang.tr("Saring")).on_hover_text(lang.tr("Jadikan filter pencarian")).clicked() {
                                    apply_filter = Some(label.clone());
                                }
                                ui.label(egui::RichText::new(label).monospace());
                            });
                        }
                    });
                }
            });

        if let Some(f) = apply_filter {
            let tab = &mut self.tabs[cur_idx];
            tab.search_text = f;
            tab.start_search(&mut self.history);
        }
        if close {
            self.top_n_open = false;
        }
    }

    /// Mode Hex Peek file biner/korup (C-C5).
    pub(crate) fn render_hex_peek_panel(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        let mut close = false;
        let tab = &mut self.tabs[cur_idx];
        let file_size = tab.doc.size;

        egui::Window::new(lang.tr("Hex Peek (Mode Inspeksi Biner Aman)"))
            .default_width(620.0)
            .default_height(400.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{}: {} ({})", lang.tr("File"), tab.doc.file_name, crate::engine::format_size(file_size)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();

                // Hitung offset byte dari baris yang sedang dipilih
                let sel_byte = tab.doc.line_byte_range(tab.selected_line)
                    .map(|(s, _)| s)
                    .unwrap_or(0) as usize;

                let take_len = 512.min(tab.doc.data().len().saturating_sub(sel_byte));
                let slice = tab.doc.data().get(sel_byte..sel_byte + take_len).unwrap_or(&[]);
                let hex_lines = crate::engine::decode::format_hex_lines(slice, sel_byte);

                ui.label(lang.f1("Menampilkan 512 byte mulai offset 0x{}:", format!("{:08x}", sel_byte)));

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for line in hex_lines {
                        ui.label(egui::RichText::new(line).monospace());
                    }
                });
            });

        if close {
            self.hex_peek_open = false;
        }
    }
}
