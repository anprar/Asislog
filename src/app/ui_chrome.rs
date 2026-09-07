// English comments: Frame preamble: poll background jobs (split from app.rs; behavior unchanged).
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
use crate::ui::icons as logo_icons;
use super::*;

impl AsisLogApp {
    pub(crate) fn poll_background(&mut self, ctx: &egui::Context) {
        // Drag-drop
        let dropped: Vec<PathBuf> = ctx
            .input(|i| i.raw.dropped_files.clone())
            .into_iter()
            .filter_map(|f| f.path)
            .collect();
        for p in dropped {
            self.open_file(p);
        }

        self.handle_shortcuts(ctx);

        // Terapkan tema pilihan (hanya saat berubah agar hemat).
        // `Sistem` dibaca dari OS tiap frame sehingga mengikuti perubahan OS.
        let (vis, dark_now) = self.tema.resolve(ctx);
        if (self.tema, dark_now) != self.tema_state {
            ctx.set_visuals(vis);
            self.tema_state = (self.tema, dark_now);
        }
        // Bangun ulang cache sorotan bila aturan berubah.
        if self.hl_dirty {
            self.rebuild_highlights();
        }

        // Poll background per tab (sebelum pinjam mut untuk UI)
        // History dipinjam terpisah (field lain) agar start_search bisa mencatat.
        let history = &mut self.history;
        let mut session_touched = false;
        for t in self.tabs.iter_mut() {
            t.poll_channels();
            t.poll_follow();
            // Penanda berubah -> tulis sidecar (best effort).
            if t.marks_dirty {
                if let Err(e) = crate::engine::marks::save(&t.doc.path, &t.doc.bookmarks) {
                    t.doc.status = e;
                }
                t.marks_dirty = false;
            }
            // debounce pencarian 150 ms
            if let Some(at) = t.debounce_at {
                if Instant::now() >= at {
                    t.start_search(history);
                    session_touched = true;
                }
            }
            // Posisi gulir berubah -> sesi perlu disimpan ulang (debounce).
            if t.top_row != t.saved_top {
                t.saved_top = t.top_row;
                session_touched = true;
            }
        }
        if session_touched {
            self.session_dirty = true;
        }
        // Flush sesi (debounce 10 dtk) + config kotor.
        if self.session_dirty && self.last_session_save.elapsed() > Duration::from_secs(10) {
            self.save_session();
        }
        if self.cfg_dirty {
            self.save_config();
        }
        // Unduhan URL selesai? Kumpulkan dulu agar tak pinjam saat open_file.
        let dl_done: Vec<DlMsg> = if let Some(rx) = &self.dl_rx {
            rx.try_iter().collect()
        } else {
            Vec::new()
        };
        if !dl_done.is_empty() {
            self.dl_rx = None;
            for m in dl_done {
                match m {
                    DlMsg::Done(p) => self.open_file(p),
                    DlMsg::Failed(e) => self.global_status = e,
                }
            }
        }
    }
}

impl AsisLogApp {
    pub(crate) fn render_toolbar(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        // Mode Zen (C-B1): 1 baris dinamis ramping, menghemat ~150px vertikal
        if self.zen_mode {
            egui::TopBottomPanel::top("zen_toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    logo_icons::paint_logo(ui, 20.0);
                    ui.strong("AsisLog");
                    ui.separator();
                    if !self.tabs.is_empty() {
                        let cur_idx = self.current.min(self.tabs.len() - 1);
                        ui.label(format!("Tab: {}", self.tabs[cur_idx].doc.file_name));
                        if self.tabs.len() > 1 {
                            ui.menu_button(lang.tr("Ganti tab v"), |ui| {
                                for (i, t) in self.tabs.iter().enumerate() {
                                    if ui.selectable_label(i == self.current, &t.doc.file_name).clicked() {
                                        self.current = i;
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(lang.tr("Keluar Zen (F11)")).on_hover_text(lang.tr("Kembalikan bilah kontrol lengkap (F11)")).clicked() {
                            self.zen_mode = false;
                            self.cfg_dirty = true;
                        }
                        if ui.button(lang.tr("Cari (Ctrl+F)")).on_hover_text(lang.tr("Buka HUD pencarian melayang")).clicked() {
                            self.zen_search_open = !self.zen_search_open;
                        }
                        if ui.button(lang.tr("Palet (Ctrl+Shift+P)")).clicked() {
                            self.palette_open = true;
                            self.palette_query.clear();
                            self.palette_selected = 0;
                        }
                    });
                });
            });
            return;
        }

        // ---- bar bilah atas standar: sesi file ----
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Logo vektor AsisLog anti-tofu & DPI-crisp.
                logo_icons::paint_logo(ui, 24.0);
                ui.strong("AsisLog");
                ui.separator();
                if ui.button(lang.tr("Buka")).clicked() {
                    self.open_dialog();
                }
                // Riwayat file + favorit.
                ui.menu_button(lang.tr("Riwayat v"), |ui| {
                    let mut open: Option<PathBuf> = None;
                    let mut missing_recent: Option<String> = None;
                    let mut fav_toggle: Option<String> = None;
                    if !self.favorites.is_empty() {
                        ui.label(lang.tr("Favorit:"));
                        for r in self.favorites.clone() {
                            ui.horizontal(|ui| {
                                let name = PathBuf::from(&r)
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| r.clone());
                                if ui.button(name).on_hover_text(r.clone()).clicked() {
                                    let p = PathBuf::from(&r);
                                    if p.exists() {
                                        open = Some(p);
                                    } else {
                                        self.global_status = lang.f1("File favorit tak ditemukan: {}", &r);
                                    }
                                    ui.close();
                                }
                                if ui
                                    .small_button("F")
                                    .on_hover_text(lang.tr("Lepas favorit"))
                                    .clicked()
                                {
                                    fav_toggle = Some(r);
                                }
                            });
                        }
                        ui.separator();
                    }
                    ui.label(lang.tr("Terakhir dibuka:"));
                    if self.recent.is_empty() {
                        ui.label(lang.tr("Belum ada riwayat."));
                    }
                    for r in self.recent.clone() {
                        ui.horizontal(|ui| {
                            let name = PathBuf::from(&r)
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| r.clone());
                            if ui.button(name).on_hover_text(r.clone()).clicked() {
                                let p = PathBuf::from(&r);
                                if p.exists() {
                                    open = Some(p);
                                } else {
                                    missing_recent = Some(r.clone());
                                }
                                ui.close();
                            }
                            let is_fav = self.favorites.iter().any(|f| f == &r);
                            if ui
                                .small_button(if is_fav { "[F]" } else { "F" })
                                .on_hover_text(lang.tr("Jadikan favorit"))
                                .clicked()
                            {
                                fav_toggle = Some(r);
                            }
                        });
                    }
                    if let Some(m) = missing_recent {
                        self.recent.retain(|p| p != &m);
                        self.save_config();
                        self.global_status =
                            lang.f1("File tidak ditemukan, dihapus dari riwayat: {}", m);
                    }
                    if let Some(f) = fav_toggle {
                        if self.favorites.iter().any(|x| x == &f) {
                            self.favorites.retain(|x| x != &f);
                        } else {
                            self.favorites.insert(0, f);
                            self.favorites.truncate(20);
                        }
                        self.save_config();
                    }
                    ui.separator();
                    if ui.button(lang.tr("Bersihkan riwayat")).clicked() {
                        // Via modal konfirmasi (tak langsung).
                        self.confirm = Some(ConfirmAction::ClearRecent);
                        ui.close();
                    }
                    if let Some(p) = open {
                        self.open_file(p);
                    }
                });
                // Buka dari URL / tempel teks (pekerjaan support).
                ui.menu_button(lang.tr("URL/teks v"), |ui| {
                    if ui
                        .button(lang.tr("Buka URL…"))
                        .on_hover_text(lang.tr("Unduh http(s) ke temp lalu buka"))
                        .clicked()
                    {
                        self.url_open = true;
                        ui.close();
                    }
                    if ui
                        .button(lang.tr("Tempel teks…"))
                        .on_hover_text(lang.tr("Tempel teks (Ctrl+V) lalu buka sebagai file"))
                        .clicked()
                    {
                        self.paste_open = true;
                        ui.close();
                    }
                });
                // Workspace produk: N log + filter + set sorotan + rentang waktu.
                ui.menu_button(lang.tr("Workspace v"), |ui| {
                    if ui
                        .button(lang.tr("Simpan workspace…"))
                        .on_hover_text(lang.tr("Simpan tab + filter + set sorotan ke 1 file JSON"))
                        .clicked()
                    {
                        let def = "workspace-asislog.json".to_string();
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .set_file_name(def)
                            .save_file()
                        {
                            self.save_workspace_to(&p);
                        }
                        ui.close();
                    }
                    if ui
                        .button(lang.tr("Buka workspace…"))
                        .on_hover_text(lang.tr("Buka N log + filter + set sorotan dari file"))
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                        {
                            match crate::store::load_workspace(&p) {
                                Ok(ws) => self.open_workspace(ws),
                                Err(e) => self.global_status = lang.tr_status(&e),
                            }
                        }
                        ui.close();
                    }
                });
                if self.tabs.is_empty() {
                    ui.label(lang.tr("Belum ada file. Seret .log / .txt ke sini atau tekan Buka."));
                }
                // Tema, bahasa, font, zen, palet, dan bantuan di kanan baris sesi.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("?").on_hover_text(lang.tr("Daftar pintasan (F1)")).clicked() {
                        self.shortcuts_open = true;
                    }
                    if ui.button(lang.tr("Zen (F11)")).on_hover_text(lang.tr("Mode Zen: sembunyikan 5 baris kontrol ke 1 baris ramping (F11)")).clicked() {
                        self.zen_mode = true;
                        self.cfg_dirty = true;
                    }
                    if ui.button(lang.tr("Palet")).on_hover_text("Command Palette (Ctrl+Shift+P)").clicked() {
                        self.palette_open = true;
                        self.palette_query.clear();
                        self.palette_selected = 0;
                    }
                    egui::ComboBox::from_id_salt("font_choice")
                        .selected_text(format!("Font: {}", self.font_family))
                        .show_ui(ui, |ui| {
                            for f in ["Bawaan", "JetBrains Mono", "Consolas"] {
                                if ui.selectable_label(self.font_family == f, f).clicked() {
                                    self.font_family = f.to_string();
                                    self.cfg_dirty = true;
                                }
                            }
                        });
                    egui::ComboBox::from_label(lang.tr("Tema"))
                        .selected_text(self.tema.nama_in(lang))
                        .show_ui(ui, |ui| {
                            for t in Tema::semua() {
                                if ui.selectable_label(self.tema == *t, t.nama_in(lang)).clicked() {
                                    self.tema = *t;
                                    self.cfg_dirty = true;
                                }
                            }
                        });
                    egui::ComboBox::from_label(lang.tr("Bahasa/Language"))
                        .selected_text(lang.label())
                        .show_ui(ui, |ui| {
                            for l in [crate::i18n::Lang::Id, crate::i18n::Lang::En] {
                                if ui.selectable_label(lang == l, l.label()).clicked() {
                                    self.set_lang(l);
                                }
                            }
                        });
                });
            });
            // Baris tab (gulir mendatar bila banyak, tidak terpotong).
            if !self.tabs.is_empty() {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut close_idx: Option<usize> = None;
                            for (i, t) in self.tabs.iter().enumerate() {
                                let sel = i == self.current;
                                let name = if sel {
                                    format!("* {}", t.doc.file_name)
                                } else {
                                    t.doc.file_name.clone()
                                };
                                if ui.selectable_label(sel, name).clicked() {
                                    self.current = i;
                                }
                                if ui.small_button("×").clicked() {
                                    close_idx = Some(i);
                                }
                            }
                            if let Some(i) = close_idx {
                                self.tabs[i].index_cancel.store(true, Ordering::Relaxed);
                                self.tabs[i].search_cancel.store(true, Ordering::Relaxed);
                                let tab = self.tabs.remove(i);
                                // Bersihkan temp ekstrak arsip (best effort).
                                if let Some(t) = tab.temp_path {
                                    let _ = std::fs::remove_file(&t);
                                    if let Some(dir) = t.parent() {
                                        let _ = std::fs::remove_dir(dir);
                                    }
                                }
                                if self.current >= self.tabs.len() && !self.tabs.is_empty() {
                                    self.current = self.tabs.len() - 1;
                                }
                                self.session_dirty = true;
                            }
                        });
                    });
            }
        });
    }
}
