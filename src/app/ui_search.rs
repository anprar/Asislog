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
        let lang = self.lang;
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
            ui.horizontal_wrapped(|ui| {
                // Menu preset pencarian (bawaan + simpanan user).
                ui.menu_button(lang.tr("Preset v"), |ui| {
                    ui.label(lang.tr("Bawaan:"));
                    for p in crate::store::builtin_presets_for(lang) {
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
                        ui.label(lang.tr("Belum ada simpanan."));
                    }
                    let mut del: Option<usize> = None;
                    let mut apply: Option<Preset> = None;
                    for (i, p) in presets.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button(p.name.clone()).clicked() {
                                apply = Some(p.clone());
                                ui.close();
                            }
                            if ui.small_button("×").on_hover_text(lang.tr("Hapus preset")).clicked() {
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
                    if ui.button(lang.tr("Simpan pencarian saat ini…")).clicked() {
                        *preset_save_name = tab.search_text.clone();
                        *preset_save_open = true;
                        ui.close();
                    }
                });

                let w = (ui.available_width() * 0.32).clamp(160.0, 360.0);
                let resp = ui.add_sized(
                    egui::vec2(w, 0.0),
                    egui::TextEdit::singleline(&mut tab.search_text)
                        .id_source("cari")
                        .hint_text(lang.tr("Cari teks, exception, request ID, atau regex…")),
                );
                search_focused = resp.has_focus();
                if resp.changed() {
                    tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                }

                // Toggle peka-huruf dan regex tepat di samping input
                let cs = tab.case_sensitive;
                if ui
                    .add(egui::Button::selectable(cs, "Aa"))
                    .on_hover_text(lang.tr("Peka huruf besar/kecil (Alt+C)"))
                    .clicked()
                {
                    tab.case_sensitive = !cs;
                    if !tab.search_text.trim().is_empty() {
                        tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                    }
                }
                let rx = tab.regex_on;
                if ui
                    .add(egui::Button::selectable(rx, ".*"))
                    .on_hover_text(lang.tr("Perlakukan query sebagai regex (Alt+R)"))
                    .clicked()
                {
                    tab.regex_on = !rx;
                    if !tab.search_text.trim().is_empty() {
                        tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                    }
                }
                if ui
                    .small_button("×")
                    .on_hover_text(lang.tr("Bersihkan pencarian (Esc)"))
                    .clicked()
                {
                    tab.clear_search();
                }

                ui.separator();

                // Navigasi hasil ringkas
                if ui
                    .button("‹")
                    .on_hover_text(lang.tr("Hasil sebelumnya (Shift+F3)"))
                    .clicked()
                    && !tab.doc.hits.is_empty()
                {
                    let n = tab.doc.hits.len();
                    let c = tab.current_hit.unwrap_or(0);
                    tab.jump_to_hit(c.saturating_sub(1).min(n - 1));
                }
                if ui
                    .button("›")
                    .on_hover_text(lang.tr("Hasil berikutnya (F3)"))
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
                        ui.label(lang.f3("Mencari… {} / {} · {} hasil", format_size(tab.search_scanned), format_size(tab.search_total), format_count(tab.doc.hits.len() as u64)));
                    } else {
                        ui.label(lang.f1("Mencari… {} hasil", format_count(tab.doc.hits.len() as u64)));
                    }
                } else if let Some(e) = &tab.doc.search_error.clone() {
                    ui.colored_label(egui::Color32::RED, lang.tr_status(e));
                } else if !tab.search_text.trim().is_empty() && tab.doc.hits.is_empty() {
                    let q: String = tab.search_text.chars().take(40).collect();
                    ui.label(lang.f1("Tidak ada kecocokan \"{}\"", q));
                } else {
                    let mode = if tab.regex_on {
                        if tab.regex_complex {
                            "regex-complex"
                        } else {
                            "regex"
                        }
                    } else if crate::engine::query::is_boolean_query(&tab.search_text) {
                        "boolean"
                    } else {
                        "literal"
                    };
                    ui.label(lang.f2("{} hasil ({})", format_count(tab.doc.hits.len() as u64), mode));
                }
                if tab.doc.search_truncated {
                    ui.label(lang.tr("(dibatasi 200 rb)"));
                }

                ui.separator();

                // Sakelar model Sorot vs Saring (C-B2)
                let is_saring = tab.view_mode == ViewMode::Hits;
                if ui
                    .selectable_label(!is_saring, lang.tr("Sorot"))
                    .on_hover_text(lang.tr("Tampilkan semua baris, sorot kecocokan (F3 untuk lompat)"))
                    .clicked()
                    && is_saring
                {
                    tab.view_mode = ViewMode::All;
                    tab.refresh_mode_map();
                }
                if ui
                    .selectable_label(is_saring, lang.tr("Saring"))
                    .on_hover_text(lang.tr("Tampilkan HANYA baris yang cocok dengan pencarian"))
                    .clicked()
                    && !is_saring
                {
                    tab.view_mode = ViewMode::Hits;
                    tab.refresh_mode_map();
                }

                // Tombol konversi AST -> filter dengan peringatan eksplisit (C-B2)
                let can_filter = !tab.search_text.trim().is_empty();
                if ui
                    .add_enabled(can_filter, egui::Button::new(lang.tr("Jadikan filter")))
                    .on_hover_text(lang.tr("Konversi query pencarian ke filter permanen"))
                    .clicked()
                {
                    if tab.regex_on {
                        tab.doc.status = lang.tr("Pencarian regex tidak dapat dijadikan filter token.").to_string();
                    } else {
                        match crate::engine::query::parse_query(&tab.search_text) {
                            Ok(ast) => match crate::engine::query::to_filter_string(&ast) {
                                Ok(ft) => {
                                    tab.filter_text = ft.clone();
                                    let cs = tab.case_sensitive;
                                    tab.doc.filter = parse_filter(&ft, cs);
                                    tab.start_filter(ft);
                                    tab.doc.status = lang.f1("Filter diterapkan: {}", tab.filter_text.clone());
                                }
                                Err(reason) => {
                                    tab.doc.status = lang.f1("Gagal konversi ke filter: {}", lang.tr_status(&reason));
                                }
                            },
                            Err(e) => {
                                tab.doc.status = lang.f1("Query pencarian tidak valid: {}", lang.tr_status(&e));
                            }
                        }
                    }
                }

                // Saat memindai: tombol batal
                if tab.doc.search_in_progress
                    && ui
                        .button(lang.tr("Batalkan"))
                        .on_hover_text(lang.tr("Hentikan pindaian yang berjalan (Esc)"))
                        .clicked()
                {
                    tab.doc.search_gen += 1;
                    tab.gen_shared
                        .store(tab.doc.search_gen, Ordering::Relaxed);
                    tab.doc.search_in_progress = false;
                    tab.doc.status = lang.tr("Pencarian dibatalkan.").to_string();
                }

                // Chip cakupan aktif
                if let Some((a, b)) = tab.scope {
                    ui.label(lang.f2("Cakupan: {}-{}", format_count(a), format_count(b)));
                    if ui.small_button("×").on_hover_text(lang.tr("Hapus cakupan")).clicked() {
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
            // Baris chip boolean: tampil bila query butuh logika AND/OR/NOT.
            // Hapus chip = buang token itu dari query, lalu cari ulang.
            if !tab.search_text.trim().is_empty()
                && !tab.regex_on
                && crate::engine::query::is_boolean_query(&tab.search_text)
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label(lang.tr("Logika:"));
                    match crate::engine::query::top_spans(&tab.search_text) {
                        Some(spans) => {
                            let mut remove: Option<(usize, usize)> = None;
                            for (txt, a, b) in spans {
                                ui.label(format!("[{}]", txt));
                                if ui
                                    .small_button("×")
                                    .on_hover_text(lang.f1("Hapus \"{}\" dari query", txt))
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
                            ui.label(lang.tr("Ekspresi kompleks (OR/kurung) dievaluasi penuh."));
                        }
                    }
                });
            }
            // Baris autocomplete history: tampil saat kolom fokus + ada yang cocok.
            if search_focused && !tab.search_text.trim().is_empty() {
                let sug = crate::store::suggest_history(history, tab.search_text.trim(), 6);
                if !sug.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(lang.tr("Riwayat:"));
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
            // ---- Filter + mode tampil + cakupan ----
            ui.horizontal_wrapped(|ui| {
                ui.label(lang.tr("Filter"));
                let w = (ui.available_width() - 420.0).clamp(120.0, 420.0);
                ui.add(
                    egui::TextEdit::singleline(&mut tab.filter_text)
                        .id_source("saring")
                        .hint_text("ERROR -DEBUG")
                        .desired_width(w),
                );
                if ui.button(lang.tr("Terapkan")).clicked() {
                    let q = tab.filter_text.clone();
                    let cs = tab.case_sensitive;
                    tab.doc.filter = parse_filter(&q, cs);
                    tab.start_filter(q);
                }
                if ui.button(lang.tr("Bersihkan")).clicked() {
                    tab.filter_text.clear();
                    tab.start_filter(String::new());
                }
                if ui
                    .button(lang.tr("Ke semua tab"))
                    .on_hover_text(lang.tr("Terapkan filter ini ke semua tab (korelasi)"))
                    .clicked()
                {
                    filter_all = true;
                }
                egui::ComboBox::from_id_salt("viewmode")
                    .selected_text(format!("{}: {}", lang.tr("Tampil"), tab.view_mode.nama_in(lang)))
                    .show_ui(ui, |ui| {
                        for m in ViewMode::semua() {
                            if ui
                                .selectable_label(tab.view_mode == *m, m.nama_in(lang))
                                .on_hover_text(match m {
                                    ViewMode::All => lang.tr("Semua baris (atau hasil filter)"),
                                    ViewMode::Hits => lang.tr("Hanya baris hasil pencarian"),
                                    ViewMode::Marks => lang.tr("Hanya baris penanda"),
                                })
                                .clicked()
                            {
                                tab.view_mode = *m;
                                tab.refresh_mode_map();
                            }
                        }
                    })
                    .response
                    .on_hover_text(lang.tr("Mode tampil viewport"));
                if ui
                    .button(lang.tr("Cakupan…"))
                    .on_hover_text(lang.tr("Batasi pencarian ke rentang baris (hemat untuk file besar)"))
                    .clicked()
                {
                    tab.scope_open = true;
                }
                if ui
                    .small_button("?")
                    .on_hover_text(
                        lang.tr("Filter menyembunyikan baris yang tidak cocok.\n\
                         Token dipisah spasi; semua token inclusions harus ada (AND).\n\
                         Awalan - berarti kecualikan.\n\
                         key=value cocokkan field baris JSON (mis. level=ERROR).\n\n\
                         Contoh:\n  ERROR            hanya baris error\n  ERROR -DEBUG     error tanpa debug\n  level=ERROR      field JSON level\n  OrderService     teks spesifik"),
                    )
                    .clicked()
                {
                    tab.doc.status = lang.tr(
                        "Filter: pisahkan token dengan spasi, awalan - mengecualikan. Contoh: ERROR -DEBUG",
                    ).to_string();
                }
                if ui
                    .button(lang.tr("Rentang waktu…"))
                    .on_hover_text(lang.tr("Tampilkan hanya baris dalam rentang cap waktu"))
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
                        lang.f3("Filter aktif: {} · {}/{} baris", tab.doc.filter.raw.clone(), format_count(tab.doc.filter_map.len()), format_count(total)),
                    );
                    if ui.small_button(lang.tr("Hapus")).clicked() {
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

    /// Floating search HUD saat di Zen mode (C-B1).
    pub(crate) fn render_zen_search(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        let tab = &mut self.tabs[cur_idx];
        let mut close = false;
        egui::Window::new(lang.tr("Pencarian (Zen)"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, [-20.0, 36.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let re = ui.add(
                        egui::TextEdit::singleline(&mut tab.search_text)
                            .id_source("cari")
                            .hint_text(lang.tr("Cari ..."))
                            .desired_width(220.0),
                    );
                    if re.changed() {
                        tab.debounce_at = Some(Instant::now() + Duration::from_millis(150));
                    }
                    if ui
                        .selectable_label(tab.case_sensitive, "Aa")
                        .on_hover_text(lang.tr("Huruf besar/kecil"))
                        .clicked()
                    {
                        tab.case_sensitive = !tab.case_sensitive;
                        tab.start_search(&mut self.history);
                    }
                    if ui
                        .selectable_label(tab.regex_on, ".*")
                        .on_hover_text(lang.tr("Regex"))
                        .clicked()
                    {
                        tab.regex_on = !tab.regex_on;
                        tab.start_search(&mut self.history);
                    }
                    if ui.button(lang.tr("Prev")).clicked() && !tab.doc.hits.is_empty() {
                        let cur = tab.current_hit.unwrap_or(0);
                        let nxt = cur.saturating_sub(1);
                        tab.jump_to_hit(nxt);
                    }
                    if ui.button(lang.tr("Next")).clicked() && !tab.doc.hits.is_empty() {
                        let n = tab.doc.hits.len();
                        let cur = tab.current_hit.unwrap_or(0);
                        let nxt = (cur + 1).min(n - 1);
                        tab.jump_to_hit(nxt);
                    }
                    let count_text = if tab.doc.search_in_progress {
                        "…".to_string()
                    } else {
                        lang.f1("{} hasil", format_count(tab.doc.hits.len() as u64))
                    };
                    ui.label(count_text);
                    if ui.small_button("×").on_hover_text(lang.tr("Tutup (Esc)")).clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.zen_search_open = false;
        }
    }
}
