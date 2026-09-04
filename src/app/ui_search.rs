// English comments: Search/filter toolbar panel (split from app.rs; behavior unchanged).
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
    pub(crate) fn render_search(&mut self, ctx: &egui::Context, cur_idx: usize) {
        // ---- Baris 2: pencarian (pusat UI, full-width) ----
        let mut open_range = false;
        let mut filter_all = false;
        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            // Pinjam terpisah agar closure menu tak konflik.
            let tab = &mut self.tabs[cur_idx];
            let presets = &mut self.presets;
            let history = &mut self.history;
            let cfg_dirty = &mut self.cfg_dirty;
            let preset_save_open = &mut self.preset_save_open;
            let preset_save_name = &mut self.preset_save_name;
            let mut search_focused = false;
            ui.horizontal(|ui| {
                // Menu preset pencarian (bawaan + simpanan user).
                ui.menu_button("Preset v", |ui| {                    ui.label("Bawaan:");
                    for p in crate::store::builtin_presets() {
                        if ui.button(p.name.clone()).clicked() {
                            tab.search_text = p.query.clone();
                            tab.regex_on = p.regex;
                            tab.case_sensitive = p.case_sensitive;
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if presets.is_empty() {
                        ui.label("Belum ada simpanan.");
                    }
                    let mut del: Option<usize> = None;
                    let mut apply: Option<Preset> = None;
                    for (i, p) in presets.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button(p.name.clone()).clicked() {
                                apply = Some(p.clone());
                                ui.close();
                            }
                            if ui.small_button("×").on_hover_text("Hapus preset").clicked() {
                                del = Some(i);
                            }
                        });
                    }
                    if let Some(i) = del {
                        presets.remove(i);
                        *cfg_dirty = true;
                    }
                    if let Some(p) = apply {
                        tab.search_text = p.query.clone();
                        tab.regex_on = p.regex;
                        tab.case_sensitive = p.case_sensitive;
                        tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                    }
                    ui.separator();
                    if ui.button("Simpan pencarian saat ini…").clicked() {
                        *preset_save_name = tab.search_text.clone();
                        *preset_save_open = true;
                        ui.close();
                    }
                });
                let clear_w = 30.0;
                let w = (ui.available_width() - clear_w - 8.0).max(120.0);
                let resp = ui.add_sized(
                    egui::vec2(w, 0.0),
                    egui::TextEdit::singleline(&mut tab.search_text)
                        .id_source("cari")
                        .hint_text("Cari teks, exception, request ID, atau regex…"),
                );
                search_focused = resp.has_focus();
                if resp.changed() {
                    tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                }
                if ui
                    .add_sized(egui::vec2(clear_w, 0.0), egui::Button::new("×"))
                    .on_hover_text("Bersihkan pencarian (Esc)")
                    .clicked()
                {
                    tab.search_text.clear();
                    tab.last_searched.clear();
                    tab.doc.hits.clear();
                    tab.doc.search_error = None;
                    tab.doc.search_in_progress = false;
                    tab.current_hit = None;
                    tab.results_collapsed = true;
                }
            });
            // Baris chip boolean: tampil bila query butuh logika AND/OR/NOT.
            // Hapus chip = buang token itu dari query, lalu cari ulang.
            if !tab.search_text.trim().is_empty()
                && !tab.regex_on
                && crate::engine::query::is_boolean_query(&tab.search_text)
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Logika:");
                    match crate::engine::query::top_spans(&tab.search_text) {
                        Some(spans) => {
                            let mut remove: Option<(usize, usize)> = None;
                            for (txt, a, b) in spans {
                                ui.label(format!("[{}]", txt));
                                if ui
                                    .small_button("×")
                                    .on_hover_text(format!("Hapus \"{}\" dari query", txt))
                                    .clicked()
                                {
                                    remove = Some((a, b));
                                }
                            }
                            if let Some((a, b)) = remove {
                                let q = tab.search_text.clone();
                                let rest = format!(
                                    "{} {}",
                                    q[..a].trim_end(),
                                    q[b..].trim_start()
                                );
                                tab.search_text = rest.trim().to_string();
                                tab.debounce_at =
                                    Some(Instant::now() + Duration::from_millis(150));
                            }
                        }
                        None => {
                            ui.label("Ekspresi kompleks (OR/kurung) dievaluasi penuh.");
                        }
                    }
                });
            }
            // Baris autocomplete history: tampil saat kolom fokus + ada yang cocok.
            if search_focused && !tab.search_text.trim().is_empty() {
                let sug = crate::store::suggest_history(history, tab.search_text.trim(), 6);
                if !sug.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Riwayat:");
                        let mut apply: Option<HistEntry> = None;
                        for h in sug {
                            let short: String = h.query.chars().take(40).collect();
                            let tag = if h.regex { " .*" } else { "" };
                            if ui.small_button(format!("{}{}", short, tag)).clicked() {
                                apply = Some(h.clone());
                            }
                        }
                        if let Some(h) = apply {
                            tab.search_text = h.query.clone();
                            tab.regex_on = h.regex;
                            tab.case_sensitive = h.case_sensitive;
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                        }
                    });
                }
            }
            ui.horizontal_wrapped(|ui| {
                // Toggle peka-huruf dan regex dengan state visual tegas.
                let cs = tab.case_sensitive;
                if ui
                    .add(egui::Button::selectable(cs, "Aa"))
                    .on_hover_text("Peka huruf besar/kecil (case sensitive)")
                    .clicked()
                {
                    tab.case_sensitive = !cs;
                }
                let rx = tab.regex_on;
                if ui
                    .add(egui::Button::selectable(rx, ".*"))
                    .on_hover_text("Perlakukan query sebagai regex")
                    .clicked()
                {
                    tab.regex_on = !rx;
                }
                ui.separator();
                if ui
                    .button("‹ Sebelumnya")
                    .on_hover_text("Hasil sebelumnya (Shift+F3)")
                    .clicked()
                    && !tab.doc.hits.is_empty()
                {
                    let n = tab.doc.hits.len();
                    let c = tab.current_hit.unwrap_or(0);
                    tab.jump_to_hit(c.saturating_sub(1).min(n - 1));
                }
                if ui
                    .button("Berikutnya ›")
                    .on_hover_text("Hasil berikutnya (F3)")
                    .clicked()
                    && !tab.doc.hits.is_empty()
                {
                    let n = tab.doc.hits.len();
                    let c = tab.current_hit.unwrap_or(0);
                    tab.jump_to_hit((c + 1).min(n - 1));
                }
                // Info hasil / progress / 0-hasil yang menjelaskan.
                if tab.doc.search_in_progress {
                    if tab.search_total > 0 {
                        ui.label(format!(
                            "Mencari… {} / {} · {} hasil",
                            format_size(tab.search_scanned),
                            format_size(tab.search_total),
                            format_count(tab.doc.hits.len() as u64),
                        ));
                    } else {
                        ui.label(format!(
                            "Mencari… {} hasil",
                            format_count(tab.doc.hits.len() as u64)
                        ));
                    }
                } else if let Some(e) = &tab.doc.search_error.clone() {
                    ui.colored_label(egui::Color32::RED, e);
                } else if !tab.search_text.trim().is_empty() && tab.doc.hits.is_empty() {
                    let q: String = tab.search_text.chars().take(60).collect();
                    ui.label(format!("Tidak ada kecocokan untuk \"{}\".", q));
                } else {
                    let mode = if tab.regex_on {
                        "regex"
                    } else if crate::engine::query::is_boolean_query(&tab.search_text) {
                        "boolean"
                    } else {
                        "literal"
                    };
                    ui.label(format!(
                        "{} hasil ({})",
                        format_count(tab.doc.hits.len() as u64),
                        mode
                    ));
                }
                if tab.doc.search_truncated {
                    ui.label("(dibatasi 200 rb)");
                }
                // Tombol filter dari pencarian: aktif hanya bila query valid.
                let can_filter = !tab.search_text.trim().is_empty();
                if ui
                    .add_enabled(can_filter, egui::Button::new("Jadikan filter"))
                    .on_hover_text("Tampilkan hanya baris yang cocok di viewport")
                    .clicked()
                {
                    tab.filter_text = tab.search_text.clone();
                    let q = tab.filter_text.clone();
                    tab.start_filter(q);
                }
                // Saat memindai: tombol batal (menaikkan generasi -> thread berhenti).
                if tab.doc.search_in_progress
                    && ui
                        .button("Batalkan pencarian")
                        .on_hover_text("Hentikan pindaian yang berjalan (Esc)")
                        .clicked()
                {
                    tab.doc.search_gen += 1;
                    tab.gen_shared
                        .store(tab.doc.search_gen, Ordering::Relaxed);
                    tab.doc.search_in_progress = false;
                    tab.doc.status = String::from("Pencarian dibatalkan.");
                }
                // Chip cakupan aktif (batasi pencarian ke rentang baris).
                if let Some((a, b)) = tab.scope {
                    ui.label(format!(
                        "Cakupan: {}-{}",
                        format_count(a),
                        format_count(b)
                    ));
                    if ui.small_button("×").on_hover_text("Hapus cakupan").clicked() {
                        tab.scope = None;
                        tab.scope_a.clear();
                        tab.scope_b.clear();
                        if !tab.search_text.trim().is_empty() {
                            tab.debounce_at =
                                Some(Instant::now() + Duration::from_millis(150));
                        }
                    }
                }
            });
            // ---- Filter + mode tampil + cakupan ----
            ui.horizontal_wrapped(|ui| {
                ui.label("Filter");
                let w = (ui.available_width() - 420.0).clamp(120.0, 420.0);
                ui.add(
                    egui::TextEdit::singleline(&mut tab.filter_text)
                        .id_source("saring")
                        .hint_text("ERROR -DEBUG")
                        .desired_width(w),
                );
                if ui.button("Terapkan").clicked() {
                    let q = tab.filter_text.clone();
                    let cs = tab.case_sensitive;
                    tab.doc.filter = parse_filter(&q, cs);
                    tab.start_filter(q);
                }
                if ui.button("Bersihkan").clicked() {
                    tab.filter_text.clear();
                    tab.start_filter(String::new());
                }
                if ui
                    .button("Ke semua tab")
                    .on_hover_text("Terapkan filter ini ke semua tab (korelasi)")
                    .clicked()
                {
                    filter_all = true;
                }
                egui::ComboBox::from_id_salt("viewmode")
                    .selected_text(format!("Tampil: {}", tab.view_mode.nama()))
                    .show_ui(ui, |ui| {
                        for m in ViewMode::semua() {
                            if ui
                                .selectable_label(tab.view_mode == *m, m.nama())
                                .on_hover_text(match m {
                                    ViewMode::All => "Semua baris (atau hasil filter)",
                                    ViewMode::Hits => "Hanya baris hasil pencarian",
                                    ViewMode::Marks => "Hanya baris penanda",
                                })
                                .clicked()
                            {
                                tab.view_mode = *m;
                                tab.refresh_mode_map();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Mode tampil viewport");
                if ui
                    .button("Cakupan…")
                    .on_hover_text("Batasi pencarian ke rentang baris (hemat untuk file besar)")
                    .clicked()
                {
                    tab.scope_open = true;
                }
                if ui
                    .small_button("?")
                    .on_hover_text(
                        "Filter menyembunyikan baris yang tidak cocok.\n\
                         Token dipisah spasi; semua token inclusions harus ada (AND).\n\
                         Awalan - berarti kecualikan.\n\
                         key=value cocokkan field baris JSON (mis. level=ERROR).\n\n\
                         Contoh:\n  ERROR            hanya baris error\n  ERROR -DEBUG     error tanpa debug\n  level=ERROR      field JSON level\n  OrderService     teks spesifik",
                    )
                    .clicked()
                {
                    tab.doc.status = String::from(
                        "Filter: pisahkan token dengan spasi, awalan - mengecualikan. Contoh: ERROR -DEBUG",
                    );
                }
                if ui
                    .button("Rentang waktu…")
                    .on_hover_text("Tampilkan hanya baris dalam rentang cap waktu")
                    .clicked()
                {
                    open_range = true;
                }
            });
            // Chip tegas saat filter aktif.
            if tab.doc.filter_active {
                ui.horizontal_wrapped(|ui| {
                    let total = if tab.doc.index.complete {
                        tab.doc.index.total_lines
                    } else {
                        tab.doc.line_count_estimate()
                    };
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 200, 255),
                        format!(
                            "Filter aktif: {} · {}/{} baris",
                            tab.doc.filter.raw,
                            format_count(tab.doc.filter_map.len() as u64),
                            format_count(total),
                        ),
                    );
                    if ui.small_button("Hapus").clicked() {
                        tab.filter_text.clear();
                        tab.start_filter(String::new());
                    }
                });
            }
        });

        if open_range {
            self.range_open = true;
        }
        // Korelasi: filter tab aktif ke semua tab.
        if filter_all && !self.tabs.is_empty() {
            let q = self.tabs[cur_idx].filter_text.clone();
            for t in self.tabs.iter_mut() {
                t.filter_text = q.clone();
                let qq = q.clone();
                t.start_filter(qq);
            }
            self.session_dirty = true;
        }

        // Flush config global yang ditandai kotor (di luar pinjam tab).
        if self.cfg_dirty {
            self.save_config();
        }
    }
}
