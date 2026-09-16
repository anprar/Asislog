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

        // ---- P1-15: dialog ubah nama tab ----
        if let Some(idx) = self.tab_rename_idx {
            let mut do_save = false;
            let mut do_close = false;
            egui::Window::new(lang.tr("Ubah nama tab"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(lang.tr("Nama tab (kosong = nama file):"));
                    ui.text_edit_singleline(&mut self.tab_rename_text);
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
                let alias = self.tab_rename_text.trim().to_string();
                self.rename_tab(idx, if alias.is_empty() { None } else { Some(alias) });
                self.tab_rename_idx = None;
            }
            if do_close {
                self.tab_rename_idx = None;
            }
        }

        // ---- Laporan crash sebelumnya (dari panic hook) ----
        if !self.crash_reports.is_empty() {
            let mut do_copy = false;
            let mut do_folder = false;
            let mut do_delete = false;
            let mut do_later = false;
            egui::Window::new(lang.tr("Laporan crash ditemukan"))
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(lang.tr("AsisLog pernah crash. Isi membantu diagnosis; kirim ke pengelola bila perlu."));
                    for p in &self.crash_reports {
                        let name = p
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.display().to_string());
                        ui.monospace(name);
                    }
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Salin isi")).clicked() {
                            do_copy = true;
                        }
                        if ui.button(lang.tr("Buka folder crash")).clicked() {
                            do_folder = true;
                        }
                        if ui.button(lang.tr("Hapus laporan")).clicked() {
                            do_delete = true;
                        }
                        if ui.button(lang.tr("Nanti")).clicked() {
                            do_later = true;
                        }
                    });
                });
            if do_copy {
                let mut all = String::new();
                for p in &self.crash_reports {
                    if let Ok(t) = std::fs::read_to_string(p) {
                        all.push_str(&p.display().to_string());
                        all.push('\n');
                        all.push_str(&t.chars().take(8192).collect::<String>());
                        all.push('\n');
                    }
                    if all.len() > 65536 {
                        break;
                    }
                }
                ctx.copy_text(all);
                self.global_status = lang.tr("Crash disalin.").to_string();
            }
            if do_folder {
                if let Some(p) = self.crash_reports.first().cloned() {
                    let mut st = String::new();
                    show_in_explorer(&p, &mut st, lang);
                    self.global_status = st;
                }
            }
            if do_delete {
                for p in &self.crash_reports {
                    let _ = std::fs::remove_file(p);
                }
                self.crash_reports.clear();
                self.global_status = lang.tr("Laporan crash dihapus.").to_string();
            }
            if do_later {
                self.crash_reports.clear();
            }
        }

        // ---- dialog Tentang: versi + log fitur (CHANGELOG di-embed) ----
        if self.about_open {
            egui::Window::new(lang.tr("Tentang AsisLog"))
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .default_height(460.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        crate::ui::icons::paint_logo(ui, 36.0);
                        ui.vertical(|ui| {
                            ui.heading("AsisLog");
                            ui.label(lang.f1("Versi {}", env!("CARGO_PKG_VERSION")));
                            ui.weak(lang.tr("Penampil log portabel untuk file sangat besar."));
                        });
                    });
                    ui.separator();
                    ui.strong(lang.tr("Batas & catatan jujur:"));
                    ui.weak(lang.tr(
                        "SQL-lite: maks 2 jt baris pertama · Gabung timeline: 200 rb baris/file · Regex fancy: 256 KB/baris · Hyperscan/Vectorscan tidak tersedia di Windows (NO-GO). Ekspor streaming tanpa batas tampil.",
                    ));
                    ui.separator();
                    ui.strong(lang.tr("Log fitur (CHANGELOG.md):"));
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .max_height(320.0)
                        .show(ui, |ui| {
                            // CHANGELOG di-embed saat build (include_str),
                            // portabel: binary tidak baca file saat runtime.
                            ui.monospace(crate::CHANGELOG_TEXT);
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Salin versi")).clicked() {
                            ctx.copy_text(format!("AsisLog {}", env!("CARGO_PKG_VERSION")));
                            self.global_status = lang.tr("Versi disalin.").to_string();
                        }
                        if ui.button(lang.tr("Tutup")).clicked() {
                            self.about_open = false;
                        }
                    });
                });
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

        // Jendela pintasan (F1): editor konfigurabel, bukan daftar statis.
        // Baris dari tabel SHORTCUTS (label + binding efektif + reset);
        // footer tetap untuk aksi yang disengaja fixed (Esc, 1–9, navigasi).
        if self.shortcuts_open {
            // Rekam tombol: tangkap tombol non-modifier pertama yang turun.
            // Esc membatalkan perekaman (Esc sendiri tetap fixed global).
            if let Some(id) = self.scut_recording.clone() {
                let mut done: Option<Option<String>> = None; // None=batal
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::Escape) {
                        done = Some(None);
                    } else {
                        let mut named: Vec<egui::Key> =
                            i.keys_down.iter().filter(|k| crate::app::shortcuts::key_name(**k) != "?").copied().collect();
                        named.sort_by_key(|k| *k as u32);
                        if let Some(&k) = named.first() {
                            let mods = crate::app::shortcuts::live_mods(ctx);
                            done = Some(Some(crate::app::shortcuts::binding_string(k, mods)));
                        }
                    }
                });
                if let Some(maybe) = done {
                    match maybe {
                        None => self.scut_recording = None,
                        Some(s) => {
                            // Tolak duplikat agar tak ada aksi yang mati diam-diam.
                            let clash = crate::app::shortcuts::SHORTCUTS
                                .iter()
                                .find(|d| d.id != id && self.scut_label(d.id) == s)
                                .map(|d| d.id);
                            match clash {
                                Some(other) => {
                                    let oname = crate::app::shortcuts::SHORTCUTS
                                        .iter()
                                        .find(|d| d.id == other)
                                        .map(|d| lang.tr(d.label_id).to_string())
                                        .unwrap_or_default();
                                    self.global_status =
                                        lang.f2("Pintasan {} sudah dipakai oleh {}.", s, oname);
                                }
                                None => {
                                    self.scut_overrides.insert(id.clone(), s);
                                    self.rebuild_shortcuts();
                                    self.cfg_dirty = true;
                                    self.save_config();
                                }
                            }
                            self.scut_recording = None;
                        }
                    }
                }
            }
            egui::Window::new(lang.tr("Pintasan AsisLog (F1)"))
                .collapsible(false)
                .resizable(true)
                .default_width(460.0)
                .show(ctx, |ui| {
                    // P1-11: konflik binding = tombol merah (klogg parity).
                    let conflicts = crate::app::shortcuts::find_conflicts(&self.scut_overrides);
                    let conflicted: Vec<String> =
                        conflicts.iter().map(|(id, _)| id.clone()).collect();
                    if !conflicted.is_empty() {
                        ui.colored_label(
                            egui::Color32::RED,
                            lang.tr("Ada pintasan dobel — dua aksi akan terpicu bersamaan."),
                        );
                    }
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for def in crate::app::shortcuts::SHORTCUTS {
                            ui.horizontal(|ui| {
                                ui.label(lang.tr(def.label_id)).on_hover_text(def.id);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let overridden = self.scut_overrides.contains_key(def.id);
                                    if overridden
                                        && ui.small_button(lang.tr("Reset")).clicked()
                                    {
                                        self.scut_overrides.remove(def.id);
                                        self.rebuild_shortcuts();
                                        self.cfg_dirty = true;
                                        self.save_config();
                                    }
                                    let rec = self.scut_recording.as_deref() == Some(def.id);
                                    let is_bad = conflicted.iter().any(|i| i == def.id);
                                    let btn = if rec {
                                        lang.tr("Tekan tombol… (Esc batal)").to_string()
                                    } else {
                                        self.scut_label(def.id)
                                    };
                                    let rich = if is_bad {
                                        egui::RichText::new(btn).color(egui::Color32::RED)
                                    } else {
                                        egui::RichText::new(btn)
                                    };
                                    let b = ui.button(rich);
                                    let b = if is_bad {
                                        b.on_hover_text(lang.tr("DOBEL: dipakai aksi lain juga"))
                                    } else {
                                        b
                                    };
                                    if b.clicked() {
                                        self.scut_recording = Some(def.id.to_string());
                                    }
                                });
                            });
                        }
                        ui.separator();
                        for (keys, desc) in [
                            ("Esc", lang.tr("Tutup dialog teratas; lalu batalkan pencarian")),
                            ("1-9", lang.tr("Label warna dari query aktif")),
                            ("PgUp / PgDn, Panah", lang.tr("Gulir viewport (di luar kolom ketik)")),
                        ] {
                            ui.horizontal(|ui| {
                                ui.strong(keys);
                                ui.label(desc);
                            });
                        }
                        ui.label(lang.tr("Esc, 1–9, dan navigasi viewport tetap (tidak dapat diubah)."));
                    });
                    if ui.button(lang.tr("Tutup")).clicked() {
                        self.shortcuts_open = false;
                        self.scut_recording = None;
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

        // ---- P1-13: Options dialog terpusat ----
        if self.options_open {
            let mut do_close = false;
            let mut apply = false;
            let screen_h = ctx.screen_rect().height();
            let max_win_h = (screen_h * 0.85).max(300.0);
            egui::Window::new(lang.tr("Pengaturan"))
                .collapsible(false)
                .resizable(true)
                .default_width(480.0)
                .max_height(max_win_h)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - 44.0).max(120.0))
                        .show(ui, |ui| {
                            ui.heading(lang.tr("Pemantauan file (LIVE)"));
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Interval poll (ms):"));
                                let mut ms = self.follow_ms as i32;
                                ui.add(
                                    egui::DragValue::new(&mut ms)
                                        .range(50..=5000)
                                        .speed(10.0),
                                );
                                self.follow_ms = (ms as u64).clamp(50, 5000);
                            });
                            ui.weak(lang.tr("Lebih kecil = lebih responsif, lebih besar = hemat CPU. Default 250 ms."));
                            ui.weak(lang.tr("Watch native (OS event) + polling sebagai fallback."));
                            let watch_txt = if self.watch_native {
                                lang.tr("Watch native: aktif (event OS memicu poll segera).")
                            } else {
                                lang.tr("Watch native: mati (polling saja).")
                            };
                            ui.weak(watch_txt);
                            ui.separator();
                            ui.heading(lang.tr("Mesin pencari"));
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Thread (0=auto):"));
                                let mut th = self.search_threads as i32;
                                ui.add(egui::DragValue::new(&mut th).range(0..=32).speed(1.0));
                                self.search_threads = (th.max(0) as usize).min(32);
                            });
                            ui.weak(lang.tr("Ganti thread berlaku setelah restart bila pencarian pernah jalan."));
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Maks hasil (0=default):"));
                                let mut mh = self.max_hits as i32;
                                ui.add(egui::DragValue::new(&mut mh).range(0..=10_000_000).speed(1000.0));
                                self.max_hits = if mh <= 0 { 0 } else { (mh as usize).clamp(10_000, 10_000_000) };
                            });
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Chunk pindai MiB (0=default):"));
                                let mut ch = self.search_chunk_mb as i32;
                                ui.add(egui::DragValue::new(&mut ch).range(0..=16).speed(1.0));
                                self.search_chunk_mb = if ch <= 0 { 0 } else { (ch as usize).clamp(1, 16) };
                            });
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Cache pola (0=default):"));
                                let mut ce = self.cache_entries as i32;
                                ui.add(egui::DragValue::new(&mut ce).range(0..=64).speed(1.0));
                                self.cache_entries = if ce <= 0 { 0 } else { (ce as usize).clamp(2, 64) };
                            });
                            ui.weak(lang.tr("Default: 200 rb hasil, 4 MiB chunk, 8 pola."));
                            ui.weak(lang.tr("Tiap hasil ±24 B RAM; 10 jt ≈ 240 MB. Jutaan match: pakai Ekspor SEMUA (streaming)."));
                            ui.separator();
                            ui.heading(lang.tr("Pembaruan & crash"));
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("URL cek versi (kosong = mati):"));
                                ui.text_edit_singleline(&mut self.update_url);
                            });
                            ui.weak(lang.tr("File teks polos berisi versi terbaru, mis. 0.2.0."));
                            ui.checkbox(&mut self.update_auto, lang.tr("Cek otomatis saat start"));
                            ui.horizontal(|ui| {
                                if ui.button(lang.tr("Periksa sekarang")).clicked() {
                                    self.check_for_updates();
                                }
                                if !self.update_status.is_empty() {
                                    ui.label(lang.tr_status(&self.update_status.clone()));
                                }
                            });
                            ui.separator();
                            ui.heading(lang.tr("Locale eksternal (JSON):"));
                            ui.horizontal(|ui| {
                                ui.label(lang.f1("Locale eksternal: {} override.", crate::i18n::locale_override_count()));
                                if ui.button(lang.tr("Muat file…")).clicked() {
                                    if let Some(p) = rfd::FileDialog::new()
                                        .add_filter("JSON", &["json"])
                                        .pick_file()
                                    {
                                        match crate::i18n::load_locale_file(&p) {
                                            Ok(n) => {
                                                self.locale_file = Some(p.display().to_string());
                                                self.global_status =
                                                    lang.f1("Locale eksternal: {} override.", n);
                                            }
                                            Err(e) => {
                                                self.global_status =
                                                    lang.f1("Gagal muat locale: {}", lang.tr_status(&e));
                                            }
                                        }
                                    }
                                }
                                if ui.button(lang.tr("Reset")).clicked() {
                                    crate::i18n::clear_locale_overrides();
                                    self.locale_file = None;
                                }
                            });
                            ui.separator();
                            ui.heading(lang.tr("Tampilan"));
                            ui.checkbox(&mut self.word_wrap_default, lang.tr("Lipat baris (word wrap) default untuk tab baru"));
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Tema:"));
                                egui::ComboBox::from_id_salt("opt_tema")
                                    .selected_text(self.tema.nama_in(lang))
                                    .show_ui(ui, |ui| {
                                        for t in Tema::semua() {
                                            if ui.selectable_label(self.tema == *t, t.nama_in(lang)).clicked() {
                                                self.tema = *t;
                                                self.cfg_dirty = true;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Font Log:"));
                                egui::ComboBox::from_id_salt("opt_font")
                                    .selected_text(lang.font_name(&self.font_family))
                                    .show_ui(ui, |ui| {
                                        for f in ["Bawaan", "JetBrains Mono", "Consolas"] {
                                            if ui.selectable_label(self.font_family == f, lang.font_name(f)).clicked() {
                                                self.font_family = f.to_string();
                                                self.apply_fonts(ctx);
                                                self.cfg_dirty = true;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("Font UI:"));
                                egui::ComboBox::from_id_salt("opt_ui_font")
                                    .selected_text(if self.ui_font == "default" { lang.tr("Bawaan") } else { lang.tr("Sistem") })
                                    .show_ui(ui, |ui| {
                                        for (key, label) in [("system", lang.tr("Sistem")), ("default", lang.tr("Bawaan"))] {
                                            if ui.selectable_label(self.ui_font == key, label).clicked() {
                                                self.ui_font = key.to_string();
                                                self.apply_fonts(ctx);
                                                self.cfg_dirty = true;
                                            }
                                        }
                                    });
                            });
                            ui.separator();
                            ui.heading(lang.tr("Bahasa"));
                            ui.horizontal(|ui| {
                                for l in [crate::i18n::Lang::Id, crate::i18n::Lang::En] {
                                    if ui.selectable_label(self.lang == l, l.label()).clicked() {
                                        self.set_lang(l);
                                    }
                                }
                            });
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button(lang.tr("Simpan")).clicked() {
                            apply = true;
                            do_close = true;
                        }
                        if ui.button(lang.tr("Batal")).clicked() {
                            do_close = true;
                        }
                    });
                });
            if apply {
                // Terapkan follow_ms ke semua tab langsung.
                let ms = self.follow_ms;
                for t in self.tabs.iter_mut() {
                    t.follow_ms = ms;
                }
                let restart_note = self.apply_search_options();
                self.cfg_dirty = true;
                self.save_config();
                self.global_status = if restart_note == "THREAD_RESTART" {
                    lang.tr("Pengaturan disimpan. Thread baru berlaku setelah restart.").to_string()
                } else {
                    lang.tr("Pengaturan disimpan.").to_string()
                };
            }
            if do_close {
                self.options_open = false;
            }
        }
    }
}
