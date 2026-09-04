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
        // Panel penanda (kiri, bisa diubah lebarnya agar tak menekan viewport)
        if self.tabs[cur_idx].show_bookmarks {
            let dark = self.tema_state.1;
            egui::SidePanel::left("penanda")
                .resizable(true)
                .default_width(260.0)
                .min_width(180.0)
                .max_width(460.0)
                .show(ctx, |ui| {
                    ui.heading("Penanda");
                    ui.label("Ctrl+B tandai - F2 ubah label - Alt+Atas/Bawah pindah");
                    let tab = &mut self.tabs[cur_idx];
                    ui.add(
                        egui::TextEdit::singleline(&mut tab.mark_query)
                            .id_source("tandai-saring")
                            .hint_text("Saring penanda…")
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
                        ui.label("Belum ada. Klik nomor baris / Ctrl+B untuk menandai.");
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
                                    .on_hover_text(format!("Lompat ke baris {}", ln))
                                    .clicked()
                                {
                                    jump = Some(ln);
                                }
                                if ui.small_button("Ubah").on_hover_text("Ubah label (F2)").clicked()
                                {
                                    rename = Some((ln, label.clone(), color));
                                }
                                if ui.small_button("×").on_hover_text("Hapus").clicked() {
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
                                ui.label("memantau tiap 500 ms");
                                // Info baris baru yang segar (< 6 detik).
                                if let Some((note, at)) = &tab.follow_note {
                                    if at.elapsed() < Duration::from_secs(6) {
                                        ui.label(note.clone());
                                    }
                                }
                            } else if tab.doc.follow {
                                ui.label("LIVE dijeda - kembali ke akhir untuk melanjutkan");
                            } else if !tab.doc.index.complete {
                                // floor (bukan round): 100% hanya tepat saat selesai.
                                let pct = (tab.doc.index.progress * 100.0).floor() as u32;
                                ui.label(format!(
                                    "Mengindeks {}% - {} baris terdeteksi",
                                    pct.min(100),
                                    format_count(tab.doc.index.total_lines),
                                ));
                            } else {
                                ui.label("Siap");
                            }
                            ui.separator();
                            ui.label(format!("File: {}", format_size(tab.doc.size)));
                            ui.separator();
                            let lines = if tab.doc.index.complete {
                                format!("{} baris", format_count(tab.doc.index.total_lines))
                            } else {
                                format!("~{} baris", format_count(tab.doc.line_count_estimate()))
                            };
                            ui.label(lines);
                            ui.separator();
                            ui.label(tab.doc.encoding().label());
                            ui.separator();
                            ui.label(format!(
                                "Cari: {} hasil",
                                format_count(tab.doc.hits.len() as u64)
                            ));
                            ui.separator();
                            ui.label(if tab.doc.filter_active {
                                format!(
                                    "Filter: aktif ({})",
                                    format_count(tab.doc.filter_map.len() as u64)
                                )
                            } else {
                                String::from("Filter: mati")
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
                            ui.label(format!(
                                "Pos: baris {} ({}{:.0}%)",
                                format_count(
                                    tab.row_to_line(tab.top_row).unwrap_or(1)
                                ),
                                if tab.doc.index.complete { "" } else { "~" },
                                pos.clamp(0.0, 100.0),
                            ));
                            if !tab.doc.status.is_empty() {
                                ui.separator();
                                ui.label(tab.doc.status.clone());
                            }
                        });
                    });
            });

        // ---- hasil pencarian: header ramping, collapsible ----
        // Tertutup = tepat 28px (header saja) agar viewport log lega;
        // terbuka = resizable 30..460 (bawaan 180).
        let show_body = {
            let t = &self.tabs[cur_idx];
            !t.results_collapsed
                && (!t.search_text.trim().is_empty()
                    || !t.doc.hits.is_empty()
                    || t.doc.search_in_progress
                    || t.doc.search_error.is_some())
        };
        let mut panel = egui::TopBottomPanel::bottom("hasil").max_height(460.0);
        if show_body {
            panel = panel.resizable(true).default_height(180.0).min_height(30.0);
        } else {
            // Header saja; tinggi ikut zoom agar teks tak terpotong.
            panel = panel.exact_height((28.0 * self.zoom).round().clamp(24.0, 44.0));
        }
        panel.show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let n = self.tabs[cur_idx].doc.hits.len();
                    ui.strong(format!("Hasil ({})", format_count(n as u64)));
                    if let Some(c) = self.tabs[cur_idx].current_hit {
                        if n > 0 {
                            ui.label(format!("dipilih #{}/{}", c + 1, n));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("×")
                            .on_hover_text("Bersihkan pencarian dan hentikan worker")
                            .clicked()
                        {
                            self.tabs[cur_idx].clear_search();
                        }
                        let t = &mut self.tabs[cur_idx];
                        let (icon, tip) = if t.results_collapsed {
                            (Icon::ChevronDown, "Tampilkan panel hasil")
                        } else {
                            (Icon::ChevronUp, "Ciutkan panel hasil")
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
                    if ui.button("Tampilkan ±20 baris").clicked() {
                        let t = &mut self.tabs[cur_idx];
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
                        .button("Salin hasil")
                        .on_hover_text("Salin semua hasil (maks 16 MB) ke papan klip")
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
                            t.doc.status = String::from("Tidak ada hasil untuk disalin.");
                        } else {
                            ctx.copy_text(out);
                            t.doc.status = if over {
                                String::from(
                                    "Hasil disalin sebagian (16 MB). Gunakan Ekspor untuk sisanya.",
                                )
                            } else {
                                String::from("Hasil disalin ke papan klip.")
                            };
                        }
                    }
                    if ui.button("Ekspor hasil…").clicked() {
                        self.tabs[cur_idx].export_open = true;
                    }
                });
            let total = self.tabs[cur_idx].doc.hits.len();
            if total == 0 {
                ui.label("Belum ada hasil. Ketik kata kunci di kolom Cari.");
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
                            let Some(h) = self.tabs[cur_idx].doc.hits.get(i).cloned() else {
                                continue;
                            };
                            // Ambil teks baris via Doc (pinjam mut singkat per baris).
                            let text = self.tabs[cur_idx]
                                .doc
                                .get_line_text(h.line)
                                .unwrap_or_default();
                            let disp = self.tabs[cur_idx].display_cached(h.line, &text);
                            let sel = self.tabs[cur_idx].current_hit == Some(i);
                            let label = result_row(i, h.line, &disp);
                            if ui.selectable_label(sel, label).clicked() {
                                self.tabs[cur_idx].jump_to_hit(i);
                            }
                        }
                    });
            }
        });
    }
}
