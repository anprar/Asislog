// English comments: Interactive Time Histogram panel (C-C4: click-to-jump).

use super::AsisLogApp;
use crate::engine::format_count;

impl AsisLogApp {
    pub(crate) fn render_histogram_panel(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        let mut close = false;
        let mut jump_target_row: Option<u64> = None;
        let dark = ctx.style().visuals.dark_mode;

        let tab = &mut self.tabs[cur_idx];
        let total_rows = tab.total_view_rows();

        egui::TopBottomPanel::bottom("histogram_panel")
            .resizable(true)
            .default_height(140.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(lang.tr("Histogram ERROR per Menit (Klik bar untuk melompat)"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text(lang.tr("Tutup panel histogram")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();

                if let Some(hist) = tab.time_hist.as_ref() {
                    let max_c = hist.counts.iter().max().copied().unwrap_or(0);
                    if max_c == 0 || hist.counts.is_empty() {
                        ui.label(lang.tr("Tidak ada data ERROR tercatat dalam histogram."));
                        return;
                    }

                    let avail_w = ui.available_width().max(200.0);
                    let chart_h = 90.0;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail_w, chart_h), egui::Sense::hover());
                    let painter = ui.painter_at(rect);

                    // Background chart
                    painter.rect_filled(
                        rect,
                        4.0,
                        if dark { egui::Color32::from_gray(24) } else { egui::Color32::from_gray(240) },
                    );

                    let n = hist.counts.len();
                    let bar_w = rect.width() / n as f32;

                    for (i, &c) in hist.counts.iter().enumerate() {
                        let h_frac = if max_c > 0 { c as f32 / max_c as f32 } else { 0.0 };
                        let bar_h = (h_frac * (chart_h - 10.0)).max(if c > 0 { 3.0 } else { 0.0 });
                        let x0 = rect.left() + i as f32 * bar_w;
                        let x1 = x0 + (bar_w - 1.0).max(1.0);
                        let y1 = rect.bottom() - 4.0;
                        let y0 = y1 - bar_h;

                        let bar_rect = egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1));
                        let color = if c > 0 {
                            egui::Color32::from_rgb(235, 75, 75)
                        } else {
                            egui::Color32::from_gray(if dark { 40 } else { 220 })
                        };

                        painter.rect_filled(bar_rect, 1.0, color);

                        // Interaction per bar: hover tooltip & click to jump
                        let bar_resp = ui.interact(
                            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
                            ui.id().with(i),
                            egui::Sense::click(),
                        );

                        if bar_resp.clicked() {
                            let frac = i as f64 / n as f64;
                            let target = ((frac * total_rows as f64) as u64).min(total_rows.saturating_sub(1));
                            jump_target_row = Some(target);
                        }

                        if bar_resp.hovered() {
                            painter.rect_filled(
                                egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
                                0.0,
                                egui::Color32::from_rgba_premultiplied(255, 255, 255, 40),
                            );
                            bar_resp.on_hover_ui(|ui| {
                                ui.label(egui::RichText::new(lang.f3("Bin #{}/{} · {} ERROR", i + 1, n, format_count(c as u64))).strong());
                                ui.small(lang.tr("Klik untuk lompat ke baris waktu ini"));
                            });
                        }
                    }
                } else {
                    ui.label(lang.tr("Histogram waktu sedang dihitung di latar belakang atau file tidak memiliki cap waktu."));
                }
            });

        if let Some(target) = jump_target_row {
            let tab = &mut self.tabs[cur_idx];
            tab.top_row = target;
            tab.doc.stick_bottom = false;
            if let Some(ln) = tab.row_to_line(target) {
                tab.selected_line = ln;
                tab.record_nav(ln);
            }
        }

        if close {
            self.hist_panel_open = false;
        }
    }
}
