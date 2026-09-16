// English comments: Dialogs: goto, export, search scope, save preset (split from app.rs; behavior unchanged).
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
    pub(crate) fn render_dialogs_main(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        // ---- dialog Ke… ----
        if self.tabs[cur_idx].goto_open {
            let mut do_go = false;
            let mut do_close = false;
            let mut goto_all = self.goto_all;
            egui::Window::new(lang.tr("Ke baris / persen / waktu (Ctrl+G)"))
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label(lang.tr("Contoh: 38166903 · 50% · akhir · 2026-09-03 13:41:02"));
                    let r = ui.text_edit_singleline(&mut tab.goto_input);
                    // fokus awal
                    if tab.goto_input.is_empty() {
                        r.request_focus();
                    }
                    if !tab.goto_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, lang.tr_status(&tab.goto_msg.clone()));
                    }
                    ui.checkbox(&mut goto_all, lang.tr("Semua tab (korelasi waktu/baris)"));
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Pergi")).clicked() {
                            do_go = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                    if r.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        do_go = true;
                    }
                });
            self.goto_all = goto_all;
            if do_go {
                self.goto_execute(cur_idx);
            }
            if do_close {
                self.tabs[cur_idx].goto_open = false;
            }
        }

        // ---- dialog Ekspor ----
        if self.tabs[cur_idx].export_open {
            let mut do_export: Option<usize> = None;
            let mut do_ticket: Option<usize> = None;
            let mut do_stream = false;
            let mut do_close = false;
            let mut do_cancel = false;
            egui::Window::new(lang.tr("Simpan hasil ke file…"))
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label(lang.f1("{} hasil.", tab.doc.hits.len()));
                    if tab.doc.search_truncated {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 170, 60),
                            lang.tr("Tampil dipangkas — ekspor biasa ikut terpangkas."),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label(lang.tr("Konteks (baris sekitar):"));
                        ui.add(egui::DragValue::new(&mut tab.export_context).range(0..=100));
                    });
                    if tab.export_rx.is_some() {
                        ui.label(
                            lang.tr_status(
                                &tab.doc
                                .status
                                .clone(),
                            ),
                        );
                        ui.label(lang.tr("Ekspor berjalan di latar; dialog boleh ditutup."));
                        if ui.button(lang.tr("Batalkan ekspor")).clicked() {
                            do_cancel = true;
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Hanya hasil")).clicked() {
                            do_export = Some(0);
                        }
                        if ui.button(lang.tr("Hasil + konteks")).clicked() {
                            do_export = Some(tab.export_context);
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button(lang.tr("Tiket Markdown (Jira)"))
                            .on_hover_text(lang.tr("Hasil + konteks sebagai Markdown siap paste"))
                            .clicked()
                        {
                            do_ticket = Some(tab.export_context);
                        }
                    });
                    if !tab.search_text.trim().is_empty() {
                        ui.separator();
                        if ui
                            .button(lang.tr("Ekspor SEMUA cocok (streaming)"))
                            .on_hover_text(lang.tr("Tanpa batas tampil — tulis langsung ke disk."))
                            .clicked()
                        {
                            do_stream = true;
                        }
                    }
                });
            if let Some(cx) = do_export {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-ekspor.txt")
                    .save_file();
                if let Some(p) = out {
                    self.tabs[cur_idx].start_export(p, cx, false, String::new());
                }
            }
            if let Some(cx) = do_ticket {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-tiket.md")
                    .save_file();
                if let Some(p) = out {
                    let q = self.tabs[cur_idx].search_text.clone();
                    self.tabs[cur_idx].start_export(p, cx, true, q);
                }
            }
            if do_stream {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-semua-cocok.txt")
                    .save_file();
                if let Some(p) = out {
                    self.tabs[cur_idx].start_export_search(p);
                }
            }
            if do_cancel {
                let t = &mut self.tabs[cur_idx];
                t.export_cancel.store(true, Ordering::Relaxed);
                t.export_rx = None;
                t.doc.status = lang.tr("Ekspor dibatalkan.").to_string();
                t.export_open = false;
            }
            if do_close {
                self.tabs[cur_idx].export_open = false;
            }
        }

        // ---- dialog cakupan pencarian ----
        if self.tabs[cur_idx].scope_open {
            let mut do_apply = false;
            let mut do_clear = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Cakupan pencarian (baris)"))
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label(lang.tr("Cari hanya dalam rentang baris ini. Hemat untuk file besar."));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(lang.tr("Dari"));
                        ui.text_edit_singleline(&mut tab.scope_a);
                        ui.label(lang.tr("Sampai"));
                        ui.text_edit_singleline(&mut tab.scope_b);
                    });
                    if !tab.scope_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, lang.tr_status(&tab.scope_msg));
                    }
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Terapkan")).clicked() {
                            do_apply = true;
                        }
                        if ui.button(lang.tr("Bersihkan")).clicked() {
                            do_clear = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_apply {
                let tab = &mut self.tabs[cur_idx];
                let parse = |s: &str| {
                    s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().ok()
                };
                match (parse(&tab.scope_a.clone()), parse(&tab.scope_b.clone())) {
                    (Some(a), Some(b)) if a >= 1 && b >= a => {
                        tab.scope = Some((a, b));
                        tab.scope_msg.clear();
                        tab.scope_open = false;
                        if !tab.search_text.trim().is_empty() {
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                        } else {
                            tab.doc.status = lang.f2("Cakupan {}-{} aktif; ketik query untuk mencari.", format_count(a), format_count(b));
                        }
                    }
                    _ => {
                        self.tabs[cur_idx].scope_msg =
                            lang.tr("Rentang tidak valid. Contoh: 1000000 sampai 2000000.").to_string();
                    }
                }
            }
            if do_clear {
                let tab = &mut self.tabs[cur_idx];
                tab.scope = None;
                tab.scope_a.clear();
                tab.scope_b.clear();
                tab.scope_msg.clear();
                tab.scope_open = false;
                if !tab.search_text.trim().is_empty() {
                    tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                }
            }
            if do_close {
                self.tabs[cur_idx].scope_open = false;
            }
        }

        // ---- dialog simpan preset ----
        if self.preset_save_open {
            let mut do_save = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Simpan pencarian sebagai preset"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(lang.tr("Nama preset:"));
                    ui.text_edit_singleline(&mut self.preset_save_name);
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Simpan")).clicked() {
                            do_save = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_save {
                let name = self.preset_save_name.trim().to_string();
                if name.is_empty() {
                    self.global_status = lang.tr("Nama preset tidak boleh kosong.").to_string();
                } else {
                    let t = &self.tabs[cur_idx];
                    if let Some(p) =
                        self.presets.iter_mut().find(|p| p.name == name)
                    {
                        p.query = t.search_text.clone();
                        p.regex = t.regex_on;
                        p.case_sensitive = t.case_sensitive;
                    } else {
                        self.presets.push(Preset {
                            name,
                            query: t.search_text.clone(),
                            regex: t.regex_on,
                            case_sensitive: t.case_sensitive,
                        });
                    }
                    self.save_config();
                    self.preset_save_open = false;
                }
            }
            if do_close {
                self.preset_save_open = false;
            }
        }
    }
}
