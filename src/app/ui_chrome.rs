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
        let follow_ms = self.follow_ms;
        for t in self.tabs.iter_mut() {
            t.follow_ms = follow_ms;
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
        // P1-10: file path dari instance kedua (single-instance IPC).
        if let Some(rx) = &self.ipc_rx {
            let incoming: Vec<PathBuf> = rx.try_iter().collect();
            if !incoming.is_empty() {
                for p in incoming {
                    self.open_file(p);
                }
            }
        }
        // P1-7: event watch native -> poll follow segera (fallback polling
        // tetap jalan bila event terlewat). Cocokkan per tab agar file lain
        // tak ikut bangun.
        let watch_events: Vec<PathBuf> = if let Some(rx) = &self.watch_rx {
            rx.try_iter().collect()
        } else {
            Vec::new()
        };
        if !watch_events.is_empty() {
            let mut need_repaint = false;
            for t in self.tabs.iter_mut() {
                if !t.doc.follow {
                    continue;
                }
                let hit = watch_events
                    .iter()
                    .any(|ev| crate::engine::watch::watch_matches(&t.doc.path, ev));
                if hit {
                    // Paksa poll_follow pada iterasi di bawah (lewati throttle).
                    t.last_follow_poll =
                        Instant::now() - Duration::from_secs(3600);
                    need_repaint = true;
                }
            }
            if need_repaint {
                ctx.request_repaint();
            }
        }
        // Hasil cek versi latar (sekali jalan per klik/startup).
        if let Some(rx) = &self.update_rx {
            let done: Vec<crate::app::update::UpdateOutcome> = rx.try_iter().collect();
            if let Some(last) = done.into_iter().last() {
                let lang = self.lang;
                self.update_status = match last {
                    crate::app::update::UpdateOutcome::Newer(v) => lang.f2(
                        "Versi baru tersedia: {} (kini {}).",
                        v,
                        env!("CARGO_PKG_VERSION"),
                    ),
                    crate::app::update::UpdateOutcome::Current(v) => {
                        lang.f1("Sudah versi terbaru ({}).", v)
                    }
                    crate::app::update::UpdateOutcome::Failed(e) => {
                        lang.f1("Gagal cek versi: {}", lang.tr_status(&e))
                    }
                };
                self.update_rx = None;
            }
        }
    }
}

impl AsisLogApp {
    pub(crate) fn render_toolbar(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        // Layar Penuh (dulu "Zen"): 1 baris dinamis ramping, menghemat ~150px vertikal
        if self.zen_mode {
            egui::TopBottomPanel::top("zen_toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    logo_icons::paint_logo(ui, 20.0);
                    ui.strong("AsisLog");
                    ui.separator();
                    if !self.tabs.is_empty() {
                        let cur_idx = self.current.min(self.tabs.len() - 1);
                        ui.label(format!("Tab: {}", self.tabs[cur_idx].display_name()));
                        if self.tabs.len() > 1 {
                            crate::ui::icons::menu_drop_down(ui, lang.tr("Ganti tab"), |ui| {
                                for (i, t) in self.tabs.iter().enumerate() {
                                    if ui.selectable_label(i == self.current, t.display_name()).clicked() {
                                        self.current = i;
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(lang.tr("Keluar Layar Penuh (F11)")).on_hover_text(lang.tr("Kembalikan bilah kontrol lengkap (F11)")).clicked() {
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
                crate::ui::icons::menu_drop_down(ui, lang.tr("Riwayat"), |ui| {
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
                crate::ui::icons::menu_drop_down(ui, lang.tr("URL/teks"), |ui| {
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
                crate::ui::icons::menu_drop_down(ui, lang.tr("Workspace"), |ui| {
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
                // Menu Tampilan ringkas & modern: tema, font, font UI, bahasa
                crate::ui::icons::menu_drop_down(ui, lang.tr("Tampilan"), |ui| {
                    ui.label(lang.tr("Tema:"));
                    for t in Tema::semua() {
                        if ui.selectable_label(self.tema == *t, t.nama_in(lang)).clicked() {
                            self.tema = *t;
                            self.cfg_dirty = true;
                        }
                    }
                    ui.separator();
                    ui.label(lang.tr("Font Log:"));
                    for f in ["Bawaan", "JetBrains Mono", "Consolas"] {
                        if ui.selectable_label(self.font_family == f, lang.font_name(f)).clicked() {
                            self.font_family = f.to_string();
                            self.apply_fonts(ctx);
                            self.cfg_dirty = true;
                        }
                    }
                    ui.separator();
                    ui.label(lang.tr("Font UI:"));
                    for (key, label) in [("system", lang.tr("Sistem")), ("default", lang.tr("Bawaan"))] {
                        if ui.selectable_label(self.ui_font == key, label).clicked() {
                            self.ui_font = key.to_string();
                            self.apply_fonts(ctx);
                            self.cfg_dirty = true;
                        }
                    }
                    ui.separator();
                    ui.label(lang.tr("Bahasa:"));
                    for l in [crate::i18n::Lang::Id, crate::i18n::Lang::En] {
                        if ui.selectable_label(lang == l, l.label()).clicked() {
                            self.set_lang(l);
                        }
                    }
                });

                // Tombol utilitas ringkas di kanan: Palet, Bagi, Layar Penuh, Pengaturan, Bantuan
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(lang.f1("v{} · i", env!("CARGO_PKG_VERSION")))
                        .on_hover_text(lang.tr("Tentang AsisLog (versi + log fitur)"))
                        .clicked()
                    {
                        self.about_open = true;
                    }
                    if ui.button("?").on_hover_text(lang.tr("Daftar pintasan (F1)")).clicked() {
                        self.shortcuts_open = true;
                    }
                    if ui
                        .button(lang.tr("Pengaturan"))
                        .on_hover_text(lang.tr("Pengaturan AsisLog (Ctrl+,)"))
                        .clicked()
                    {
                        self.options_open = !self.options_open;
                    }
                    if ui.button(lang.tr("Layar Penuh (F11)")).on_hover_text(lang.tr("Layar Penuh: sembunyikan 5 baris kontrol ke 1 baris ramping (F11)")).clicked() {
                        self.zen_mode = true;
                        self.cfg_dirty = true;
                    }
                    if ui
                        .add(egui::Button::selectable(self.split_view, lang.tr("Bagi")))
                        .on_hover_text(lang.tr("Dual-pane: panel hasil selalu terbuka lebar"))
                        .clicked()
                    {
                        self.split_view = !self.split_view;
                        self.cfg_dirty = true;
                        if self.split_view {
                            if let Some(t) = self.tabs.get_mut(self.current) {
                                t.results_collapsed = false;
                            }
                        }
                    }
                    if ui.button(lang.tr("Palet")).on_hover_text("Command Palette (Ctrl+Shift+P)").clicked() {
                        self.palette_open = true;
                        self.palette_query.clear();
                        self.palette_selected = 0;
                    }
                });
            });
            // Baris tab (gulir mendatar bila banyak, tidak terpotong).
            // P1-15: klik-tengah tutup; drag reorder; klik kanan menu konteks
            // (tutup / pindah / rename / salin path / buka folder).
            if !self.tabs.is_empty() {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut close_idx: Option<usize> = None;
                            let mut ctx_action: Option<(usize, u8)> = None;
                            let mut move_tab: Option<(usize, isize)> = None;
                            let n_tabs = self.tabs.len();
                            for (i, t) in self.tabs.iter().enumerate() {
                                let sel = i == self.current;
                                let base = t.display_name();
                                let name = if sel {
                                    format!("* {}", base)
                                } else {
                                    base.to_string()
                                };
                                let lab = ui.selectable_label(sel, name).on_hover_text(t.doc.path.display().to_string());
                                // Klik tengah = tutup (klogg parity).
                                if lab.middle_clicked() {
                                    close_idx = Some(i);
                                }
                                if lab.clicked() {
                                    self.current = i;
                                }
                                // Drag-reorder: seret tab ke posisi tetangga.
                                if lab.drag_started() {
                                    self.tab_drag = Some(i);
                                }
                                if lab.hovered() {
                                    if let Some(src) = self.tab_drag {
                                        if src != i && ctx.input(|inp| inp.pointer.button_down(egui::PointerButton::Primary)) {
                                            move_tab = Some((src, i as isize - src as isize));
                                            self.tab_drag = Some(i);
                                        }
                                    }
                                }
                                // Menu konteks tab (klik kanan).
                                let lang2 = self.lang;
                                lab.context_menu(|ui| {
                                    if ui.button(lang2.tr("Tutup tab ini")).clicked() {
                                        ctx_action = Some((i, 0));
                                        ui.close();
                                    }
                                    if ui.button(lang2.tr("Tutup tab lainnya")).clicked() {
                                        ctx_action = Some((i, 1));
                                        ui.close();
                                    }
                                    if ui.button(lang2.tr("Tutup semua tab")).clicked() {
                                        ctx_action = Some((i, 2));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.add_enabled(i > 0, egui::Button::new(lang2.tr("Pindahkan ke kiri"))).clicked() {
                                        ctx_action = Some((i, 5));
                                        ui.close();
                                    }
                                    if ui.add_enabled(i + 1 < n_tabs, egui::Button::new(lang2.tr("Pindahkan ke kanan"))).clicked() {
                                        ctx_action = Some((i, 6));
                                        ui.close();
                                    }
                                    if ui.button(lang2.tr("Ubah nama tab…")).clicked() {
                                        ctx_action = Some((i, 7));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button(lang2.tr("Salin path penuh")).clicked() {
                                        ctx_action = Some((i, 3));
                                        ui.close();
                                    }
                                    if ui.button(lang2.tr("Buka folder file")).clicked() {
                                        ctx_action = Some((i, 4));
                                        ui.close();
                                    }
                                });
                                if ui.small_button("×").clicked() {
                                    close_idx = Some(i);
                                }
                            }
                            if ctx.input(|inp| !inp.pointer.button_down(egui::PointerButton::Primary)) {
                                self.tab_drag = None;
                            }
                            if let Some((src, delta)) = move_tab {
                                let to = (src as isize + delta).clamp(0, self.tabs.len() as isize - 1) as usize;
                                self.move_tab(src, to);
                            }
                            if let Some((i, act)) = ctx_action {
                                match act {
                                    0 => close_idx = Some(i),
                                    1 => {
                                        // Tutup semua kecuali i (klogg parity).
                                        for (j, t) in self.tabs.iter_mut().enumerate() {
                                            if j != i {
                                                t.index_cancel.store(true, Ordering::Relaxed);
                                                t.search_cancel.store(true, Ordering::Relaxed);
                                                t.marker_cancel.store(true, Ordering::Relaxed);
                                            }
                                        }
                                        let keep_path = self.tabs[i].doc.path.clone();
                                        let keep_archive = self.tabs[i].archive_src.clone();
                                        let mut kept = None;
                                        for t in self.tabs.drain(..) {
                                            let is_keep = t.doc.path == keep_path
                                                && t.archive_src == keep_archive;
                                            if is_keep {
                                                kept = Some(t);
                                            } else if let Some(tmp) = t.temp_path {
                                                let _ = std::fs::remove_file(&tmp);
                                                if let Some(dir) = tmp.parent() {
                                                    let _ = std::fs::remove_dir(dir);
                                                }
                                            }
                                        }
                                        self.tabs.clear();
                                        if let Some(t) = kept {
                                            self.tabs.push(t);
                                        }
                                        self.current = 0;
                                        self.session_dirty = true;
                                        self.sync_watch();
                                    }
                                    2 => {
                                        for t in self.tabs.iter_mut() {
                                            t.index_cancel.store(true, Ordering::Relaxed);
                                            t.search_cancel.store(true, Ordering::Relaxed);
                                            t.marker_cancel.store(true, Ordering::Relaxed);
                                        }
                                        self.tabs.clear();
                                        self.sync_watch();
                                    }
                                    3 => {
                                        let p = self.tabs[i].doc.path.display().to_string();
                                        ui.ctx().copy_text(p);
                                    }
                                    4 => {
                                        let p = self.tabs[i].doc.path.clone();
                                        let mut st = String::new();
                                        show_in_explorer(&p, &mut st, self.lang);
                                        if let Some(t) = self.tabs.get_mut(i) {
                                            t.doc.status = st;
                                        }
                                    }
                                    5 => {
                                        if i > 0 {
                                            self.move_tab(i, i - 1);
                                        }
                                    }
                                    6 => {
                                        if i + 1 < self.tabs.len() {
                                            self.move_tab(i, i + 1);
                                        }
                                    }
                                    7 => {
                                        self.tab_rename_idx = Some(i);
                                        self.tab_rename_text = self.tabs[i].alias.clone().unwrap_or_default();
                                    }
                                    _ => {}
                                }
                            }
                            if let Some(i) = close_idx {
                                self.tabs[i].index_cancel.store(true, Ordering::Relaxed);
                                self.tabs[i].search_cancel.store(true, Ordering::Relaxed);
                                self.tabs[i].marker_cancel.store(true, Ordering::Relaxed);
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
                                self.sync_watch();
                            }
                        });
                    });
            }
        });
    }
}
