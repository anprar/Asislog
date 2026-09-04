// English comments: Dialog: highlight sets manager (split from app.rs; behavior unchanged).
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
    pub(crate) fn render_highlight(&mut self, ctx: &egui::Context) {
        // ---- jendela set sorotan ----
        if self.hl_open {
            let mut dirty = false;
            let mut do_export = false;
            let mut do_import = false;
            egui::Window::new("Set highlight (viewport)")
                .collapsible(false)
                .resizable(true)
                .default_width(440.0)
                .show(ctx, |ui| {
                    ui.label("Set bernama per produk; aturan hanya untuk baris terlihat.");
                    // Pemilih set + buat + hapus.
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Set:");
                        let cur = self
                            .active_set
                            .clone()
                            .unwrap_or_else(|| String::from("-"));
                        egui::ComboBox::from_id_salt("hlset")
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                for s in &self.sets {
                                    let mut sel = self.active_set.as_deref() == Some(&s.name);
                                    if ui.checkbox(&mut sel, s.name.clone()).clicked() {
                                        self.active_set = Some(s.name.clone());
                                        dirty = true;
                                    }
                                }
                            });
                        ui.text_edit_singleline(&mut self.hl_set_name);
                        if ui.button("Buat").clicked() {
                            let name = self.hl_set_name.trim().to_string();
                            if name.is_empty() {
                                self.global_status =
                                    String::from("Nama set tidak boleh kosong.");
                            } else if self.sets.iter().any(|s| s.name == name) {
                                self.global_status =
                                    String::from("Set dengan nama itu sudah ada.");
                            } else {
                                self.sets.push(HighlightSet { name: name.clone(), rules: Vec::new() });
                                self.active_set = Some(name);
                                self.hl_set_name.clear();
                                dirty = true;
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Ekspor set…").clicked() {
                            do_export = true;
                        }
                        if ui.button("Impor set…").clicked() {
                            do_import = true;
                        }
                        if ui.button("Hapus set").clicked() {
                            if let Some(n) = self.active_set.clone() {
                                self.sets.retain(|s| s.name != n);
                                self.active_set =
                                    self.sets.first().map(|s| s.name.clone());
                                dirty = true;
                            }
                        }
                    });
                    ui.separator();
                    // Aturan milik set aktif.
                    let active = self.active_set.clone().unwrap_or_default();
                    let idx = self.sets.iter().position(|s| s.name == active);
                    if let Some(si) = idx {
                        let mut del: Option<usize> = None;
                        // Pinjam rules saja agar field form tetap bebas.
                        let rules = &mut self.sets[si].rules;
                        for (i, r) in rules.iter_mut().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                let show = format!(
                                    "{} [{}] {}",
                                    if r.enabled { "[x]" } else { "[ ]" },
                                    if r.regex { "regex" } else { "teks" },
                                    r.name
                                );
                                if ui.small_button(show).clicked() {
                                    r.enabled = !r.enabled;
                                    dirty = true;
                                }
                                ui.label(format!("\"{}\" -> {}", r.pattern, color_name_id(&r.color)));
                                if ui.small_button("×").on_hover_text("Hapus aturan").clicked() {
                                    del = Some(i);
                                }
                            });
                        }
                        if let Some(i) = del {
                            self.sets[si].rules.remove(i);
                            dirty = true;
                        }
                    } else {
                        ui.label("Belum ada set. Buat set dulu di atas.");
                    }
                    ui.separator();
                    ui.label("Tambah aturan ke set aktif:");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Nama");
                        ui.text_edit_singleline(&mut self.hl_name);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Pola");
                        ui.text_edit_singleline(&mut self.hl_pattern);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.hl_regex, "Regex");
                        ui.checkbox(&mut self.hl_case, "Peka huruf");
                        ui.checkbox(&mut self.hl_whole, "Baris penuh");
                        egui::ComboBox::from_id_salt("hlcolor")
                            .selected_text(color_name_id(
                                highlight_keys()[self.hl_color_idx % highlight_keys().len()].0,
                            ))
                            .show_ui(ui, |ui| {
                                for (k, nama) in highlight_keys() {
                                    ui.selectable_value(&mut self.hl_color_idx, key_index(k), *nama);
                                }
                            });
                    });
                    if ui.button("Tambah").clicked() {
                        let keys = highlight_keys();
                        let key = keys[self.hl_color_idx % keys.len()].0.to_string();
                        let rule = HighlightRule {
                            name: if self.hl_name.trim().is_empty() {
                                self.hl_pattern.trim().to_string()
                            } else {
                                self.hl_name.trim().to_string()
                            },
                            pattern: self.hl_pattern.trim().to_string(),
                            regex: self.hl_regex,
                            case_sensitive: self.hl_case,
                            color: key,
                            whole_line: self.hl_whole,
                            enabled: true,
                        };
                        match rule.validate() {
                            Ok(()) => {
                                let active = self.active_set.clone().unwrap_or_default();
                                if let Some(s) =
                                    self.sets.iter_mut().find(|s| s.name == active)
                                {
                                    s.rules.push(rule);
                                    self.hl_name.clear();
                                    self.hl_pattern.clear();
                                    dirty = true;
                                } else {
                                    self.global_status = String::from(
                                        "Buat/pilih set dulu sebelum menambah aturan.",
                                    );
                                }
                            }
                            Err(e) => self.global_status = e,
                        }
                    }
                    if ui.button("Tutup").clicked() {
                        self.hl_open = false;
                    }
                });
            if do_export {
                if let Some(n) = self.active_set.clone() {
                    if let Some(s) = self.sets.iter().find(|s| s.name == n) {
                        let fname = format!("{}-highlight.json", sanitize_name(&n));
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name(fname)
                            .save_file()
                        {
                            match serde_json::to_string_pretty(s) {
                                Ok(text) => match std::fs::write(&p, text) {
                                    Ok(()) => {
                                        self.global_status = format!(
                                            "Set '{}' diekspor ke {}.",
                                            n,
                                            p.display()
                                        )
                                    }
                                    Err(e) => {
                                        self.global_status =
                                            format!("Gagal menulis: {}", e)
                                    }
                                },
                                Err(e) => {
                                    self.global_status = format!("Gagal menyusun: {}", e)
                                }
                            }
                        }
                    }
                }
            }
            if do_import {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
                {
                    match std::fs::read_to_string(&p) {
                        Ok(text) => {
                            // Terima set penuh atau daftar aturan polos.
                            let parsed: Option<HighlightSet> =
                                serde_json::from_str(&text).ok().or_else(|| {
                                    serde_json::from_str::<Vec<HighlightRule>>(&text)
                                        .ok()
                                        .map(|rules| HighlightSet {
                                            name: p
                                                .file_stem()
                                                .map(|s| s.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| {
                                                    String::from("Impor")
                                                }),
                                            rules,
                                        })
                                });
                            match parsed {
                                Some(mut set) => {
                                    let mut skipped = 0;
                                    set.rules.retain(|r| {
                                        let ok = r.validate().is_ok();
                                        if !ok {
                                            skipped += 1;
                                        }
                                        ok
                                    });
                                    let base = set.name.clone();
                                    let mut name = base.clone();
                                    let mut n = 1;
                                    while self.sets.iter().any(|s| s.name == name) {
                                        n += 1;
                                        name = format!("{} ({})", base, n);
                                    }
                                    set.name = name.clone();
                                    self.sets.push(set);
                                    self.active_set = Some(name.clone());
                                    dirty = true;
                                    self.global_status = format!(
                                        "Set '{}' diimpor{}.",
                                        name,
                                        if skipped > 0 {
                                            format!(", {} aturan salah dilewati", skipped)
                                        } else {
                                            String::new()
                                        }
                                    );
                                }
                                None => {
                                    self.global_status =
                                        String::from("File bukan set highlight yang valid.");
                                }
                            }
                        }
                        Err(e) => self.global_status = format!("Gagal membaca: {}", e),
                    }
                }
            }
            if dirty {
                self.hl_dirty = true;
                self.save_config();
            }
        }
    }
}
