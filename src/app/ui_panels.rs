// English comments: Bookmarks side panel + status bar + results pane (split from app.rs; behavior unchanged).
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
    pub(crate) fn render_panels(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        // Panel penanda (kiri, bisa diubah lebarnya agar tak menekan viewport)
        if self.tabs[cur_idx].show_bookmarks {
            let dark = self.tema_state.1;
            egui::SidePanel::left("penanda")
                .resizable(true)
                .default_width(260.0)
                .min_width(180.0)
                .max_width(460.0)
                .show(ctx, |ui| {
                    ui.heading(lang.tr("Penanda"));
                    ui.label(lang.tr("Ctrl+B tandai - F2 ubah label - Alt+Atas/Bawah pindah"));
                    let tab = &mut self.tabs[cur_idx];
                    ui.add(
                        egui::TextEdit::singleline(&mut tab.mark_query)
                            .id_source("tandai-saring")
                            .hint_text(lang.tr("Saring penanda…"))
                            .desired_width(f32::INFINITY),
                    );
                    let q = tab.mark_query.to_lowercase();
                    // Kumpulkan agar pinjam berakhir sebelum aksi.
                    let rows: Vec<(u64, String, BookmarkColor)> = tab
                        .doc
                        .bookmarks
                        .iter()
                        .filter(|b| {
                            q.is_empty()
                                || b.label.to_lowercase().contains(&q)
                                || b.line.to_string().contains(&q)
                        })
                        .map(|b| (b.line, b.label.clone(), b.color))
                        .collect();
                    if rows.is_empty() {
                        ui.label(lang.tr("Belum ada. Klik nomor baris / Ctrl+B untuk menandai."));
                    }
                    let mut jump: Option<u64> = None;
                    let mut rename: Option<(u64, String, BookmarkColor)> = None;
                    let mut delete: Option<u64> = None;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (ln, label, color) in rows {
                            ui.horizontal(|ui| {
                                let (dot_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(
                                    dot_rect.center(),
                                    4.0,
                                    mark_color(color, dark),
                                );
                                if ui
                                    .button(format!("{} · {}", format_count(ln), label))
                                    .on_hover_text(lang.f1("Lompat ke baris {}", ln))
                                    .clicked()
                                {
                                    jump = Some(ln);
                                }
                                if ui.small_button(lang.tr("Ubah")).on_hover_text(lang.tr("Ubah label (F2)")).clicked()
                                {
                                    rename = Some((ln, label.clone(), color));
                                }
                                if ui.small_button("×").on_hover_text(lang.tr("Hapus")).clicked() {
                                    delete = Some(ln);
                                }
                            });
                        }
                    });
                    if let Some(ln) = jump {
                        self.tabs[cur_idx].nav_to(ln);
                    }
                    if let Some((ln, label, color)) = rename {
                        self.rename_line = ln;
                        self.rename_label = label;
                        self.rename_color = color;
                        self.rename_open = true;
                    }
                    if let Some(ln) = delete {
                        // Hapus via modal konfirmasi (tak langsung).
                        self.confirm = Some(ConfirmAction::DeleteMark(ln));
                    }
                });
        }

        // ---- status bawah: grup ringkas, gulir mendatar bila sempit ----
        // Tinggi mengikuti zoom agar proporsional dengan teks.
        egui::TopBottomPanel::bottom("status")
            .exact_height((26.0 * self.zoom).round().clamp(22.0, 40.0))
            .show(ctx, |ui| {
                let tab = &self.tabs[cur_idx];
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Keadaan utama (teks berwarna, tanpa glyph simbol).
                            if tab.doc.follow && tab.doc.stick_bottom {
                                ui.colored_label(
                                    egui::Color32::from_rgb(90, 220, 120),
                                    "LIVE",
                                );
                                ui.label(lang.tr("memantau tiap 500 ms"));
                                // Info baris baru yang segar (< 6 detik).
                                if let Some((note, at)) = &tab.follow_note {
                                    if at.elapsed() < Duration::from_secs(6) {
                                        ui.label(lang.tr_status(note));
                                    }
                                }
                            } else if tab.doc.follow {
                                ui.label(lang.tr("LIVE dijeda - kembali ke akhir untuk melanjutkan"));
                            } else if !tab.doc.index.complete {
                                // floor (bukan round): 100% hanya tepat saat selesai.
                                let pct = (tab.doc.index.progress * 100.0).floor() as u32;
                                ui.label(lang.f2("Mengindeks {}% - {} baris terdeteksi", pct.min(100), format_count(tab.doc.index.total_lines)));
                            } else {
                                ui.label(lang.tr("Siap"));
                            }
                            ui.separator();
                            ui.label(format!("{}: {}", lang.tr("File"), format_size(tab.doc.size)));
                            ui.separator();
                            let lines = if tab.doc.index.complete {
                                lang.f1("{} baris", format_count(tab.doc.index.total_lines))
                            } else {
                                lang.f1("~{} baris", format_count(tab.doc.line_count_estimate()))
                            };
                            ui.label(lines);
                            ui.separator();
                            ui.label(tab.doc.encoding().label());
                            ui.separator();
                            ui.label(lang.f1("Cari: {} hasil", format_count(tab.doc.hits.len() as u64)));
                            ui.separator();
                            ui.label(if tab.doc.filter_active {
                                lang.f1("Filter: aktif ({})", format_count(tab.doc.filter_map.len()))
                            } else {
                                lang.tr("Filter: mati").to_string()
                            });
                            ui.separator();
                            let total_rows = tab.total_view_rows();
                            // Jujur: 100% tepat saat viewport mencapai akhir;
                            // "~" saat total masih estimasi (indeks berjalan).
                            let at_end = total_rows > 0
                                && tab.top_row + tab.last_visible.max(1) >= total_rows;
                            let pos = if at_end {
                                100.0
                            } else if total_rows > 0 {
                                tab.top_row as f64 / total_rows as f64 * 100.0
                            } else {
                                0.0
                            };
                            ui.label(lang.f3("Pos: baris {} ({}{}%)", format_count(
                                    tab.row_to_line(tab.top_row).unwrap_or(1)
                                ), if tab.doc.index.complete { "" } else { "~" }, format!("{:.0}", pos.clamp(0.0, 100.0))));
                            if !tab.doc.status.is_empty() {
                                ui.separator();
                                ui.label(lang.tr_status(&tab.doc.status.clone()));
                            }
                        });
                    });
            });

        // ---- hasil pencarian: header ramping, collapsible, dual-pane ----
        // Tertutup = tepat 28px (header saja) agar viewport log lega;
        // terbuka = resizable (bawaan 180, 320 dalam mode Bagi).
        // Mode Bagi (dual-pane, paritas klogg): panel selalu terbuka lebar
        // sehingga log + hasil terlihat bersamaan seperti dua jendela klogg.
        let split = self.split_view;
        let show_body = {
            let t = &self.tabs[cur_idx];
            split
                || !t.results_collapsed
                    && (!t.search_text.trim().is_empty()
                        || !t.doc.hits.is_empty()
                        || t.doc.search_in_progress
                        || t.doc.search_error.is_some())
        };
        let mut panel = egui::TopBottomPanel::bottom("hasil")
            .max_height(if split { 600.0 } else { 460.0 });
        if show_body {
            panel = panel
                .resizable(true)
                .default_height(if split { 320.0 } else { 180.0 })
                .min_height(if split { 120.0 } else { 30.0 });
        } else {
            // Header saja; tinggi ikut zoom agar teks tak terpotong.
            panel = panel.exact_height((28.0 * self.zoom).round().clamp(24.0, 44.0));
        }
        panel.show(ctx, |ui| {
                // Snapshot yang sedang dilihat (None = live). Disalin keluar
                // dulu agar pinjam tab di bawah tidak konflik.
                let kept_view: Option<usize> = self.tabs[cur_idx].kept_view;
                let kept_names: Vec<String> =
                    self.tabs[cur_idx].kept.iter().map(|k| k.name.clone()).collect();
                let viewing_name: Option<String> =
                    kept_view.and_then(|i| kept_names.get(i).cloned());
                ui.horizontal(|ui| {
                    let n = match kept_view {
                        Some(i) => self.tabs[cur_idx].kept.get(i).map(|k| k.hits.len()).unwrap_or(0),
                        None => self.tabs[cur_idx].doc.hits.len(),
                    };
                    match &viewing_name {
                        Some(name) => ui.strong(format!("{} ({})", name, format_count(n as u64))),
                        None => ui.strong(lang.f1("Hasil ({})", format_count(n as u64))),
                    };
                    if kept_view.is_none() {
                        if let Some(c) = self.tabs[cur_idx].current_hit {
                            if n > 0 {
                                ui.label(lang.f2("dipilih #{}/{}", c + 1, n));
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("×")
                            .on_hover_text(lang.tr("Bersihkan pencarian dan hentikan worker"))
                            .clicked()
                        {
                            self.tabs[cur_idx].clear_search();
                        }
                        let t = &mut self.tabs[cur_idx];
                        let (icon, tip) = if t.results_collapsed && !split {
                            (Icon::ChevronDown, lang.tr("Tampilkan panel hasil"))
                        } else {
                            (Icon::ChevronUp, lang.tr("Ciutkan panel hasil"))
                        };
                        if icon_button(ui, icon, tip).clicked() {
                            t.results_collapsed = !t.results_collapsed;
                        }
                    });
                });
                // Isi hanya bila dibuka dan relevan (show_body dihitung di atas).
                if !show_body {
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    // konteks hasil terpilih
                    if ui.button(lang.tr("Tampilkan ±20 baris")).clicked() {                        let t = &mut self.tabs[cur_idx];
                        if let Some(c) = t.current_hit {
                            if let Some(h) = t.doc.hits.get(c) {
                                let ln = h.line;
                                t.selected_line = ln;
                                // top = ln-20 dalam koordinat view
                                let row = t.view_row_of_line(ln).unwrap_or(0);
                                t.top_row = row.saturating_sub(20);
                                t.record_nav(ln);
                            }
                        }
                    }
                    if ui
                        .button(lang.tr("Salin hasil"))
                        .on_hover_text(lang.tr("Salin semua hasil (maks 16 MB) ke papan klip"))
                        .clicked()
                    {
                        let t = &mut self.tabs[cur_idx];
                        let mut out = String::new();
                        let mut over = false;
                        // Salin per indeks (satu Hit 24 B per iterasi),
                        // bukan clone seluruh vec hasil.
                        for i in 0..t.doc.hits.len() {
                            let h = t.doc.hits[i].clone();
                            let txt = t.doc.get_line_text(h.line).unwrap_or_default();
                            let row = format!("{}: {}\n", h.line, txt);
                            if out.len() + row.len() > crate::engine::COPY_CAP_BYTES {
                                over = true;
                                break;
                            }
                            out.push_str(&row);
                        }
                        if out.is_empty() {
                            t.doc.status = lang.tr("Tidak ada hasil untuk disalin.").to_string();
                        } else {
                            ctx.copy_text(out);
                            t.doc.status = if over {
                                lang.tr(
                                    "Hasil disalin sebagian (16 MB). Gunakan Ekspor untuk sisanya.",
                                ).to_string()
                            } else {
                                lang.tr("Hasil disalin ke papan klip.").to_string()
                            };
                        }
                    }
                    if ui.button(lang.tr("Ekspor hasil…")).clicked() {
                        self.tabs[cur_idx].export_open = true;
                    }
                });
                // Keep results (paritas klogg): bekukan hasil live menjadi
                // snapshot bernama; query boleh pindah tanpa kehilangan.
                // Session-only, maks 5 per tab (lihat MAX_KEPT).
                ui.horizontal_wrapped(|ui| {
                    let can_keep = !self.tabs[cur_idx].doc.hits.is_empty()
                        && self.tabs[cur_idx].kept.len() < crate::app::tab::MAX_KEPT
                        && kept_view.is_none();
                    if ui
                        .add_enabled(can_keep, egui::Button::new(lang.tr("Simpan hasil")))
                        .on_hover_text(lang.tr("Bekukan hasil ini sebagai snapshot"))
                        .clicked()
                    {
                        let t = &mut self.tabs[cur_idx];
                        let auto = format!(
                            "{} · {}",
                            t.search_text.chars().take(30).collect::<String>(),
                            t.doc.hits.len()
                        );
                        if t.keep_results(auto).is_err() {
                            t.doc.status = lang.tr("Tidak ada hasil untuk disimpan.").to_string();
                        }
                    }
                    if !kept_names.is_empty() || kept_view.is_some() {
                        let cur_label = viewing_name
                            .clone()
                            .unwrap_or_else(|| lang.tr("Live").to_string());
                        egui::ComboBox::from_id_salt("kept_view")
                            .selected_text(cur_label)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(kept_view.is_none(), lang.tr("Live"))
                                    .on_hover_text(lang.tr("Kembali ke hasil live"))
                                    .clicked()
                                {
                                    self.tabs[cur_idx].kept_view = None;
                                }
                                for (i, name) in kept_names.iter().enumerate() {
                                    if ui.selectable_label(kept_view == Some(i), name).clicked() {
                                        self.tabs[cur_idx].kept_view = Some(i);
                                        self.tabs[cur_idx].results_collapsed = false;
                                    }
                                }
                            });
                        if kept_view.is_some()
                            && ui
                                .small_button("×")
                                .on_hover_text(lang.tr("Hapus snapshot ini"))
                                .clicked()
                        {
                            self.tabs[cur_idx].drop_kept(kept_view.unwrap_or(0));
                        }
                    }
                });
            let total: usize = match kept_view {
                Some(i) => self.tabs[cur_idx].kept.get(i).map(|k| k.hits.len()).unwrap_or(0),
                None => self.tabs[cur_idx].doc.hits.len(),
            };
            let live = kept_view.is_none();
            if total == 0 {
                ui.label(lang.tr("Belum ada hasil. Ketik kata kunci di kolom Cari."));
            } else {
                let row_h = self.row_h();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show_rows(ui, row_h, total, |ui, range| {
                        // Satu baris hasil = satu baris visual (potong, jangan wrap).
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        // Decode preview per row (cached in Doc); Hit disalin
                        // per baris terlihat saja (24 B), bukan seluruh vec.
                        for i in range {
                            // Snapshot dibaca per indeks (tanpa clone seluruh vec).
                            let h = if live {
                                match self.tabs[cur_idx].doc.hits.get(i).cloned() {
                                    Some(h) => h,
                                    None => continue,
                                }
                            } else {
                                let kv = self.tabs[cur_idx].kept_view.unwrap_or(usize::MAX);
                                match self.tabs[cur_idx]
                                    .kept
                                    .get(kv)
                                    .and_then(|k| k.hits.get(i))
                                    .cloned()
                                {
                                    Some(h) => h,
                                    None => continue,
                                }
                            };
                            // Kepala 16 KiB (baris raksasa tak dirender penuh).
                            let (text, total_b, trunc) = self.tabs[cur_idx]
                                .doc
                                .get_line_head(h.line, crate::engine::HEAD_BYTES)
                                .unwrap_or_default();
                            let mut text = text;
                            if trunc {
                                text.push_str(&format!(
                                    " …(+{} B)",
                                    crate::engine::format_count(
                                        total_b.saturating_sub(text.len() as u64)
                                    )
                                ));
                            }
                            let disp = self.tabs[cur_idx].display_cached(h.line, &text);
                            let sel = live && self.tabs[cur_idx].current_hit == Some(i);
                            let label = result_row(i, h.line, &disp);
                            if ui.selectable_label(sel, label).clicked() {
                                if live {
                                    self.tabs[cur_idx].jump_to_hit(i);
                                } else {
                                    // Snapshot beku: lompat langsung, tanpa
                                    // menggeser current_hit milik live.
                                    self.tabs[cur_idx].nav_to(h.line);
                                }
                            }
                        }
                    });
            }
        });
    }
}
