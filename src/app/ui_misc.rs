// English comments: Dialogs: rename mark, time range, URL, paste, scratchpad, help, error (split from app.rs; behavior unchanged).
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
    pub(crate) fn render_misc(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        // ---- dialog ubah label penanda ----
        if self.rename_open {
            let mut do_save = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Ubah label penanda"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(lang.f1("Baris {}", format_count(self.rename_line)));
                    ui.label(lang.tr("Label:"));
                    ui.text_edit_singleline(&mut self.rename_label);
                    egui::ComboBox::from_label(lang.tr("Warna"))
                        .selected_text(self.rename_color.nama_in(lang))
                        .show_ui(ui, |ui| {
                            for c in BookmarkColor::semua() {
                                ui.selectable_value(&mut self.rename_color, *c, c.nama_in(lang));
                            }
                        });
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
                if let Some(t) = self.tabs.get_mut(cur_idx) {
                    if let Some(b) =
                        t.doc.bookmarks.iter_mut().find(|b| b.line == self.rename_line)
                    {
                        b.label = self.rename_label.clone();
                        b.color = self.rename_color;
                        t.marks_dirty = true;
                        t.doc.status = lang.f1("Penanda baris {} diperbarui.", self.rename_line);
                    }
                }
                self.rename_open = false;
            }
            if do_close {
                self.rename_open = false;
            }
        }

        // ---- dialog rentang waktu ----
        if self.range_open {
            let mut do_go = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Tampilkan rentang waktu"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(lang.tr("Contoh: 2026-08-24 13:00:00 sampai 2026-08-24 14:00:00"));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(lang.tr("Dari"));
                        ui.text_edit_singleline(&mut self.range_start);
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(lang.tr("Sampai"));
                        ui.text_edit_singleline(&mut self.range_end);
                    });
                    if !self.range_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, lang.tr_status(&self.range_msg));
                    }
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Tampilkan rentang")).clicked() {
                            do_go = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_go {
                let a = self.range_start.clone();
                let b = self.range_end.clone();
                match apply_time_range(&mut self.tabs[cur_idx], &a, &b)
                {
                    Ok(msg) => {
                        self.tabs[cur_idx].doc.status = msg;
                        self.tabs[cur_idx].range_applied = Some((a, b));
                        self.range_msg.clear();
                        self.range_open = false;
                    }
                    Err(e) => self.range_msg = e,
                }
            }
            if do_close {
                self.range_open = false;
                self.range_msg.clear();
            }
        }

        // ---- dialog buka URL ----
        if self.url_open {
            let mut do_dl = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Buka dari URL"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(lang.tr("Contoh: https://server/app.log"));
                    ui.text_edit_singleline(&mut self.url_text);
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Unduh & buka")).clicked() {
                            do_dl = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_dl {
                let url = self.url_text.trim().to_string();
                if url.is_empty() {
                    self.global_status = lang.tr("URL kosong.").to_string();
                } else {
                    let (tx, rx) = mpsc::channel();
                    self.dl_rx = Some(rx);
                    spawn_download(url, tx);
                    self.global_status = lang.tr("Mengunduh…").to_string();
                    self.url_open = false;
                }
            }
            if do_close {
                self.url_open = false;
            }
        }

        // ---- dialog tempel teks ----
        if self.paste_open {
            let mut do_open = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Tempel teks sebagai file"))
                .collapsible(false)
                .resizable(true)
                .default_width(480.0)
                .show(ctx, |ui| {
                    ui.label(lang.tr("Tempel (Ctrl+V), lalu buka sebagai file temp."));
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 220.0),
                        egui::TextEdit::multiline(&mut self.paste_text)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Buka sebagai file")).clicked() {
                            do_open = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                });
            if do_open {
                if self.paste_text.trim().is_empty() {
                    self.global_status = lang.tr("Teks kosong.").to_string();
                } else {
                    let mut p = std::env::temp_dir();
                    p.push("asislog-tempel");
                    let _ = std::fs::create_dir_all(&p);
                    p.push(format!(
                        "tempel-{}.log",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|t| t.as_millis())
                            .unwrap_or(0)
                    ));
                    match std::fs::write(&p, self.paste_text.clone()) {
                        Ok(()) => {
                            self.paste_open = false;
                            self.open_file(p);
                        }
                        Err(e) => self.global_status = lang.f1("Gagal menulis temp: {}", e),
                    }
                }
            }
            if do_close {
                self.paste_open = false;
            }
        }

        // ---- jendela scratchpad ----
        if self.scratch_open {
            egui::Window::new(lang.tr("Scratchpad (catatan + transform)"))
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .show(ctx, |ui| {
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 240.0),
                        egui::TextEdit::multiline(&mut self.scratch_text)
                            .font(egui::TextStyle::Monospace)
                            .hint_text(lang.tr("Catatan, token, JSON, JWT, SQL…")),
                    );
                    if !self.scratch_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, lang.tr_status(&self.scratch_msg));
                    }
                    let (cc, ww, ll) = crate::engine::scratch::stats(&self.scratch_text);
                    ui.label(lang.f3("{} karakter · {} kata · {} baris", cc, ww, ll));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(lang.tr("JSON rapi")).clicked() {
                            match crate::engine::scratch::json_pretty(&self.scratch_text) {
                                Ok(o) => {
                                    self.scratch_text = o;
                                    self.scratch_msg.clear();
                                    self.cfg_dirty = true;
                                }
                                Err(e) => self.scratch_msg = lang.tr_status(&e),
                            }
                        }
                        if ui.button(lang.tr("Base64 decode")).clicked() {
                            match crate::engine::scratch::b64_decode(&self.scratch_text) {
                                Ok(o) => {
                                    self.scratch_text = o;
                                    self.scratch_msg.clear();
                                    self.cfg_dirty = true;
                                }
                                Err(e) => self.scratch_msg = lang.tr_status(&e),
                            }
                        }
                        if ui.button(lang.tr("JWT decode")).clicked() {
                            match crate::engine::scratch::jwt_decode(&self.scratch_text) {
                                Ok(o) => {
                                    self.scratch_text = o;
                                    self.scratch_msg.clear();
                                    self.cfg_dirty = true;
                                }
                                Err(e) => self.scratch_msg = lang.tr_status(&e),
                            }
                        }
                        if ui.button(lang.tr("SQL rapi")).clicked() {
                            self.scratch_text =
                                crate::engine::scratch::sql_tidy(&self.scratch_text);
                            self.scratch_msg.clear();
                            self.cfg_dirty = true;
                        }
                        if ui.button(lang.tr("Bersihkan")).clicked() {
                            self.scratch_text.clear();
                            self.scratch_msg.clear();
                            self.cfg_dirty = true;
                        }
                        if ui.button(lang.tr("Tutup")).clicked() {
                            self.scratch_open = false;
                        }
                    });
                });
        }

        // Jendela daftar pintasan (F1).
        if self.shortcuts_open {            egui::Window::new(lang.tr("Pintasan AsisLog (F1)"))
                .collapsible(false)
                .resizable(true)
                .default_width(380.0)
                .show(ctx, |ui| {
                    for (keys, desc) in [
                        ("Ctrl+O", lang.tr("Buka file log")),
                        ("Ctrl+F", lang.tr("Fokus ke kolom Cari")),
                        ("F3 / Shift+F3", lang.tr("Hasil berikutnya / sebelumnya")),
                        ("n / N", lang.tr("Hasil berikut / sebelum (di luar kolom ketik)")),
                        ("1-9", lang.tr("Label warna dari query aktif")),
                        ("Ctrl+G", lang.tr("Ke baris / persen / akhir / waktu")),
                        ("Ctrl+E", lang.tr("Ekspor hasil pencarian")),
                        ("Ctrl+Home / Ctrl+End", lang.tr("Awal / akhir file")),
                        ("Ctrl+Tab / Ctrl+Shift+Tab", lang.tr("Pindah tab (berlaku juga saat mengetik)")),
                        ("Ctrl+Shift+F", lang.tr("Ikuti akhir file (LIVE)")),
                        ("Ctrl+B", lang.tr("Tandai baris aktif")),
                        ("Ctrl+Shift+B", lang.tr("Panel penanda")),
                        ("F2", lang.tr("Ubah label penanda")),
                        ("Alt+Left / Alt+Right", lang.tr("History mundur / maju")),
                        ("Alt+Atas / Alt+Bawah", lang.tr("Penanda sebelumnya / berikutnya")),
                        ("Ctrl+= / Ctrl+- / Ctrl+0", lang.tr("Zoom UI")),
                        ("PgUp / PgDn, Panah", lang.tr("Gulir viewport (di luar kolom ketik)")),
                        ("Esc", lang.tr("Tutup dialog teratas; lalu batalkan pencarian")),
                    ] {
                        ui.horizontal(|ui| {
                            ui.strong(keys);
                            ui.label(desc);
                        });
                    }
                    if ui.button(lang.tr("Tutup")).clicked() {
                        self.shortcuts_open = false;
                    }
                });
        }

        // Modal konfirmasi hapus (di atas segalanya kecuali galat).
        if let Some(action) = self.confirm.clone() {
            let mut done = false;
            let mut confirmed = false;
            egui::Window::new(action.title_in(lang))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(action.message_in(lang));
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Ya, hapus")).clicked() {
                            confirmed = true;
                            done = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            done = true;
                        }
                    });
                });
            if done {
                self.confirm = None;
            }
            if confirmed {
                match action {
                    ConfirmAction::DeleteMark(ln) => {
                        if let Some(t) = self.tabs.get_mut(cur_idx) {
                            t.doc.bookmarks.retain(|b| b.line != ln);
                            t.marks_dirty = true;
                            t.refresh_mode_map();
                            t.doc.status =
                                lang.f1("Penanda baris {} dihapus.", ln);
                        }
                    }
                    ConfirmAction::ClearRecent => {
                        self.recent.clear();
                        self.save_config();
                        self.global_status =
                            lang.tr("Riwayat file dikosongkan.").to_string();
                    }
                }
            }
        }

        // Global error modal
        if self.global_error.is_some() {
            egui::Window::new(lang.tr("Galat"))
                .collapsible(false)
                .show(ctx, |ui| {
                    if let Some(e) = &self.global_error.clone() {
                        ui.colored_label(egui::Color32::RED, lang.tr_status(e));
                    }
                    if ui.button(lang.tr("Tutup")).clicked() {
                        self.global_error = None;
                    }
                });
        }
    }
}
