// English comments: Main log viewport (virtual scroll, highlights, strip map)
// + P0-1 word wrap, P0-2 text selection, P0-5 quickfind bar, P1-12 scope dim.
#![allow(unused_imports)]

use std::time::{Duration, Instant};

use crate::engine::{format_count, format_size};
use crate::ui::{
    icons::{icon_button, Icon},
    viewer,
};
use super::*;

impl AsisLogApp {
    pub(crate) fn render_viewport(&mut self, ctx: &egui::Context, cur_idx: usize) {
        // Status fokus dihitung SEBELUM pinjam tab (navigasi keyboard di
        // bawah butuh tahu apakah caret sedang di kolom teks).
        let (field_focused, dialog_open) = self.focus_state(ctx);
        let lang = self.lang;
        // Shortcut tabel untuk navigasi viewport: dihitung sebelum pinjam
        // tab (&self vs &mut tab tak boleh berdampingan).
        let typing_vp = field_focused || dialog_open;
        let go_home_plain = self.scut_pressed(ctx, "home_plain", true, typing_vp, dialog_open);
        let go_end_plain = self.scut_pressed(ctx, "end_plain", true, typing_vp, dialog_open);
        // Spasi = paging ala pembaca, TAPI jangan rebut tombol yang sedang
        // fokus (Spasi = klik tombol itu — perilaku UI standar).
        let btn_focused = !typing_vp && ctx.memory(|m| m.focused().is_some());
        let go_page_down =
            !btn_focused && self.scut_pressed(ctx, "page_down", true, typing_vp, dialog_open);
        let go_page_up =
            !btn_focused && self.scut_pressed(ctx, "page_up", true, typing_vp, dialog_open);
        let mut cfg_dirty_after = false;
        let session_after = false;
        // ---- viewport utama: memakai seluruh sisa tinggi CentralPanel ----
        egui::CentralPanel::default().show(ctx, |ui| {
            let rh = self.row_h();
            let rules = self.hl_compiled.clone();
            let sql_cols_enabled = self.sql_cols_enabled;
            let wrap_on = self.tabs[cur_idx].word_wrap;
            let dark = self.tema_state.1;
            let mut toggle_wrap = false;
            let mut clear_sel = false;
            let mut select_all = false;
            let mut copy_sel = false;
            let mut qf_open_new = false;
            let mut qf_dir: Option<bool> = None;
            // Ukur sekali di awal: tinggi log = sisa panel dikurangi baris aksi.
            let avail = ui.available_size();
            let action_h = 30.0;
            let log_h = (avail.y - action_h).max(60.0);
            let tab = &mut self.tabs[cur_idx];
            let total_rows = tab.total_view_rows();
            let visible = ((log_h / rh).floor() as u64).clamp(10, 400);
            tab.last_visible = visible;

            // Wheel: gulir per baris — hanya bila tidak ada dialog/palet dan
            // kursor berada di viewport. Sengaja baca RAW delta saja:
            // smooth_scroll_delta adalah gema meluruh dari event yang sama
            // (memakainya juga = scroll ganda / hanyut tanpa input).
            let mouse_in_viewport = ui.rect_contains_pointer(ui.max_rect());
            if !field_focused && !dialog_open && !self.palette_open && mouse_in_viewport {
                let delta_y = ctx.input(|i| i.raw_scroll_delta.y);
                if delta_y != 0.0 {
                    let step = ((delta_y.abs() / 20.0).ceil() as u64).clamp(1, 50);
                    if delta_y < 0.0 {
                        tab.top_row = (tab.top_row + step).min(total_rows.saturating_sub(1));
                        if tab.doc.follow
                            && tab.top_row + visible >= total_rows.saturating_sub(2)
                        {
                            tab.doc.stick_bottom = true;
                        }
                    } else {
                        tab.top_row = tab.top_row.saturating_sub(step);
                        tab.doc.stick_bottom = false;
                    }
                }
            }
            // Keyboard atas/bawah PgUp/PgDn + wrap/selection/quickfind keys.
            if !field_focused && !dialog_open && !tab.qf_open {
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    tab.top_row = (tab.top_row + 1).min(total_rows.saturating_sub(1));
                    if tab.doc.follow && tab.top_row + visible >= total_rows.saturating_sub(2) {
                        tab.doc.stick_bottom = true;
                    }
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    tab.top_row = tab.top_row.saturating_sub(1);
                    tab.doc.stick_bottom = false;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::PageDown)) {
                    tab.top_row = (tab.top_row + visible).min(total_rows.saturating_sub(1));
                }
                if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
                    tab.top_row = tab.top_row.saturating_sub(visible);
                    tab.doc.stick_bottom = false;
                }
                if go_page_down {
                    tab.top_row = (tab.top_row + visible).min(total_rows.saturating_sub(1));
                    if tab.doc.follow && tab.top_row + visible >= total_rows.saturating_sub(2) {
                        tab.doc.stick_bottom = true;
                    }
                }
                if go_page_up {
                    tab.top_row = tab.top_row.saturating_sub(visible);
                    tab.doc.stick_bottom = false;
                }
                if go_home_plain {
                    tab.top_row = 0;
                    tab.selected_line = tab.row_to_line(0).unwrap_or(1);
                    tab.doc.stick_bottom = false;
                }
                if go_end_plain {
                    tab.top_row = total_rows.saturating_sub(visible);
                    tab.doc.stick_bottom = true;
                }
                // (Sengaja tanpa re-stick pasif di sini: menempel ulang
                // hanya saat baris BARU tiba — lihat pin di bawah. Re-stick
                // pasif membuat scroll manual mental balik ke bawah.)
                // P0-1: W toggle word wrap (tanpa modifier, di luar ketik).
                if ctx.input(|i| i.key_pressed(egui::Key::W) && !i.modifiers.any()) {
                    toggle_wrap = true;
                }
                // P0-2: Ctrl+A select-all.
                if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::A)) {
                    select_all = true;
                }
                // Escape menghapus seleksi bila ada.
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && tab.sel_has_any() {
                    clear_sel = true;
                }
                // P0-2: Ctrl+C / Ctrl+Insert salin seleksi.
                let ctrl_copy = ctx.input(|i| {
                    i.modifiers.ctrl
                        && !i.modifiers.shift
                        && !i.modifiers.alt
                        && (i.key_pressed(egui::Key::C) || i.key_pressed(egui::Key::Insert))
                });
                if ctrl_copy && tab.sel_has_any() {
                    copy_sel = true;
                }
                // P0-5: `/` (forward) / `?` (Shift+/) buka quickfind.
                let slash = ctx.input(|i| {
                    i.key_pressed(egui::Key::Slash)
                        && (!i.modifiers.any() || i.modifiers.shift_only())
                });
                if slash {
                    qf_open_new = true;
                    qf_dir = Some(!ctx.input(|i| i.modifiers.shift));
                }
            }
            if toggle_wrap {
                tab.word_wrap = !tab.word_wrap;
                tab.wrap_rows_cache.clear();
                cfg_dirty_after = true;
            }
            if select_all {
                let total_lines_now = if tab.doc.index.complete {
                    tab.doc.index.total_lines
                } else {
                    tab.doc.line_count_estimate()
                };
                tab.sel_anchor = Some(1);
                tab.sel_active = Some(total_lines_now.max(1));
                tab.sel_portion = None;
                tab.sel_extra.clear();
            }
            if clear_sel {
                tab.sel_clear();
            }
            if copy_sel {
                match tab.sel_copy_text() {
                    Ok(s) => {
                        ctx.copy_text(s);
                        tab.doc.status = lang.tr("Seleksi disalin ke papan klip.").to_string();
                    }
                    Err(e) => tab.doc.status = lang.tr_status(&e),
                }
            }
            if qf_open_new && !tab.qf_open {
                tab.qf_open = true;
                tab.qf_match = None;
                tab.qf_msg = None;
                if let Some(fwd) = qf_dir {
                    tab.qf_forward = fwd;
                }
            }
            // Pin follow: menempel di bawah HANYA saat baris baru tiba
            // (total_rows tumbuh) selagi menempel. Scroll manual ke atas
            // melepas pin dan melepasnya tetap — tanpa re-stick pasif,
            // semua input scroll (wheel/drag/keyboard) bebas bergerak.
            if tab.doc.follow && tab.doc.stick_bottom && total_rows > tab.follow_total {
                tab.top_row = total_rows.saturating_sub(visible);
            }
            tab.follow_total = total_rows;
            tab.top_row = tab.top_row.min(total_rows.saturating_sub(1));

            // Ambil baris viewport (decode kepala 16 KiB saja; baris
            // 100 MB tak pernah dirender penuh — anti-freeze C-C6).
            let start_row = tab.top_row;
            let mut rows: Vec<(u64, u64, String)> = Vec::new();
            for r in start_row..(start_row + visible).min(total_rows) {
                let Some(ln) = tab.row_to_line(r) else { continue };
                let txt = match tab.doc.get_line_head(ln, crate::engine::HEAD_BYTES) {
                    Some((mut head, total_b, true)) => {
                        head.push_str(&format!(
                            " …(+{} B)",
                            crate::engine::format_count(
                                total_b.saturating_sub(head.len() as u64)
                            )
                        ));
                        head
                    }
                    Some((head, _, false)) => head,
                    None => String::from("…"),
                };
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
            // aktif (hijau). Disampling maks ~1200 titik.
            let mut markers: Vec<(f32, egui::Color32)> = Vec::new();
            if total_lines > 0 {
                let total_f = total_lines as f64;
                let frac = |line: u64| ((line as f64 / total_f).clamp(0.0, 1.0)) as f32;
                let stride = (tab.doc.hits.len() / 1200).max(1);
                for (i, h) in tab.doc.hits.iter().enumerate() {
                    if i % stride != 0 {
                        continue;
                    }
                    markers.push((frac(h.line), egui::Color32::from_rgb(70, 210, 200)));
                }
                for b in &tab.doc.bookmarks {
                    markers.push((frac(b.line), egui::Color32::from_rgb(90, 160, 255)));
                }
                if let Some(c) = tab.current_hit.and_then(|c| tab.doc.hits.get(c)) {
                    markers.push((frac(c.line), egui::Color32::GREEN));
                }
            }
            let digits = total_lines.to_string().len().max(4);
            // Cakupan aktif (P1-12: baris di luar cakupan dirender redup).
            let scope = tab.scope;
            // Area log setinggi log_h persis (kolom daftar + strip + slider).
            // Diberikan koordinat absolut pasti agar kolom strip dan scrollbar
            // TETAP kokoh menempel di batas kanan jendela (tidak pernah bergeser,
            // tidak pernah mengecil ketika teks baris pendek, dan tidak terdorong
            // keluar ketika teks baris panjang).
            let total_w = avail.x.max(200.0);
            let (area_rect, _) = ui.allocate_exact_size(
                egui::vec2(total_w, log_h),
                egui::Sense::hover(),
            );

            let strip_w = 12.0;
            let bar_w = 32.0;
            let right_w = strip_w + bar_w;
            let list_w = (area_rect.width() - right_w).max(120.0);

            let list_rect = egui::Rect::from_min_size(area_rect.min, egui::vec2(list_w, log_h));
            let strip_rect = egui::Rect::from_min_size(
                egui::pos2(area_rect.right() - right_w, area_rect.top()),
                egui::vec2(strip_w, log_h),
            );
            let bar_rect = egui::Rect::from_min_size(
                egui::pos2(area_rect.right() - bar_w, area_rect.top()),
                egui::vec2(bar_w, log_h),
            );

            // 1. Kolom daftar log: wrap mematikan gulir mendatar, teks melipat ke bawah
            ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
                ui.set_clip_rect(list_rect);
                if wrap_on {
                    vertical_rows(
                        ui, tab, &rows, cur_hit_line, digits, dark, &rules,
                        sql_cols_enabled, lang, scope, Some(list_w),
                    );
                } else {
                    egui::ScrollArea::horizontal()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            vertical_rows(
                                ui, tab, &rows, cur_hit_line, digits, dark, &rules,
                                sql_cols_enabled, lang, scope, None,
                            );
                        });
                }
            });

            // 2. Strip peta: klik = lompat ke posisi file (selalu tertambat di kanan)
            ui.scope_builder(egui::UiBuilder::new().max_rect(strip_rect), |ui| {
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
                    let y = rect.top() + f.clamp(0.0, 1.0) * rect.height();
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(rect.left(), y - 1.0),
                            egui::vec2(rect.width(), 2.0),
                        ),
                        0.0,
                        *c,
                    );
                }
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
                            let y1 = rect.top() + ((i + 1) as f32 / n) * rect.height();
                            let a = (40.0 + 160.0 * (*c as f32 / hist_bins as f32)) as u8;
                            painter.rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(rect.left(), y0),
                                    egui::pos2(rect.right(), y1.max(y0 + 1.0)),
                                ),
                                0.0,
                                egui::Color32::from_rgba_premultiplied(220, 60, 60, a),
                            );
                        }
                    }
                }
                if total_rows > 0 {
                    let f = tab.top_row as f64 / total_rows as f64;
                    let y = rect.top() + (f.clamp(0.0, 1.0) as f32) * rect.height();
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(rect.left(), y - 1.0),
                            egui::vec2(rect.width(), 2.0),
                        ),
                        0.0,
                        if dark { egui::Color32::WHITE } else { egui::Color32::BLACK },
                    );
                }
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let f = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64;
                        tab.top_row = ((f * total_rows as f64) as u64)
                            .min(total_rows.saturating_sub(1));
                        tab.doc.stick_bottom = tab.doc.follow
                            && tab.top_row + visible >= total_rows.saturating_sub(2);
                        if let Some(ln) = tab.row_to_line(tab.top_row) {
                            tab.selected_line = ln;
                            tab.record_nav(ln);
                        }
                    }
                }
                if resp.hovered() {
                    if let Some(pos) = resp.hover_pos() {
                        let f = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0) as f64;
                        let target_row = (f * total_rows as f64) as u64;
                        if let Some(ln) = tab.row_to_line(target_row) {
                            let preview = tab.doc.get_line_head(ln, 160)
                                .map(|(t, _, _)| t)
                                .unwrap_or_default();
                            let preview_clean = preview.trim();
                            resp.on_hover_ui(|ui| {
                                ui.horizontal(|ui| {
                                    ui.strong(lang.f1("Baris #{}", format_count(ln)));
                                    ui.label(format!("({:.1}%)", f * 100.0));
                                });
                                if !preview_clean.is_empty() {
                                    ui.label(egui::RichText::new(preview_clean).monospace());
                                }
                                ui.separator();
                                ui.small(lang.tr("Klik untuk lompat ke baris ini"));
                            });
                        }
                    }
                } else {
                    let bits = tab.marker_bits.as_ref();
                    let eb = bits.map(|b| b.iter().filter(|x| *x & 0x01 != 0).count()).unwrap_or(0);
                    let wb = bits.map(|b| b.iter().filter(|x| *x & 0x02 != 0).count()).unwrap_or(0);
                    resp.on_hover_text(lang.f4("Teal: hasil pencarian ({})\nBiru: penanda ({})\nMerah: bucket ERROR ({} dari 512)\nKuning: bucket WARN ({} dari 512)\nArsir merah: kepadatan ERROR/menit\nHijau: hasil aktif - Putih: posisi viewport\nKlik: lompat ke posisi", format_count(tab.doc.hits.len() as u64), format_count(tab.doc.bookmarks.len() as u64), eb, wb));
                }
            });

            // 3. Bilah gulir kustom (selalu tertambat di ujung paling kanan)
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                let track_h = (log_h - 28.0 * self.zoom).max(40.0);
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
                    .clamp(10.0, rect.height() as f64) as f32;
                let top_f = if total_rows > 1 {
                    tab.top_row as f64 / (total_rows - 1) as f64
                } else {
                    0.0
                };
                let y0 = rect.top() + (top_f as f32) * (rect.height() - th);
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
                        tab.top_row = ((f * total_rows.saturating_sub(1) as f64) as u64)
                            .min(total_rows.saturating_sub(1));
                        // Mendarat di dasar + follow = menempel lagi;
                        // di tempat lain = lepas (tanpa re-stick pasif).
                        tab.doc.stick_bottom = tab.doc.follow
                            && tab.top_row + visible >= total_rows.saturating_sub(2);
                        if let Some(ln) = tab.row_to_line(tab.top_row) {
                            tab.selected_line = ln;
                        }
                    }
                }
                resp.on_hover_text(lang.f3("Baris {} / {} · {}%", format_count(tab.top_row + 1), format_count(total_rows), format!("{:.0}", if total_rows > 0 {
                        tab.top_row as f64 / total_rows as f64 * 100.0
                    } else {
                        0.0
                    })));
                if icon_button(ui, Icon::ChevronDown, lang.tr("Ke akhir file (Ctrl+End)")).clicked() {
                    tab.top_row = total_rows.saturating_sub(visible);
                    tab.doc.stick_bottom = true;
                }
            });

            // ---- P0-5: QuickFind bar (di atas baris aksi) ----
            if tab.qf_open {
                let mut close = false;
                let mut next = false;
                let mut prev = false;
                ui.horizontal(|ui| {
                    ui.label(lang.tr("Cari cepat:"));
                    let re = ui.add(
                        egui::TextEdit::singleline(&mut tab.qf_text)
                            .id(egui::Id::new("qf"))
                            .hint_text(lang.tr("Ketik untuk cari instan… (Enter berikutnya)"))
                            .desired_width(240.0),
                    );
                    let is_focused = re.has_focus();
                    if re.changed() {
                        tab.qf_match = None;
                        tab.qf_find(true);
                    }
                    if is_focused && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if ctx.input(|i| i.modifiers.shift) {
                            prev = true;
                        } else {
                            next = true;
                        }
                    }
                    if ui.button("‹").on_hover_text(lang.tr("Sebelumnya (Shift+Enter)")).clicked() {
                        prev = true;
                    }
                    if ui.button("›").on_hover_text(lang.tr("Berikutnya (Enter)")).clicked() {
                        next = true;
                    }
                    if !tab.qf_text.trim().is_empty() {
                        let count_text = if let Some((ln, _, _)) = tab.qf_match {
                            lang.f1("Baris {}", format_count(ln))
                        } else {
                            lang.tr("Tidak ditemukan.").to_string()
                        };
                        ui.label(count_text);
                    }
                    if let Some((msg, at)) = &tab.qf_msg {
                        if at.elapsed() < Duration::from_secs(3) {
                            ui.weak(lang.tr_status(msg));
                        }
                    }
                    if ui.small_button("×").on_hover_text(lang.tr("Tutup (Esc)")).clicked() {
                        close = true;
                    }
                });
                if next {
                    tab.qf_find(true);
                }
                if prev {
                    tab.qf_find(false);
                }
                if close {
                    tab.qf_open = false;
                }
            }

            // Baris aksi bawah (setinggi action_h).
            let tab = &mut self.tabs[cur_idx];
            let mut sql_toggle = false;
            let mut sql_next = sql_cols_enabled;
            let mut wrap_toggle = false;
            let mut wrap_next = wrap_on;
            let mut open_folder = false;
            let mut open_ext = false;
            let mut copy_full_path = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(lang.f1("Baris {}", tab.selected_line));
                // P0-2: ringkasan multi-seleksi (non-kontigu).
                let ranges = tab.sel_all_ranges();
                if ranges.len() > 1 || tab.sel_portion.is_some() {
                    let n = tab.sel_line_count();
                    let info: String = if tab.sel_portion.is_some() {
                        lang.tr("seleksi teks").to_string()
                    } else {
                        lang.f2("{} baris ({} rentang)", format_count(n), format_count(ranges.len() as u64))
                    };
                    ui.label(info).on_hover_text(lang.tr("Ctrl+klik: tambah/hapus baris ke seleksi."));
                }
                // P0-1: toggle wrap di baris aksi.
                if ui
                    .selectable_label(wrap_on, lang.tr("Lipat (W)"))
                    .on_hover_text(lang.tr("Word wrap: baris panjang dilipat ke lebar jendela"))
                    .clicked()
                {
                    wrap_toggle = true;
                    wrap_next = !wrap_on;
                }
                crate::ui::icons::menu_drop_down(ui, lang.tr("Salin"), |ui| {
                    if ui.button(lang.tr("Salin baris ini")).clicked() {
                        match tab.doc.copy_range_text(tab.selected_line, tab.selected_line) {
                            Ok(s) => {
                                ctx.copy_text(s);
                                tab.doc.status = lang.tr("Baris disalin ke papan klip.").to_string();
                            }
                            Err(e) => tab.doc.status = lang.tr_status(&e),
                        }
                        ui.close();
                    }
                    if ui.button(lang.tr("Salin 50 baris")).clicked() {
                        match tab.doc.copy_range_text(tab.selected_line, tab.selected_line + 49) {
                            Ok(s) => {
                                ctx.copy_text(s);
                                tab.doc.status = lang.tr("50 baris disalin.").to_string();
                            }
                            Err(e) => tab.doc.status = lang.tr_status(&e),
                        }
                        ui.close();
                    }
                    if ui.button(lang.tr("Salin + nomor (50 baris)")).on_hover_text(lang.tr("Format \"nomor: isi\"")).clicked() {
                        copy_numbered(tab, ctx, 50);
                        ui.close();
                    }
                    if ui.button(lang.tr("Simpan 200 baris ke file…")).on_hover_text(lang.tr("Tulis 200 baris dari posisi ini ke file baru")).clicked() {
                        let a = tab.selected_line;
                        if let Some(p) = rfd::FileDialog::new().set_file_name("asislog-pilihan.txt").save_file() {
                            match tab.doc.export_range_to_file(&p, a, a + 199) {
                                Ok(n) => tab.doc.status = lang.f2("Disimpan {} baris ke {}.", n, p.display()),
                                Err(e) => tab.doc.status = lang.tr_status(&e),
                            }
                        }
                        ui.close();
                    }
                    if ui.button(lang.tr("Salin sebagai path")).on_hover_text(lang.tr("Salin \"file:baris\" untuk referensi")).clicked() {
                        ctx.copy_text(format!("{}:{}", tab.doc.path.display(), tab.selected_line));
                        tab.doc.status = lang.tr("Path + baris disalin.").to_string();
                        ui.close();
                    }
                });
                crate::ui::icons::menu_drop_down(ui, lang.tr("Salin blok"), |ui| {
                    if ui.button(lang.tr("Salin blok SQL")).on_hover_text(lang.tr("Statement --INSERT-…/INSERT INTO… s.d. go")).clicked() {
                        copy_block(tab, ctx, 0);
                        ui.close();
                    }
                    if ui.button(lang.tr("Salin blok transaksi")).on_hover_text(lang.tr("BEGIN TRANSACTION s.d. COMMIT/ROLLBACK/go")).clicked() {
                        copy_block(tab, ctx, 1);
                        ui.close();
                    }
                    if ui.button(lang.tr("Salin blok checkpoint")).on_hover_text(lang.tr("--START CHECKPOINT s.d. --FINISH CHECKPOINT")).clicked() {
                        copy_block(tab, ctx, 2);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(lang.tr("Ekspor blok SQL…")).clicked() {
                        export_block(tab, 0, "asislog-blok.sql");
                        ui.close();
                    }
                    if ui.button(lang.tr("Ekspor blok transaksi…")).clicked() {
                        export_block(tab, 1, "asislog-transaksi.sql");
                        ui.close();
                    }
                    if ui.button(lang.tr("Ekspor blok checkpoint…")).clicked() {
                        export_block(tab, 2, "asislog-checkpoint.txt");
                        ui.close();
                    }
                });
                // P1-9: integrasi OS di baris aksi.
                if ui.button(lang.tr("Folder")).on_hover_text(lang.tr("Buka folder file ini di File Explorer")).clicked() {
                    open_folder = true;
                }
                if ui.button(lang.tr("Buka di aplikasi")).on_hover_text(lang.tr("Buka file ini di aplikasi default OS")).clicked() {
                    open_ext = true;
                }
                if ui.button(lang.tr("Path penuh")).on_hover_text(lang.tr("Salin path penuh file ini")).clicked() {
                    copy_full_path = true;
                }
                ui.separator();
                if ui.selectable_label(sql_cols_enabled, lang.tr("Kolom SQL")).on_hover_text(lang.tr("Tampilan Kolom SQL: pisahkan Timestamp / Sesi / Aksi / Kueri secara terstruktur")).clicked() {
                    sql_toggle = true;
                    sql_next = !sql_cols_enabled;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pos = if total_rows > 0 {
                        (tab.top_row as f64 / total_rows as f64 * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    };
                    ui.label(lang.f3("{}/{} · {}%", format_count(tab.top_row + 1), format_count(total_rows), format!("{:.0}", pos)));
                });
            });
            if wrap_toggle {
                tab.word_wrap = wrap_next;
                tab.wrap_rows_cache.clear();
                cfg_dirty_after = true;
            }
            if sql_toggle {
                // tulis balik ke self melalui thread-local; self masih
                // dipinjam tab di dalam closure — di-apply setelah panel.
                SQL_PENDING.with(|c| *c.borrow_mut() = Some((cur_idx, sql_next)));
            }
            if open_folder {
                show_in_explorer(&tab.doc.path, &mut tab.doc.status, lang);
            }
            if open_ext {
                open_in_default_app(&tab.doc.path, &mut tab.doc.status, lang);
            }
            if copy_full_path {
                ctx.copy_text(tab.doc.path.display().to_string());
                tab.doc.status = lang.tr("Path penuh disalin.").to_string();
            }
            let _ = sql_cols_enabled;
        });
        // Terapkan sql toggle yang dijendela di atas (self bebas pinjam kini).
        if let Some((idx, v)) = SQL_PENDING.with(|c| c.borrow_mut().take()) {
            let _ = idx;
            self.sql_cols_enabled = v;
            self.cfg_dirty = true;
        }
        if cfg_dirty_after {
            self.cfg_dirty = true;
        }
        if session_after {
            self.session_dirty = true;
        }
    }
}

thread_local! {
    /// Sql-toggle pending dari dalam closure panel (dipakai sekali per frame).
    static SQL_PENDING: std::cell::RefCell<Option<(usize, bool)>> =
        const { std::cell::RefCell::new(None) };
}

/// Render daftar baris vertikal di dalam viewport (dipakai wrap & scroll).
/// P0-2: klik = seleksi, drag = rentang, double-click = pilih kata;
/// P1-12: baris di luar cakupan pencarian dirender redup.
#[allow(clippy::too_many_arguments)]
fn vertical_rows(
    ui: &mut egui::Ui,
    tab: &mut crate::app::tab::TabState,
    rows: &[(u64, u64, String)],
    cur_hit_line: Option<u64>,
    digits: usize,
    dark: bool,
    rules: &[viewer::CompiledRule],
    sql_cols: bool,
    lang: crate::i18n::Lang,
    scope: Option<(u64, u64)>,
    wrap_width: Option<f32>,
) {
    let ctx = ui.ctx().clone();
    ui.vertical(|ui| {
        let hover_old = tab.hover_line;
        let mut hover_new = None;
        for (_r, ln, txt) in rows {
            let ln = *ln;
            let disp = tab.display_cached(ln, txt);
            let kind = viewer::classify(&disp);
            let is_current = Some(ln) == cur_hit_line;
            let in_sel = tab.sel_contains(ln);
            let has_portion = tab.sel_portion.map(|(l, cs, ce)| l == ln && cs != ce).unwrap_or(false);
            let portion_arg = if has_portion {
                tab.sel_portion.map(|(_, cs, ce)| (cs, ce))
            } else {
                None
            };
            let is_sel = !is_current && !has_portion && (ln == tab.selected_line || in_sel);
            // P1-12: di luar cakupan pencarian = redup ekstra.
            let out_scope = scope.map(|(a, b)| ln < a || ln > b).unwrap_or(false);
            let bg = if is_current {
                viewer::bg_for_current_match_theme(dark)
            } else if is_sel {
                viewer::bg_for_selection(dark)
            } else if Some(ln) == tab.hover_line {
                viewer::bg_for_hover(dark)
            } else {
                egui::Color32::TRANSPARENT
            };
            let rr = egui::Frame::new().fill(bg).show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Gutter: nomor asli + penanda ("*" ASCII, anti-tofu).
                    let mark_col = tab.doc.bookmarks.iter().find(|b| b.line == ln).map(|b| b.color);
                    let gutter = format!(
                        "{} {:>w$}",
                        if mark_col.is_some() { "*" } else { " " },
                        ln,
                        w = digits
                    );
                    let gcolor = match mark_col {
                        Some(c) => mark_color(c, dark),
                        None => viewer::gutter_color(dark),
                    };
                    let g = ui.add(
                        egui::Label::new(egui::RichText::new(gutter).monospace().color(gcolor))
                            .sense(egui::Sense::click()),
                    );
                    ui.separator();
                    // Teks: click = pilih, drag = rentang/sebagian, dblclick = kata.
                    let t = if sql_cols {
                        if let Some(cols) = crate::engine::sqlcols::parse_sql_cols(&disp) {
                            ui.label(
                                egui::RichText::new(cols.timestamp)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(100, 180, 240)),
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(cols.session)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(180, 150, 220)),
                            );
                            ui.separator();
                            let act_color = if cols.action == "ERROR" || cols.action == "ROLLBACK" {
                                egui::Color32::from_rgb(240, 80, 80)
                            } else if cols.action == "COMMIT" || cols.action == "CHECKPOINT" {
                                egui::Color32::from_rgb(60, 200, 120)
                            } else {
                                egui::Color32::from_rgb(240, 190, 60)
                            };
                            ui.label(
                                egui::RichText::new(cols.action)
                                    .strong()
                                    .monospace()
                                    .color(act_color),
                            );
                            ui.separator();
                            let line_wrap = wrap_width.map(|_| (ui.available_width() - 8.0).max(60.0));
                            viewer::render_log_line(ui, cols.query, kind, dark, rules, is_sel, line_wrap, portion_arg)
                        } else {
                            let line_wrap = wrap_width.map(|_| (ui.available_width() - 8.0).max(60.0));
                            viewer::render_log_line(ui, &disp, kind, dark, rules, is_sel, line_wrap, portion_arg)
                        }
                    } else {
                        let line_wrap = wrap_width.map(|_| (ui.available_width() - 8.0).max(60.0));
                        viewer::render_log_line(ui, &disp, kind, dark, rules, is_sel, line_wrap, portion_arg)
                    };
                    // P1-12 tanda visual kecil di gutter bila di luar cakupan.
                    if out_scope {
                        ui.painter().circle_filled(
                            g.rect.right_top() + egui::vec2(-4.0, 4.0),
                            2.0,
                            viewer::out_of_scope_color(dark),
                        );
                    }
                    RowResp { g, t }
                })
                .inner
            })
            .inner;
            // --- interaksi seleksi (P0-2: klik / Shift+klik / Ctrl+klik multi / drag) ---
            let shift = ctx.input(|i| i.modifiers.shift);
            let ctrl = ctx.input(|i| i.modifiers.ctrl);
            let dragging = ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
            if rr.g.clicked() {
                // Gutter = toggle penanda (perilaku lama).
                tab.doc.toggle_bookmark(ln);
                tab.selected_line = ln;
                tab.marks_dirty = true;
                tab.refresh_mode_map();
                tab.sel_clear();
            } else if rr.t.double_clicked() {
                // Double-click: pilih kata pada posisi kursor.
                if let Some(pos) = rr.t.interact_pointer_pos() {
                    let rel_pos = pos - rr.t.rect.min;
                    let fid = viewer::mono_font_id(&ctx);
                    let line_wrap = wrap_width.map(|_| rr.t.rect.width().max(60.0));
                    let col = viewer::pos_to_col(&ctx, &fid, &disp, rel_pos, line_wrap);
                    tab.sel_word_at(ln, col);
                }
            } else if rr.t.dragged() && !ctrl {
                // Drag untuk seleksi sebagian isi teks (partial text) atau multi-baris
                if let Some(pos) = rr.t.interact_pointer_pos() {
                    let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                    if rr.t.rect.y_range().contains(origin.y) {
                        // Drag dalam baris yang sama: pilih rentang karakter (portion)
                        let rel_origin = origin - rr.t.rect.min;
                        let rel_pos = pos - rr.t.rect.min;
                        let fid = viewer::mono_font_id(&ctx);
                        let line_wrap = wrap_width.map(|_| rr.t.rect.width().max(60.0));
                        let c_start = viewer::pos_to_col(&ctx, &fid, &disp, rel_origin, line_wrap);
                        let c_cur = viewer::pos_to_col(&ctx, &fid, &disp, rel_pos, line_wrap);
                        if c_start != c_cur {
                            tab.sel_portion = Some((ln, c_start.min(c_cur), c_start.max(c_cur)));
                            tab.sel_anchor = Some(ln);
                            tab.sel_active = Some(ln);
                            tab.selected_line = ln;
                        }
                    } else if tab.sel_anchor.is_some() {
                        // Drag melintasi baris lain -> perluas seleksi multi-baris
                        tab.sel_portion = None;
                        tab.sel_extend(ln);
                    }
                }
            } else if rr.t.clicked() {
                if ctrl && !shift {
                    // Ctrl+klik: toggle non-kontigu (klogg multi-select).
                    tab.sel_toggle(ln);
                } else if shift {
                    // Shift+klik: perluas seleksi dari anchor.
                    tab.sel_extend(ln);
                } else {
                    tab.sel_start(ln, false);
                }
            } else if dragging && rr.t.hovered() && tab.sel_anchor.is_some() && !ctrl {
                // Pointer bergerak ke baris ini saat drag dari baris lain
                if let Some(origin) = ctx.input(|i| i.pointer.press_origin()) {
                    if !rr.t.rect.y_range().contains(origin.y) {
                        tab.sel_portion = None;
                        tab.sel_extend(ln);
                    }
                }
            }
            if rr.g.hovered() || rr.t.hovered() {
                hover_new = Some(ln);
            }
            let _ = lang;
        }
        tab.hover_line = hover_new;
        if hover_new != hover_old {
            ctx.request_repaint();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Headless GUI harness: real app + real file, synthetic input.
    /// Catches wiring bugs (focus gates, hover rects, repaint) that pure
    /// engine tests cannot see.
    struct Harness {
        ctx: egui::Context,
        app: AsisLogApp,
        frame: eframe::Frame,
        _env: Option<std::sync::MutexGuard<'static, ()>>,
        _cfg_dir: tempfile::TempDir,
        _log_dir: tempfile::TempDir,
        old_appdata: Option<String>,
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl Harness {
        fn open(lines: usize) -> Self {
            Self::open_with(lines, &|i| format!("2026-09-16 INFO scrolltest id={:05}", i))
        }

        fn open_with(lines: usize, fmt_line: &dyn Fn(u32) -> String) -> Self {
            // Serialize env-sensitive setup (APPDATA override is global).
            let env_guard: std::sync::MutexGuard<'static, ()> =
                ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let cfg_dir = tempfile::tempdir().unwrap();
            let old_appdata = std::env::var("APPDATA").ok();
            std::env::set_var("APPDATA", cfg_dir.path());
            let log_dir = tempfile::tempdir().unwrap();
            let log = log_dir.path().join("scroll.log");
            {
                use std::io::Write as _;
                let mut f = std::fs::File::create(&log).unwrap();
                for i in 0..lines as u32 {
                    writeln!(f, "{}", fmt_line(i)).unwrap();
                }
            }
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = AsisLogApp::new(&cc);
            let frame = eframe::Frame::_new_kittest();
            app.open_file(log);
            assert_eq!(app.tabs.len(), 1, "log must open as one tab");
            let mut h = Self {
                ctx,
                app,
                frame,
                _env: Some(env_guard),
                _cfg_dir: cfg_dir,
                _log_dir: log_dir,
                old_appdata,
            };
            // Pump frames until the background indexer finishes.
            for _ in 0..600 {
                h.frame_no_input();
                if h.app.tabs[0].doc.index.complete {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(
                h.app.tabs[0].doc.index.complete,
                "indexer must finish headless"
            );
            assert_eq!(h.app.tabs[0].total_view_rows(), lines as u64);
            h
        }

        fn screen() -> egui::Rect {
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 800.0))
        }

        fn frame_no_input(&mut self) {
            let raw = egui::RawInput {
                screen_rect: Some(Self::screen()),
                ..Default::default()
            };
            let _ = self.ctx.run(raw, |ctx| {
                eframe::App::update(&mut self.app, ctx, &mut self.frame);
            });
        }

        fn frame_events(&mut self, events: Vec<egui::Event>) {
            let raw = egui::RawInput {
                screen_rect: Some(Self::screen()),
                events,
                ..Default::default()
            };
            let _ = self.ctx.run(raw, |ctx| {
                eframe::App::update(&mut self.app, ctx, &mut self.frame);
            });
        }

        fn hover_list(&mut self) {
            // Pointer over the log list (center of the window lands in
            // the viewport once toolbar/search/panels take their space).
            self.frame_events(vec![egui::Event::PointerMoved(egui::pos2(600.0, 400.0))]);
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            if let Some(v) = self.old_appdata.take() {
                std::env::set_var("APPDATA", v);
            } else {
                std::env::remove_var("APPDATA");
            }
        }
    }

    #[test]
    fn headless_wheel_scroll_moves_top_row() {
        let mut h = Harness::open(5000);
        assert_eq!(h.app.tabs[0].top_row, 0);
        h.hover_list();
        // Wheel down (negative y): viewport must advance.
        h.frame_events(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            modifiers: egui::Modifiers::default(),
        }]);
        let after = h.app.tabs[0].top_row;
        assert!(after > 0, "wheel down must move top_row, got {}", after);
        // Wheel up returns to the top.
        for _ in 0..40 {
            h.frame_events(vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 120.0),
                modifiers: egui::Modifiers::default(),
            }]);
            if h.app.tabs[0].top_row == 0 {
                break;
            }
        }
        assert_eq!(h.app.tabs[0].top_row, 0);
    }

    #[test]
    fn headless_keys_scroll_viewport() {        let mut h = Harness::open(5000);
        h.hover_list();
        // PageDown jumps a screen; Home returns.
        h.frame_events(vec![egui::Event::Key {
            key: egui::Key::PageDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]);
        assert!(
            h.app.tabs[0].top_row > 0,
            "PageDown must move top_row"
        );
        h.frame_events(vec![egui::Event::Key {
            key: egui::Key::Home,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]);
        assert_eq!(h.app.tabs[0].top_row, 0, "Home must return to top");
    }

    #[test]
    fn headless_follow_up_scroll_unsticks() {
        // Follow tail pinned at the bottom: scrolling UP one row must
        // unpin and stay unpinned (the old passive re-stick ate inputs
        // that landed within 2 rows of the bottom).
        let mut h = Harness::open(5000);
        h.hover_list();
        // Pin to bottom like follow-tail does.
        {
            let tab = &mut h.app.tabs[0];
            tab.doc.follow = true;
            let total = tab.total_view_rows();
            let vis = tab.last_visible.max(10);
            tab.top_row = total.saturating_sub(vis);
            tab.doc.stick_bottom = true;
        }
        h.frame_no_input();
        let pinned = h.app.tabs[0].top_row;
        assert!(pinned > 0, "must start pinned near the end");
        // Wheel up ONE row (Point +20 -> step 1): stays inside the old
        // re-stick zone on purpose.
        h.frame_events(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 20.0),
            modifiers: egui::Modifiers::default(),
        }]);
        let moved = h.app.tabs[0].top_row;
        assert_eq!(moved + 1, pinned, "wheel up one row");
        // Idle frames: no drift (raw-only wheel, no smooth echo) and no
        // snap-back without new rows.
        h.frame_no_input();
        h.frame_no_input();
        assert_eq!(
            h.app.tabs[0].top_row, moved,
            "view must not move without input or new rows"
        );
        assert!(
            !h.app.tabs[0].doc.stick_bottom,
            "stick flag must stay cleared after manual scroll"
        );
    }

    /// Paged search: a truncated result set grows page by page with the
    /// exact grand total preserved, and un-truncates when complete.
    /// Uses a shrunken global cap (restored on drop, even on panic).
    struct PageCapGuard;
    impl PageCapGuard {
        fn capped() -> Self {
            crate::engine::search::set_search_limits(10_000, 0, 0);
            PageCapGuard
        }
    }
    impl Drop for PageCapGuard {
        fn drop(&mut self) {
            crate::engine::search::set_search_limits(0, 0, 0);
        }
    }

    #[test]
    fn headless_search_pages_past_cap() {
        let mut h = Harness::open_with(30_000, &|i| format!("2026-09-16 ERROR page id={:05}", i));
        // NOTE: cap AFTER open — AsisLogApp::new applies config defaults
        // to the same globals.
        let _guard = PageCapGuard::capped();
        assert_eq!(crate::engine::search::effective_max_hits(), 10_000);
        // Start the search (bypass debounce like the UI does on Enter).
        h.app.tabs[0].search_text = String::from("ERROR");
        {
            let (tabs, hist) = (&mut h.app.tabs, &mut h.app.history);
            tabs[0].start_search(hist);
        }
        for _ in 0..600 {
            h.frame_no_input();
            if !h.app.tabs[0].doc.search_in_progress {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let t = &h.app.tabs[0];
            assert_eq!(t.doc.hits.len(), 10_000, "page 1 capped");
            assert!(t.doc.search_truncated);
            assert_eq!(t.search_grand_total, 30_000, "exact total kept");
        }
        // Page 2 appends; total preserved; still truncated.
        assert!(h.app.tabs[0].continue_search_page());
        for _ in 0..600 {
            h.frame_no_input();
            if !h.app.tabs[0].doc.search_in_progress {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let t = &h.app.tabs[0];
            assert_eq!(t.doc.hits.len(), 20_000, "pages append in order");
            assert_eq!(t.search_grand_total, 30_000);
            assert!(t.doc.search_truncated);
            assert_eq!(t.current_hit, Some(10_000), "jump to page start");
            assert_eq!(t.doc.hits[10_000].line, 10_001);
        }
        // Page 3 completes the set: un-truncated, cacheable.
        assert!(h.app.tabs[0].continue_search_page());
        for _ in 0..600 {
            h.frame_no_input();
            if !h.app.tabs[0].doc.search_in_progress {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let t = &h.app.tabs[0];
            assert_eq!(t.doc.hits.len(), 30_000);
            assert!(!t.doc.search_truncated, "complete set un-truncates");
        }
        assert!(!h.app.tabs[0].continue_search_page(), "nothing left");
    }

        /// Manual GUI bench (headless): open + index + first paint + search
    /// latency on a synthetic ~50 MB file. Run explicitly:
    /// cargo test --release -- --ignored gui_bench --nocapture
    /// Numbers go to BENCHMARK.md; the klogg column stays empty until
    /// someone measures the same file in its GUI (see protocol there).
    #[test]
    #[ignore]
    fn gui_bench_open_and_search() {
        use std::io::Write as _;
        let mut h = Harness::open_with(1, &|_| String::from("warmup"));
        let path = h._log_dir.path().join("bench.log");
        let t0 = std::time::Instant::now();
        {
            let mut f = std::fs::File::create(&path).unwrap();
            for i in 0..1_250_000u32 {
                if i % 50 == 0 {
                    writeln!(f, "2026-09-16 ERROR bench id={:07} something failed", i).unwrap();
                } else {
                    writeln!(f, "2026-09-16 INFO bench id={:07} ok", i).unwrap();
                }
            }
            f.sync_all().unwrap();
        }
        let gen_ms = t0.elapsed().as_millis();
        let t0 = std::time::Instant::now();
        h.app.open_file(path);
        let bi = h.app.current;
        // Pump until indexed; first content frame time = first paint proxy.
        let mut first_paint_ms = 0u128;
        let mut painted = false;
        loop {
            let f0 = std::time::Instant::now();
            h.frame_no_input();
            if !painted && !h.app.tabs.is_empty() {
                first_paint_ms = f0.elapsed().as_millis();
                painted = true;
            }
            if h.app.tabs[bi].doc.index.complete {
                break;
            }
            if t0.elapsed().as_secs() > 300 {
                panic!("index too slow");
            }
        }
        let index_ms = t0.elapsed().as_millis();
        let total = h.app.tabs[bi].total_view_rows();
        // Search to completion (25k ERROR matches, under the cap).
        h.app.tabs[bi].search_text = String::from("ERROR");
        {
            let (tabs, hist) = (&mut h.app.tabs, &mut h.app.history);
            tabs[bi].start_search(hist);
        }
        let t0 = std::time::Instant::now();
        let mut first_batch_ms = 0u128;
        loop {
            h.frame_no_input();
            let t = &h.app.tabs[bi];
            if first_batch_ms == 0 && !t.doc.hits.is_empty() {
                first_batch_ms = t0.elapsed().as_millis();
            }
            if !t.doc.search_in_progress {
                break;
            }
            if t0.elapsed().as_secs() > 300 {
                panic!("search too slow");
            }
        }
        let search_ms = t0.elapsed().as_millis();
        let t = &h.app.tabs[bi];
        eprintln!(
            "[gui-bench] gen={}ms open+index={}ms first_paint={}ms rows={} \
             search_first_batch={}ms search_done={}ms hits={} truncated={}",
            gen_ms,
            index_ms,
            first_paint_ms,
            total,
            first_batch_ms,
            search_ms,
            t.doc.hits.len(),
            t.doc.search_truncated,
        );
        assert_eq!(total, 1_250_000);
        assert_eq!(t.doc.hits.len(), 25_000);
        assert!(!t.doc.search_truncated);
    }

    #[test]
    fn headless_follow_pin_tracks_appends() {        // While stuck, appended rows must pull the view to the new
        // bottom (the only case that re-engages the pin).
        let mut h = Harness::open(2000);
        h.hover_list();
        let path = h.app.tabs[0].doc.path.clone();
        {
            let tab = &mut h.app.tabs[0];
            tab.doc.follow = true;
            tab.follow_ms = 0; // bypass poll throttle in tests
            let total = tab.total_view_rows();
            let vis = tab.last_visible.max(10);
            tab.top_row = total.saturating_sub(vis);
            tab.doc.stick_bottom = true;
        }
        h.frame_no_input();
        let pinned = h.app.tabs[0].top_row;
        // Append 100 lines; poll until follow picks them up.
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            for i in 0..100u32 {
                writeln!(f, "2026-09-16 INFO appended id={:05}", i).unwrap();
            }
            f.sync_all().unwrap();
        }
        for _ in 0..200 {
            h.frame_no_input();
            if h.app.tabs[0].total_view_rows() >= 2100 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(h.app.tabs[0].total_view_rows(), 2100);
        assert!(
            h.app.tabs[0].top_row > pinned,
            "pinned view must track the new bottom ({} -> {})",
            pinned,
            h.app.tabs[0].top_row
        );
        assert!(h.app.tabs[0].doc.stick_bottom);
    }
}
