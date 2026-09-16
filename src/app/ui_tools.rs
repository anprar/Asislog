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
            let lang = self.lang;
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);
                    ui.heading("AsisLog");
                    ui.label(lang.tr("Buka file log…"));
                    ui.label(lang.tr("Penampil portabel untuk file .log / .txt / .out yang sangat besar."));
                    ui.add_space(12.0);
                    if ui.button(lang.tr("Buka file log…")).clicked() {
                        self.open_dialog();
                    }
                    ui.add_space(8.0);
                    ui.weak(lang.tr("Atau seret file ke jendela ini · Ctrl+O · Ctrl+Shift+P untuk palet"));
                    ui.add_space(4.0);
                    ui.weak(lang.tr("F11 Layar Penuh · Ctrl+F cari · F1 pintasan"));
                    if let Some(e) = &self.global_error.clone() {
                        ui.colored_label(egui::Color32::RED, lang.tr_status(e));
                    }
                    ui.label(lang.tr_status(&self.global_status.clone()));
                });
            });
    }
}

impl AsisLogApp {
    pub(crate) fn render_tools(&mut self, ctx: &egui::Context, cur_idx: usize) {
        // ---- Baris 1: file & navigasi ----
        let lang = self.lang;
        let mut sess_touch = false;
        egui::TopBottomPanel::top("tools").show(ctx, |ui| {
            let tab = &mut self.tabs[cur_idx];
            ui.horizontal_wrapped(|ui| {
                // Encoding override (tanpa label teks: kotaknya menjelaskan sendiri).
                egui::ComboBox::from_id_salt("enc")
                    .selected_text(tab.doc.encoding().label())
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(tab.doc.encoding_override.is_none(), lang.tr("Otomatis"))
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
                    .on_hover_text(lang.tr("Encoding file (Otomatis = deteksi BOM + sampel)"));
                if ui
                    .button(lang.tr("Ke baris…"))
                    .on_hover_text(lang.tr("Ke nomor baris, persen, akhir, atau cap waktu (Ctrl+G)"))
                    .clicked()
                {
                    tab.goto_open = true;
                }
                if icon_button(ui, Icon::ChevronLeft, lang.tr("Kembali ke lokasi sebelumnya (Alt+Left)"))
                    .clicked()
                    && !tab.go_hist(true)
                {
                    tab.doc.status = lang.tr("Tidak ada lokasi sebelumnya.").to_string();
                }
                if icon_button(ui, Icon::ChevronRight, lang.tr("Maju ke lokasi berikutnya (Alt+Right)"))
                    .clicked()
                    && !tab.go_hist(false)
                {
                    tab.doc.status = lang.tr("Tidak ada lokasi berikutnya.").to_string();
                }
                if ui
                    .button(lang.tr("Ekspor hasil…"))
                    .on_hover_text(lang.tr("Simpan hasil pencarian ke file baru"))
                    .clicked()
                {
                    tab.export_open = true;
                }
                // Progressive disclosure: secondary tools collapse into Alat ▾
                // so the first-run chrome stays short (search stays dominant).
                let mut open_marks = false;
                let mut open_hl = false;
                let mut open_scratch = false;
                let mut toggle_analyze = false;
                crate::ui::icons::menu_drop_down(ui, lang.tr("Alat"), |ui| {
                    if ui.button(lang.tr("Penanda")).on_hover_text(lang.tr("Panel penanda (Ctrl+Shift+B)")).clicked() {
                        open_marks = true;
                        ui.close();
                    }
                    if ui.button(lang.tr("Sorotan…")).on_hover_text(lang.tr("Aturan highlight kustom (hanya viewport)")).clicked() {
                        open_hl = true;
                        ui.close();
                    }
                    if ui.button(lang.tr("Catatan")).on_hover_text(lang.tr("Scratchpad: catatan + base64/JWT/JSON/SQL")).clicked() {
                        open_scratch = true;
                        ui.close();
                    }
                    if ui.button(lang.tr("Analisis")).on_hover_text(lang.tr("Analisis Log (Parser · SQL · Gabung)")).clicked() {
                        toggle_analyze = true;
                        ui.close();
                    }
                });
                if open_marks {
                    tab.show_bookmarks = !tab.show_bookmarks;
                }
                if open_hl {
                    self.hl_open = true;
                }
                if open_scratch {
                    self.scratch_open = true;
                }
                if toggle_analyze {
                    self.analyze_open = !self.analyze_open;
                }
                // Toggle LIVE: tiga status selaras dengan bilah status
                // (LIVE aktif / LIVE dijeda / mati). Klik saat jeda = lanjut.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stuck = tab.doc.follow && tab.doc.stick_bottom;
                    let paused = tab.doc.follow && !tab.doc.stick_bottom;
                    let (label, tip) = if stuck {
                        ("LIVE", lang.tr("Mengikuti ekor file (klik untuk berhenti)"))
                    } else if paused {
                        (
                            lang.tr("LIVE jeda"),
                            lang.tr("Terjeda karena menggulir ke atas (klik untuk kembali ke ekor)"),
                        )
                    } else {
                        (lang.tr("Ikuti akhir file"), lang.tr("Pantau akhir file / tail (Ctrl+Shift+F)"))
                    };
                    if ui
                        .add(egui::Button::selectable(stuck, label))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        if paused {
                            tab.doc.stick_bottom = true;
                        } else {
                            tab.doc.follow = !tab.doc.follow;
                            tab.doc.stick_bottom = tab.doc.follow;
                        }
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
