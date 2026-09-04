// English comments: Main log viewport (virtual scroll, highlights, strip map) (split from app.rs; behavior unchanged).
#![allow(unused_imports)]
// English comments: AsisLog egui app (tabs, shortcuts, background jobs).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::engine::decode::Encoding;
use crate::engine::filter::{parse_filter, ParsedFilter};
use crate::engine::follow::{check_follow, load_identity, FollowEvent};
use crate::engine::index::{self, SparseIndex};
use crate::engine::search::{self, CacheKey, FileRev, Hit, SearchCache};
use crate::engine::{format_count, format_size, BlockKind, BookmarkColor, Doc};
use crate::store::{HighlightRule, HighlightSet, HistEntry, Preset};
use crate::ui::{
    dialogs::parse_goto,
    dialogs::GotoTarget,
    icons::{icon_button, Icon},
    results::result_row,
    theme::Tema,
    viewer::{self, CompiledRule},
};
use super::*;

impl AsisLogApp {
    pub(crate) fn render_viewport(&mut self, ctx: &egui::Context, cur_idx: usize) {
        // Status fokus dihitung SEBELUM pinjam tab (navigasi keyboard di
        // bawah butuh tahu apakah caret sedang di kolom teks).
        let (field_focused, dialog_open) = self.focus_state(ctx);
        // ---- viewport utama: memakai seluruh sisa tinggi CentralPanel ----
        egui::CentralPanel::default().show(ctx, |ui| {
            let rh = self.row_h();
            let rules = &self.hl_compiled;
            let tab = &mut self.tabs[cur_idx];
            let total_rows = tab.total_view_rows();
            // Ukur sekali di awal: tinggi log = sisa panel dikurangi baris aksi.
            // (Jangan ukur ulang di tengah; itulah sumber viewport kerdil.)
            let avail = ui.available_size();
            let action_h = 30.0;
            let log_h = (avail.y - action_h).max(60.0);
            let visible = ((log_h / rh).floor() as u64).clamp(10, 400);
            tab.last_visible = visible;
            // Wheel: gulir per baris
            let delta_y = ctx.input(|i| {
                let a = i.raw_scroll_delta.y;
                let b = i.smooth_scroll_delta.y;
                if a != 0.0 { a } else { b }
            });
            if delta_y != 0.0 {
                let step = ((delta_y.abs() / 20.0).ceil() as u64).clamp(1, 50);
                if delta_y < 0.0 {
                    tab.top_row = (tab.top_row + step).min(total_rows.saturating_sub(1));
                    // di dekat bawah -> kunci bawah bila Ikuti
                    if tab.doc.follow
                        && tab.top_row + visible >= total_rows.saturating_sub(2)
                    {
                        tab.doc.stick_bottom = true;
                    }
                } else {
                    tab.top_row = tab.top_row.saturating_sub(step);
                    tab.doc.stick_bottom = false; // gulir ke atas melepas kunci
                }
            }
            // Keyboard atas/bawah PgUp/PgDn — hanya bila fokus TIDAK di kolom
            // teks (caret butuh tombol ini) dan tak ada dialog terbuka.
            // (field_focused/dialog_open dihitung di atas, sebelum pinjam tab.)
            if !field_focused && !dialog_open {
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    tab.top_row = (tab.top_row + 1).min(total_rows.saturating_sub(1));
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    tab.top_row = tab.top_row.saturating_sub(1);
                    tab.doc.stick_bottom = false; // seperti gulir roda ke atas
                }
                if ctx.input(|i| i.key_pressed(egui::Key::PageDown)) {
                    tab.top_row = (tab.top_row + visible).min(total_rows.saturating_sub(1));
                }
                if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
                    tab.top_row = tab.top_row.saturating_sub(visible);
                    tab.doc.stick_bottom = false;
                }
                // Turun hingga dasar mengunci lagi bila Ikuti (cermin roda mouse).
                if tab.doc.follow
                    && tab.top_row + visible >= total_rows.saturating_sub(2)
                {
                    tab.doc.stick_bottom = true;
                }
            }
            // Stick-to-bottom bila follow
            if tab.doc.follow && tab.doc.stick_bottom {
                tab.top_row = total_rows.saturating_sub(visible);
            }
            tab.top_row = tab.top_row.min(total_rows.saturating_sub(1));
            let dark = self.tema_state.1;

            // Ambil baris viewport (decode hanya yang terlihat).
            let start_row = tab.top_row;
            let mut rows: Vec<(u64, u64, String)> = Vec::new();
            for r in start_row..(start_row + visible).min(total_rows) {
                let Some(ln) = tab.row_to_line(r) else { continue };
                // `None` = baris belum terpetakan (indeks berjalan): tampilkan
                // placeholder agar area tak tampak kosong misterius.
                let txt = tab
                    .doc
                    .get_line_text(ln)
                    .unwrap_or_else(|| String::from("…"));
                rows.push((r, ln, txt));
            }
            let cur_hit_line = tab
                .current_hit
                .and_then(|c| tab.doc.hits.get(c))
                .map(|h| h.line);
            let total_lines = if tab.doc.index.complete {
                tab.doc.index.total_lines
            } else {
                tab.doc.line_count_estimate()
            };
            // Marker peta kepadatan: hasil cari (teal), penanda (biru),
            // aktif (hijau). Merah/kuning khusus bucket ERROR/WARN.
            // Disampling maks ~1200 titik agar murah tiap frame.
            let mut markers: Vec<(f32, egui::Color32)> = Vec::new();
            if total_lines > 0 {
                // Fraksi dalam f64 (baris bisa ratusan juta; f32 hanya
                // presisi ~16 juta bilangan bulat); ke f32 hanya untuk
                // koordinat piksel.
                let total_f = total_lines as f64;
                let frac = |line: u64| {
                    ((line as f64 / total_f).clamp(0.0, 1.0)) as f32
                };
                let stride = (tab.doc.hits.len() / 1200).max(1);
                for (i, h) in tab.doc.hits.iter().enumerate() {
                    if i % stride != 0 {
                        continue;
                    }
                    markers.push((
                        frac(h.line),
                        egui::Color32::from_rgb(70, 210, 200),
                    ));
                }
                for b in &tab.doc.bookmarks {
                    markers.push((
                        frac(b.line),
                        egui::Color32::from_rgb(90, 160, 255),
                    ));
                }
                if let Some(c) = tab.current_hit.and_then(|c| tab.doc.hits.get(c)) {
                    markers.push((frac(c.line), egui::Color32::GREEN));
                }
            }
            // Lebar gutter dinamis mengikuti digit jumlah baris.
            let digits = total_lines.to_string().len().max(4);

            // Area log setinggi log_h persis (kolom daftar + strip + slider).
            ui.allocate_ui_with_layout(
                egui::vec2(avail.x.max(200.0), log_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.horizontal(|ui| {
                        let strip_w = 12.0;
                        let bar_w = 32.0;
                        let list_w = (ui.available_width() - strip_w - bar_w).max(120.0);
                        // Kolom daftar log: gulir mendatar untuk baris panjang.
                        ui.allocate_ui_with_layout(
                            egui::vec2(list_w, log_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.style_mut().wrap_mode =
                                    Some(egui::TextWrapMode::Extend);
                                egui::ScrollArea::horizontal()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.vertical(|ui| {
                                            let hover_old = tab.hover_line;
                                            let mut hover_new = None;
                                            for (_r, ln, txt) in &rows {
                                                let ln = *ln;
                                                // Baris JSON diringkas inline (asli tetap untuk salin);
                                                // hasil ringkasan di-cache per baris (lihat display_cached).
                                                let disp = tab.display_cached(ln, txt);
                                                let kind = viewer::classify(&disp);
                                                // SELALU bungkus Frame (isi beda, ukuran sama)
                                                // agar hover tak menggeser layout (anti-flicker).
                                                // Seleksi memakai visuals() tema aktif agar konsisten
                                                // di semua tema (mis. Kontras Tinggi: hitam di kuning).
                                                let is_current = Some(ln) == cur_hit_line;
                                                let is_sel =
                                                    !is_current && ln == tab.selected_line;
                                                let bg = if is_current {
                                                    viewer::bg_for_current_match_theme(dark)
                                                } else if is_sel {
                                                    ui.visuals().selection.bg_fill
                                                } else if Some(ln) == tab.hover_line {
                                                    viewer::bg_for_hover(dark)
                                                } else {
                                                    egui::Color32::TRANSPARENT
                                                };
                                                let rr = egui::Frame::new().fill(bg).show(
                                                    ui,
                                                    |ui| {
                            ui.horizontal(|ui| {
                                                            // Gutter: nomor asli + penanda ("*" ASCII,
                                                            // bukan glyph bintang agar anti-tofu).
                                                            let mark_col = tab
                                                                .doc
                                                                .bookmarks
                                                                .iter()
                                                                .find(|b| b.line == ln)
                                                                .map(|b| b.color);
                                                            let gutter = format!(
                                                                "{} {:>w$}",
                                                                if mark_col.is_some() {
                                                                    "*"
                                                                } else {
                                                                    " "
                                                                },
                                                                ln,
                                                                w = digits
                                                            );
                                                            let gcolor = match mark_col {
                                                                Some(c) => mark_color(c, dark),
                                                                None => viewer::gutter_color(dark),
                                                            };
                                                            let g = ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(gutter)
                                                                        .monospace()
                                                                        .color(gcolor),
                                                                )
                                                                .sense(egui::Sense::click()),
                                                            );
                                                            ui.separator();
                                                            let t = viewer::render_log_line(
                                                                ui, &disp, kind, dark, rules,
                                                                is_sel,
                                                            );
                                                            RowResp { g, t }
                                                        })
                                                        .inner
                                                    },
                                                )
                                                .inner;
                                                if rr.g.clicked() {
                                                    tab.doc.toggle_bookmark(ln);
                                                    tab.selected_line = ln;
                                                    tab.marks_dirty = true;
                                                    tab.refresh_mode_map();
                                                } else if rr.t.clicked() {
                                                    tab.selected_line = ln;
                                                }
                                                if rr.g.hovered() || rr.t.hovered() {
                                                    hover_new = Some(ln);
                                                }
                                            }
                                            tab.hover_line = hover_new;
                                            if hover_new != hover_old {
                                                ctx.request_repaint();
                                            }
                                        });
                                    });
                            },
                        );
                        // Strip peta: klik = lompat ke posisi file.
                        ui.allocate_ui_with_layout(
                            egui::vec2(strip_w, log_h),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(strip_w, log_h),
                                    egui::Sense::click(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(
                                    rect,
                                    0.0,
                                    egui::Color32::from_gray(if dark { 26 } else { 235 }),
                                );
                                // Peta bucket ERROR (merah) / WARN (kuning).
                                if let Some(bits) = tab.marker_bits.as_ref() {
                                    let n = bits.len().max(1) as f32;
                                    for (i, b) in bits.iter().enumerate() {
                                        if *b == 0 {
                                            continue;
                                        }
                                        let y0 = rect.top() + (i as f32 / n) * rect.height();
                                        let y1 = rect.top() + ((i + 1) as f32 / n) * rect.height();
                                        let c = if *b & 0x01 != 0 {
                                            egui::Color32::from_rgb(220, 60, 60)
                                        } else {
                                            egui::Color32::from_rgb(220, 170, 40)
                                        };
                                        painter.rect_filled(
                                            egui::Rect::from_min_max(
                                                egui::pos2(rect.left(), y0),
                                                egui::pos2(rect.right(), y1.max(y0 + 1.0)),
                                            ),
                                            0.0,
                                            c,
                                        );
                                    }
                                }
                                for (f, c) in &markers {
                                    let y = rect.top()
                                        + f.clamp(0.0, 1.0) * rect.height();
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(rect.left(), y - 1.0),
                                            egui::vec2(rect.width(), 2.0),
                                        ),
                                        0.0,
                                        *c,
                                    );
                                }
                                // Shading histogram ERROR/menit di atas bucket.
                                let hist_bins = tab
                                    .time_hist
                                    .as_ref()
                                    .map(|h| h.counts.iter().max().copied().unwrap_or(0))
                                    .unwrap_or(0);
                                if let Some(h) = tab.time_hist.as_ref() {
                                    if hist_bins > 0 && !h.counts.is_empty() {
                                        let n = h.counts.len() as f32;
                                        for (i, c) in h.counts.iter().enumerate() {
                                            if *c == 0 {
                                                continue;
                                            }
                                            let y0 = rect.top() + (i as f32 / n) * rect.height();
                                            let y1 = rect.top()
                                                + ((i + 1) as f32 / n) * rect.height();
                                            let a = (40.0
                                                + 160.0 * (*c as f32 / hist_bins as f32))
                                                as u8;
                                            painter.rect_filled(
                                                egui::Rect::from_min_max(
                                                    egui::pos2(rect.left(), y0),
                                                    egui::pos2(rect.right(), y1.max(y0 + 1.0)),
                                                ),
                                                0.0,
                                                egui::Color32::from_rgba_premultiplied(
                                                    220, 60, 60, a,
                                                ),
                                            );
                                        }
                                    }
                                }
                                // Garis posisi viewport kini (f64: presisi di ratusan jt baris;
                                // warna sadar-tema: putih tak terlihat di strip terang).
                                if total_rows > 0 {
                                    let f = tab.top_row as f64 / total_rows as f64;
                                    let y = rect.top()
                                        + (f.clamp(0.0, 1.0) as f32) * rect.height();
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(rect.left(), y - 1.0),
                                            egui::vec2(rect.width(), 2.0),
                                        ),
                                        0.0,
                                        if dark {
                                            egui::Color32::WHITE
                                        } else {
                                            egui::Color32::BLACK
                                        },
                                    );
                                }
                                if resp.clicked() {
                                    if let Some(p) = resp.interact_pointer_pos() {
                                        let f = ((p.y - rect.top()) / rect.height())
                                            .clamp(0.0, 1.0)
                                            as f64;
                                        tab.top_row = ((f * total_rows as f64) as u64)
                                            .min(total_rows.saturating_sub(1));
                                        tab.doc.stick_bottom = false;
                                        if let Some(ln) =
                                            tab.row_to_line(tab.top_row)
                                        {
                                            tab.selected_line = ln;
                                            tab.record_nav(ln);
                                        }
                                    }
                                }
                                // Legenda makna warna strip.
                                {
                                    let bits = tab.marker_bits.as_ref();
                                    let eb = bits
                                        .map(|b| b.iter().filter(|x| *x & 0x01 != 0).count())
                                        .unwrap_or(0);
                                    let wb = bits
                                        .map(|b| b.iter().filter(|x| *x & 0x02 != 0).count())
                                        .unwrap_or(0);
                                    resp.on_hover_text(format!(
                                        "Teal: hasil pencarian ({})\nBiru: penanda ({})\nMerah: bucket ERROR ({} dari 512)\nKuning: bucket WARN ({} dari 512)\nArsir merah: kepadatan ERROR/menit\nHijau: hasil aktif - Putih: posisi viewport\nKlik: lompat ke posisi",
                                        format_count(tab.doc.hits.len() as u64),
                                        format_count(tab.doc.bookmarks.len() as u64),
                                        eb,
                                        wb,
                                    ));
                                }
                            },
                        );
                        // Bilah gulir kustom setinggi viewport: track penuh +
                        // thumb proporsional (posisi pada 38 jt baris terbaca sekilas).
                        ui.allocate_ui_with_layout(
                            egui::vec2(bar_w, log_h),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                // Ruang untuk tombol "Akhir" di bawah track (ikut zoom).
                                let track_h =
                                    (log_h - 28.0 * self.zoom).max(40.0);
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(bar_w, track_h),
                                    egui::Sense::click_and_drag(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(
                                    rect,
                                    4.0,
                                    if dark {
                                        egui::Color32::from_gray(38)
                                    } else {
                                        egui::Color32::from_gray(208)
                                    },
                                );
                                let total_f = total_rows.max(1) as f64;
                                let th = ((visible as f64 / total_f) * rect.height() as f64)
                                    .clamp(10.0, rect.height() as f64)
                                    as f32;
                                let top_f = if total_rows > 1 {
                                    tab.top_row as f64 / (total_rows - 1) as f64
                                } else {
                                    0.0
                                };
                                let y0 = rect.top()
                                    + (top_f as f32) * (rect.height() - th);
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(rect.left() + 2.0, y0),
                                        egui::vec2(rect.width() - 4.0, th),
                                    ),
                                    4.0,
                                    if dark {
                                        egui::Color32::from_gray(120)
                                    } else {
                                        egui::Color32::from_gray(140)
                                    },
                                );
                                if resp.clicked() || resp.dragged() {
                                    if let Some(p) = resp.interact_pointer_pos() {
                                        let h = rect.height() as f64;
                                        let f = ((p.y - th / 2.0 - rect.top()) as f64
                                            / (h - th as f64).max(1.0))
                                        .clamp(0.0, 1.0);
                                        tab.top_row = ((f * total_rows.saturating_sub(1) as f64)
                                            as u64)
                                            .min(total_rows.saturating_sub(1));
                                        tab.doc.stick_bottom = false;
                                        if let Some(ln) = tab.row_to_line(tab.top_row)
                                        {
                                            tab.selected_line = ln;
                                        }
                                    }
                                }
                                resp.on_hover_text(format!(
                                    "Baris {} / {} · {:.0}%",
                                    format_count(tab.top_row + 1),
                                    format_count(total_rows),
                                    if total_rows > 0 {
                                        tab.top_row as f64 / total_rows as f64 * 100.0
                                    } else {
                                        0.0
                                    },
                                ));
                                if ui.small_button("Akhir").on_hover_text("Ke akhir file").clicked() {
                                    tab.top_row = total_rows.saturating_sub(visible);
                                    tab.doc.stick_bottom = true;
                                }
                            },
                        );
                    });
                },
            );
            // Baris aksi bawah (setinggi action_h).
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Baris {}", tab.selected_line));
                ui.menu_button("Salin v", |ui| {
                    if ui.button("Salin baris ini").clicked() {
                        match tab.doc.copy_range_text(tab.selected_line, tab.selected_line) {
                            Ok(s) => {
                                ctx.copy_text(s);
                                tab.doc.status = String::from("Baris disalin ke papan klip.");
                            }
                            Err(e) => tab.doc.status = e,
                        }
                        ui.close();
                    }
                    if ui.button("Salin 50 baris").clicked() {
                        match tab.doc.copy_range_text(tab.selected_line, tab.selected_line + 49) {
                            Ok(s) => {
                                ctx.copy_text(s);
                                tab.doc.status = String::from("50 baris disalin.");
                            }
                            Err(e) => tab.doc.status = e,
                        }
                        ui.close();
                    }
                    if ui
                        .button("Salin + nomor (50 baris)")
                        .on_hover_text("Format \"nomor: isi\"")
                        .clicked()
                    {
                        copy_numbered(tab, ctx, 50);
                        ui.close();
                    }
                    if ui
                        .button("Simpan 200 baris ke file…")
                        .on_hover_text("Tulis 200 baris dari posisi ini ke file baru")
                        .clicked()
                    {
                        let a = tab.selected_line;
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name("asislog-pilihan.txt")
                            .save_file()
                        {
                            match tab.doc.export_range_to_file(&p, a, a + 199) {
                                Ok(n) => {
                                    tab.doc.status = format!(
                                        "Disimpan {} baris ke {}.",
                                        n,
                                        p.display()
                                    )
                                }
                                Err(e) => tab.doc.status = e,
                            }
                        }
                        ui.close();
                    }
                    if ui
                        .button("Salin sebagai path")
                        .on_hover_text("Salin \"file:baris\" untuk referensi")
                        .clicked()
                    {
                        ctx.copy_text(format!(
                            "{}:{}",
                            tab.doc.path.display(),
                            tab.selected_line
                        ));
                        tab.doc.status = String::from("Path + baris disalin.");
                        ui.close();
                    }
                });
                ui.menu_button("Salin blok v", |ui| {
                    if ui
                        .button("Salin blok SQL")
                        .on_hover_text("Statement --INSERT-…/INSERT INTO… s.d. go")
                        .clicked()
                    {
                        copy_block(tab, ctx, 0);
                        ui.close();
                    }
                    if ui
                        .button("Salin blok transaksi")
                        .on_hover_text("BEGIN TRANSACTION s.d. COMMIT/ROLLBACK/go")
                        .clicked()
                    {
                        copy_block(tab, ctx, 1);
                        ui.close();
                    }
                    if ui
                        .button("Salin blok checkpoint")
                        .on_hover_text("--START CHECKPOINT s.d. --FINISH CHECKPOINT")
                        .clicked()
                    {
                        copy_block(tab, ctx, 2);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Ekspor blok SQL…").clicked() {
                        export_block(tab, 0, "asislog-blok.sql");
                        ui.close();
                    }
                    if ui.button("Ekspor blok transaksi…").clicked() {
                        export_block(tab, 1, "asislog-transaksi.sql");
                        ui.close();
                    }
                    if ui.button("Ekspor blok checkpoint…").clicked() {
                        export_block(tab, 2, "asislog-checkpoint.txt");
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pos = if total_rows > 0 {
                        (tab.top_row as f64 / total_rows as f64 * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    };
                    ui.label(format!(
                        "{}/{} · {:.0}%",
                        format_count(tab.top_row + 1),
                        format_count(total_rows),
                        pos,
                    ));
                });
            });
        });
    }
}
