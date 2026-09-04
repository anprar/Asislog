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
        // ---- bar bilah atas: sesi file ----
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Logo + nama di kiri (PNG baked; fallback vektor anti-tofu).
                if let Some(img) = logo_icons::logo_image() {
                    ui.add(img.fit_to_exact_size(egui::vec2(26.0, 26.0)));
                } else {
                    logo_icons::paint_logo_fallback(ui, 26.0);
                }
                ui.strong("AsisLog");
                ui.separator();
                if ui.button("Buka").clicked() {
                    self.open_dialog();
                }
                // Riwayat file + favorit.
                ui.menu_button("Riwayat v", |ui| {
                    let mut open: Option<PathBuf> = None;
                    let mut missing_recent: Option<String> = None;
                    let mut fav_toggle: Option<String> = None;
                    if !self.favorites.is_empty() {
                        ui.label("Favorit:");
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
                                        self.global_status = format!(
                                            "File favorit tak ditemukan: {}",
                                            r
                                        );
                                    }
                                    ui.close();
                                }
                                if ui
                                    .small_button("F")
                                    .on_hover_text("Lepas favorit")
                                    .clicked()
                                {
                                    fav_toggle = Some(r);
                                }
                            });
                        }
                        ui.separator();
                    }
                    ui.label("Terakhir dibuka:");
                    if self.recent.is_empty() {
                        ui.label("Belum ada riwayat.");
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
                                .on_hover_text("Jadikan favorit")
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
                            format!("File tidak ditemukan, dihapus dari riwayat: {}", m);
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
                    if ui.button("Bersihkan riwayat").clicked() {
                        // Via modal konfirmasi (tak langsung).
                        self.confirm = Some(ConfirmAction::ClearRecent);
                        ui.close();
                    }
                    if let Some(p) = open {
                        self.open_file(p);
                    }
                });
                // Buka dari URL / tempel teks (pekerjaan support).
                ui.menu_button("URL/teks v", |ui| {
                    if ui
                        .button("Buka URL…")
                        .on_hover_text("Unduh http(s) ke temp lalu buka")
                        .clicked()
                    {
                        self.url_open = true;
                        ui.close();
                    }
                    if ui
                        .button("Tempel teks…")
                        .on_hover_text("Tempel teks (Ctrl+V) lalu buka sebagai file")
                        .clicked()
                    {
                        self.paste_open = true;
                        ui.close();
                    }
                });
                // Workspace produk: N log + filter + set sorotan + rentang waktu.
                ui.menu_button("Workspace v", |ui| {
                    if ui
                        .button("Simpan workspace…")
                        .on_hover_text("Simpan tab + filter + set sorotan ke 1 file JSON")
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
                        .button("Buka workspace…")
                        .on_hover_text("Buka N log + filter + set sorotan dari file")
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                        {
                            match crate::store::load_workspace(&p) {
                                Ok(ws) => self.open_workspace(ws),
                                Err(e) => self.global_status = e,
                            }
                        }
                        ui.close();
                    }
                });
                if self.tabs.is_empty() {
                    ui.label("Belum ada file. Seret .log / .txt ke sini atau tekan Buka.");
                }
                // Tema + bantuan di kanan baris sesi.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("?").on_hover_text("Daftar pintasan (F1)").clicked() {
                        self.shortcuts_open = true;
                    }
                    egui::ComboBox::from_label("Tema")
                        .selected_text(self.tema.nama())
                        .show_ui(ui, |ui| {
                            for t in Tema::semua() {
                                if ui.selectable_label(self.tema == *t, t.nama()).clicked() {
                                    self.tema = *t;
                                    self.cfg_dirty = true;
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
