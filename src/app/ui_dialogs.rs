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
        // ---- dialog Ke… ----
        if self.tabs[cur_idx].goto_open {
            let mut do_go = false;
            let mut do_close = false;
            let mut goto_all = self.goto_all;
            egui::Window::new("Ke baris / persen / waktu (Ctrl+G)")
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label("Contoh: 38166903 · 50% · akhir · 2026-09-03 13:41:02");
                    let r = ui.text_edit_singleline(&mut tab.goto_input);
                    // fokus awal
                    if tab.goto_input.is_empty() {
                        r.request_focus();
                    }
                    if !tab.goto_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, &tab.goto_msg);
                    }
                    ui.checkbox(&mut goto_all, "Semua tab (korelasi waktu/baris)");
                    ui.horizontal(|ui| {
                        if ui.button("Pergi").clicked() {
                            do_go = true;
                        }
                        if ui.button("Batal").clicked() {
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
            let mut do_close = false;
            egui::Window::new("Simpan hasil ke file…")
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label(format!("{} hasil.", tab.doc.hits.len()));
                    ui.horizontal(|ui| {
                        ui.label("Konteks (baris sekitar):");
                        ui.add(egui::DragValue::new(&mut tab.export_context).range(0..=100));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Hanya hasil").clicked() {
                            do_export = Some(0);
                        }
                        if ui.button("Hasil + konteks").clicked() {
                            do_export = Some(tab.export_context);
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button("Tiket Markdown (Jira)")
                            .on_hover_text("Hasil + konteks sebagai Markdown siap paste")
                            .clicked()
                        {
                            do_ticket = Some(tab.export_context);
                        }
                    });
                });
            if let Some(cx) = do_export {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-ekspor.txt")
                    .save_file();
                if let Some(p) = out {
                    let tab = &mut self.tabs[cur_idx];
                    match tab.doc.export_hits_to_file(&p, cx) {
                        Ok(n) => {
                            tab.doc.status =
                                format!("Diekspor {} baris ke {}.", n, p.display());
                            tab.export_open = false;
                        }
                        Err(e) => tab.doc.status = e,
                    }
                }
            }
            if let Some(cx) = do_ticket {
                let out = rfd::FileDialog::new()
                    .set_file_name("asislog-tiket.md")
                    .save_file();
                if let Some(p) = out {
                    let tab = &mut self.tabs[cur_idx];
                    let q = tab.search_text.clone();
                    match tab.doc.export_ticket_to_file(&p, &q, cx) {
                        Ok(n) => {
                            tab.doc.status = format!(
                                "Tiket ({} baris konteks) disimpan ke {}.",
                                n,
                                p.display()
                            );
                            tab.export_open = false;
                        }
                        Err(e) => tab.doc.status = e,
                    }
                }
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
            egui::Window::new("Cakupan pencarian (baris)")
                .collapsible(false)
                .show(ctx, |ui| {
                    let tab = &mut self.tabs[cur_idx];
                    ui.label("Cari hanya dalam rentang baris ini. Hemat untuk file besar.");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Dari");
                        ui.text_edit_singleline(&mut tab.scope_a);
                        ui.label("Sampai");
                        ui.text_edit_singleline(&mut tab.scope_b);
                    });
                    if !tab.scope_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, tab.scope_msg.clone());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Terapkan").clicked() {
                            do_apply = true;
                        }
                        if ui.button("Bersihkan").clicked() {
                            do_clear = true;
                        }
                        if ui.button("Batal").clicked() {
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
                            tab.doc.status = format!(
                                "Cakupan {}-{} aktif; ketik query untuk mencari.",
                                format_count(a),
                                format_count(b)
                            );
                        }
                    }
                    _ => {
                        self.tabs[cur_idx].scope_msg =
                            String::from("Rentang tidak valid. Contoh: 1000000 sampai 2000000.");
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
            egui::Window::new("Simpan pencarian sebagai preset")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Nama preset:");
                    ui.text_edit_singleline(&mut self.preset_save_name);
                    ui.horizontal(|ui| {
                        if ui.button("Simpan").clicked() {
                            do_save = true;
                        }
                        if ui.button("Batal").clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_save {
                let name = self.preset_save_name.trim().to_string();
                if name.is_empty() {
                    self.global_status = String::from("Nama preset tidak boleh kosong.");
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
