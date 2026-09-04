// English comments: Empty state view (split from app.rs; behavior unchanged).
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
    pub(crate) fn render_empty(&mut self, ctx: &egui::Context) {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading("AsisLog");
                    ui.label("Buka file log…");
                    ui.label("Penampil portabel untuk file .log / .txt / .out yang sangat besar.");
                    ui.add_space(12.0);
                    if ui.button("Buka file log…").clicked() {
                        self.open_dialog();
                    }
                    if let Some(e) = &self.global_error {
                        ui.colored_label(egui::Color32::RED, e);
                    }
                    ui.label(&self.global_status);
                });
            });
    }
}

impl AsisLogApp {
    pub(crate) fn render_tools(&mut self, ctx: &egui::Context, cur_idx: usize) {
        // ---- Baris 1: file & navigasi ----
        let mut sess_touch = false;
        egui::TopBottomPanel::top("tools").show(ctx, |ui| {
            let tab = &mut self.tabs[cur_idx];
            ui.horizontal_wrapped(|ui| {
                // Encoding override (tanpa label teks: kotaknya menjelaskan sendiri).
                egui::ComboBox::from_id_salt("enc")
                    .selected_text(tab.doc.encoding().label())
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(tab.doc.encoding_override.is_none(), "Otomatis")
                            .clicked()
                        {
                            tab.doc.set_encoding_override(None);
                            tab.search_cache.clear();
                            tab.disp_cache.clear();
                            sess_touch = true;
                        }
                        for e in Encoding::all() {
                            if ui
                                .selectable_label(tab.doc.encoding_override == Some(*e), e.label())
                                .clicked()
                            {
                                tab.doc.set_encoding_override(Some(*e));
                                tab.search_cache.clear();
                                tab.disp_cache.clear();
                                sess_touch = true;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Encoding file (Otomatis = deteksi BOM + sampel)");
                if ui
                    .button("Ke baris…")
                    .on_hover_text("Ke nomor baris, persen, akhir, atau cap waktu (Ctrl+G)")
                    .clicked()
                {
                    tab.goto_open = true;
                }
                if icon_button(ui, Icon::ChevronLeft, "Kembali ke lokasi sebelumnya (Alt+Left)")
                    .clicked()
                    && !tab.go_hist(true)
                {
                    tab.doc.status = String::from("Tidak ada lokasi sebelumnya.");
                }
                if icon_button(ui, Icon::ChevronRight, "Maju ke lokasi berikutnya (Alt+Right)")
                    .clicked()
                    && !tab.go_hist(false)
                {
                    tab.doc.status = String::from("Tidak ada lokasi berikutnya.");
                }
                if ui.button("Penanda").clicked() {
                    tab.show_bookmarks = !tab.show_bookmarks;
                }
                if ui
                    .button("Ekspor hasil…")
                    .on_hover_text("Simpan hasil pencarian ke file baru")
                    .clicked()
                {
                    tab.export_open = true;
                }
                if ui
                    .button("Sorotan…")
                    .on_hover_text("Aturan highlight kustom (hanya viewport)")
                    .clicked()
                {
                    self.hl_open = true;
                }
                if ui
                    .button("Catatan")
                    .on_hover_text("Scratchpad: catatan + base64/JWT/JSON/SQL")
                    .clicked()
                {
                    self.scratch_open = true;
                }
                // Toggle LIVE (mode) di kanan baris navigasi.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let live = tab.doc.follow;
                    if ui
                        .add(egui::Button::selectable(
                            live,
                            if live { "LIVE" } else { "Ikuti akhir file" },
                        ))
                        .on_hover_text("Pantau akhir file / tail (Ctrl+Shift+F)")
                        .clicked()
                    {
                        tab.doc.follow = !live;
                        tab.doc.stick_bottom = tab.doc.follow;
                        tab.last_follow_poll = Instant::now();
                        sess_touch = true;
                    }
                });
            });
        });
        if sess_touch {
            self.session_dirty = true;
        }
    }
}
