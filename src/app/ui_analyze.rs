// English comments: Analyzer panel — auto-parser + wizard + custom columns,
// SQL-lite query + chart + CSV, multi-log merge timeline + multi-file search.
// All scans are bounded and documented honestly in the UI.

use crate::engine::merge::{self, MergedRow};
use crate::engine::parser::{self, LogParser};
use crate::engine::squery::{self, QueryResult};

use super::AsisLogApp;

const MERGE_SCAN_PER_FILE: u64 = 200_000;
const MERGE_VIEW_CAP: usize = 5_000;
const SQL_VIEW_CAP: usize = 500;

/// Deferred panel actions needing `&mut app` after the egui closure.
enum PanelAct {
    Csv,
    Merge,
}

pub struct AnalyzePanel {
    pub tab: usize,
    pub parsers: Vec<LogParser>,
    pub active_name: Option<String>,
    pub detect_msg: String,
    pub wiz_sample: String,
    pub wiz_pattern: String,
    pub wiz_msg: String,
    pub cols_text: String,
    pub sql_text: String,
    pub sql_history: Vec<String>,
    pub sql_result: Option<QueryResult>,
    pub sql_msg: String,
    pub merge_rows: Vec<MergedRow>,
    pub merge_skew: String,
    pub merge_msg: String,
    pub ms_text: String,
    pub ms_regex: bool,
    pub ms_case: bool,
    pub ms_rows: Vec<(String, usize, bool)>,
    pub ms_msg: String,
}

impl AnalyzePanel {
    pub fn new(
        parsers: Vec<LogParser>,
        active: Option<String>,
        cols: Vec<String>,
        hist: Vec<String>,
    ) -> Self {
        Self {
            tab: 0,
            parsers,
            active_name: active,
            detect_msg: String::new(),
            wiz_sample: String::new(),
            wiz_pattern: "{TS} {LVL} {MSG}".to_string(),
            wiz_msg: String::new(),
            cols_text: cols.join(", "),
            sql_text: "SELECT level, msg WHERE level = ERROR GROUP BY level ORDER BY count DESC LIMIT 20"
                .to_string(),
            sql_history: hist,
            sql_result: None,
            sql_msg: String::new(),
            merge_rows: Vec::new(),
            merge_skew: String::new(),
            merge_msg: String::new(),
            ms_text: String::new(),
            ms_regex: false,
            ms_case: false,
            ms_rows: Vec::new(),
            ms_msg: String::new(),
        }
    }

    pub fn custom_cols(&self) -> Vec<String> {
        self.cols_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Resolve active parser: custom saved first, then builtin, None = auto.
    pub fn active_parser(&self) -> Option<LogParser> {
        let name = self.active_name.as_ref()?;
        if let Some(p) = self.parsers.iter().find(|p| &p.name == name) {
            return Some(p.clone());
        }
        if let Some(p) = parser::builtin_parsers()
            .iter()
            .find(|p| &p.name == name)
        {
            return Some(p.clone());
        }
        None
    }

    pub fn compiled_active(&self) -> Result<Option<regex::Regex>, String> {
        match self.active_parser() {
            Some(p) => Ok(Some(p.compiled()?)),
            None => Ok(None),
        }
    }

    fn push_sql_hist(&mut self, q: String) {
        self.sql_history.retain(|h| h != &q);
        self.sql_history.insert(0, q);
        self.sql_history.truncate(30);
    }
}

impl AsisLogApp {
    pub(crate) fn render_analyze(&mut self, ctx: &egui::Context, cur_idx: usize) {
        let lang = self.lang;
        let mut close = false;
        let mut acts: Vec<PanelAct> = Vec::new();

        egui::Window::new(lang.tr("Analisis Log (Parser · SQL · Gabung)"))
            .default_width(780.0)
            .default_height(540.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.analyze.tab, 0, lang.tr("Parser"));
                    ui.selectable_value(&mut self.analyze.tab, 1, lang.tr("SQL-lite"));
                    ui.selectable_value(&mut self.analyze.tab, 2, lang.tr("Gabung"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();
                match self.analyze.tab {
                    0 => Self::analyze_parser_ui(self, ui, cur_idx),
                    1 => Self::analyze_sql_ui(self, ui, cur_idx, &mut acts),
                    _ => Self::analyze_merge_ui(self, ui, &mut acts),
                }
            });

        for a in acts {
            match a {
                PanelAct::Csv => {
                    if let Some(res) = self.analyze.sql_result.clone() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name("asislog-hasil.csv")
                            .save_file()
                        {
                            match squery::export_csv(&p, &res) {
                                Ok(()) => {
                                    self.analyze.sql_msg =
                                        lang.f1("CSV tersimpan: {} baris.", res.rows.len());
                                }
                                Err(e) => self.analyze.sql_msg = lang.tr_status(&e),
                            }
                        }
                    }
                }
                PanelAct::Merge => {
                    let rows = self.analyze.merge_rows.clone();
                    if !rows.is_empty() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name("asislog-gabung.txt")
                            .save_file()
                        {
                            match merge::export_merge(&p, &rows) {
                                Ok(n) => {
                                    self.analyze.merge_msg =
                                        lang.f1("Gabungan tersimpan: {} baris.", n);
                                }
                                Err(e) => self.analyze.merge_msg = lang.tr_status(&e),
                            }
                        }
                    }
                }
            }
        }
        if close {
            self.analyze_open = false;
        }
    }

    fn analyze_parser_ui(app: &mut AsisLogApp, ui: &mut egui::Ui, cur_idx: usize) {
        let lang = app.lang;
        if ui.button(lang.tr("Deteksi format tab ini")).clicked() {
            let sample: Vec<String> = if cur_idx < app.tabs.len() {
                let tab = &mut app.tabs[cur_idx];
                let total = if tab.doc.index.complete {
                    tab.doc.index.total_lines
                } else {
                    tab.doc.line_count_estimate()
                };
                let step = (total / 200).max(1);
                let mut v = Vec::new();
                let mut ln = 1u64;
                while ln <= total && v.len() < 200 {
                    if let Some(t) = tab.doc.get_line_text(ln) {
                        v.push(t);
                    }
                    ln += step;
                }
                v
            } else {
                Vec::new()
            };
            if sample.is_empty() {
                app.analyze.detect_msg = lang.tr("Tab kosong / belum terindeks.").to_string();
            } else {
                let (kind, pct, n) = parser::detect_format(&sample);
                let name = match kind {
                    parser::DetectedKind::Json => "JSON",
                    parser::DetectedKind::Sql => "SQL-dump",
                    parser::DetectedKind::Generic => "Generik (cap waktu + level)",
                    parser::DetectedKind::Plain => "Teks polos (tanpa pola dominan)",
                };
                app.analyze.detect_msg =
                    lang.f3("Terdeteksi: {} ({}% dari {} baris sampel).", name, pct, n);
            }
        }
        if !app.analyze.detect_msg.is_empty() {
            ui.label(&app.analyze.detect_msg);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(lang.tr("Parser aktif:"));
            let cur_label = app.analyze.active_name.clone().unwrap_or_else(|| {
                lang.tr("Otomatis (JSON → SQL → generik)").to_string()
            });
            egui::ComboBox::from_id_salt("an_parser")
                .selected_text(cur_label)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            app.analyze.active_name.is_none(),
                            lang.tr("Otomatis (JSON → SQL → generik)"),
                        )
                        .clicked()
                    {
                        app.analyze.active_name = None;
                        app.cfg_dirty = true;
                    }
                    for b in parser::builtin_parsers() {
                        let sel = app.analyze.active_name.as_deref() == Some(&b.name);
                        if ui
                            .selectable_label(
                                sel,
                                format!("{} ▸ {}", lang.tr("Bawaan"), b.name),
                            )
                            .clicked()
                        {
                            app.analyze.active_name = Some(b.name.clone());
                            app.cfg_dirty = true;
                        }
                    }
                    let names: Vec<String> =
                        app.analyze.parsers.iter().map(|p| p.name.clone()).collect();
                    for n in names {
                        let sel = app.analyze.active_name.as_deref() == Some(&n);
                        if ui
                            .selectable_label(
                                sel,
                                format!("{} ▸ {}", lang.tr("Kustom"), n),
                            )
                            .clicked()
                        {
                            app.analyze.active_name = Some(n);
                            app.cfg_dirty = true;
                        }
                    }
                });
        });
        if let Some(p) = app.analyze.active_parser() {
            ui.monospace(&p.pattern);
            ui.weak(lang.f1("Kolom: {}", p.field_names().join(", ")));
        } else {
            ui.weak(lang.tr("Otomatis: tiap baris dicoba JSON → kolom SQL → pola generik; kolom kustom di bawah tetap ditambahkan ke tabel SQL."));
        }
        ui.separator();
        ui.strong(lang.tr("Parser kustom tersimpan:"));
        if app.analyze.parsers.is_empty() {
            ui.weak(lang.tr("Belum ada. Buat lewat wizard di bawah."));
        } else {
            let mut del: Option<usize> = None;
            for (i, p) in app.analyze.parsers.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&p.name);
                    ui.monospace(&p.pattern);
                    if ui.small_button(lang.tr("Pakai")).clicked() {
                        app.analyze.active_name = Some(p.name.clone());
                        app.cfg_dirty = true;
                    }
                    if ui
                        .small_button("×")
                        .on_hover_text(lang.tr("Hapus parser"))
                        .clicked()
                    {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                let gone = app.analyze.parsers.remove(i);
                if app.analyze.active_name.as_deref() == Some(&gone.name) {
                    app.analyze.active_name = None;
                }
                app.cfg_dirty = true;
            }
        }
        ui.separator();
        ui.strong(lang.tr("Wizard parser:"));
        ui.horizontal(|ui| {
            ui.label(lang.tr("Contoh baris:"));
            if ui.small_button(lang.tr("Ambil baris terpilih")).clicked()
                && cur_idx < app.tabs.len()
            {
                let tab = &mut app.tabs[cur_idx];
                let ln = tab.selected_line.max(1);
                app.analyze.wiz_sample = tab.doc.get_line_text(ln).unwrap_or_default();
            }
        });
        ui.text_edit_singleline(&mut app.analyze.wiz_sample);
        ui.label(lang.tr("Pola (regex + (?P<nama>...) atau singkatan {TS} {LVL} {MSG} {kolom} {kolom:regex}):"));
        ui.text_edit_singleline(&mut app.analyze.wiz_pattern);
        ui.weak("Cth: {TS} {LVL} {MSG} · [{ts:.+?}] [{level:.+?}] {MSG}");
        ui.horizontal(|ui| {
            if ui.button(lang.tr("Uji pola")).clicked() {
                let p = LogParser {
                    name: "uji".to_string(),
                    pattern: app.analyze.wiz_pattern.clone(),
                };
                match p.compiled() {
                    Err(e) => app.analyze.wiz_msg = lang.tr_status(&e),
                    Ok(re) => match parser::parse_with(&re, &app.analyze.wiz_sample) {
                        None => {
                            app.analyze.wiz_msg = lang
                                .tr("Pola valid tapi TIDAK cocok dengan contoh.")
                                .to_string()
                        }
                        Some(r) => {
                            let f: Vec<String> = r
                                .fields
                                .iter()
                                .map(|(k, v)| format!("{}={}", k, v))
                                .collect();
                            app.analyze.wiz_msg =
                                lang.f1("Cocok! Kolom: {}", f.join(" · "));
                        }
                    },
                }
            }
            if ui.button(lang.tr("Simpan parser")).clicked() {
                let n = format!("Kustom {}", app.analyze.parsers.len() + 1);
                let p = LogParser {
                    name: n.clone(),
                    pattern: app.analyze.wiz_pattern.clone(),
                };
                match p.validate() {
                    Err(e) => app.analyze.wiz_msg = lang.tr_status(&e),
                    Ok(names) => {
                        app.analyze.parsers.push(p);
                        app.analyze.active_name = Some(n);
                        app.cfg_dirty = true;
                        app.analyze.wiz_msg =
                            lang.f1("Parser tersimpan. Kolom: {}", names.join(", "));
                    }
                }
            }
        });
        if !app.analyze.wiz_msg.is_empty() {
            ui.label(&app.analyze.wiz_msg);
        }
        ui.separator();
        ui.strong(lang.tr("Kolom kustom (tambah ke tabel SQL, pisah koma):"));
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut app.analyze.cols_text);
            if ui.small_button(lang.tr("Simpan")).clicked() {
                app.cfg_dirty = true;
                app.save_config();
            }
        });
        ui.weak(lang.tr("Mis. host, status, session — diambil dari parser/JSON/k=v bila ada, kosong bila tidak."));
    }

    fn analyze_sql_ui(
        app: &mut AsisLogApp,
        ui: &mut egui::Ui,
        cur_idx: usize,
        acts: &mut Vec<PanelAct>,
    ) {
        let lang = app.lang;
        ui.label(lang.tr("Dialek SQL-lite (bukan Transact-SQL penuh): SELECT koloms / * / COUNT(*) · WHERE AND OR NOT = != ~ !~ > < >= <= · GROUP BY · ORDER BY count DESC · LIMIT. Tanpa SELECT = filter WHERE saja. Pindai dibatasi 2 jt baris pertama tab aktif."));
        ui.text_edit_multiline(&mut app.analyze.sql_text);
        ui.horizontal(|ui| {
            if ui.button(lang.tr("Jalankan di tab aktif")).clicked() {
                Self::run_sql(app, cur_idx);
            }
            if ui.button(lang.tr("Ekspor CSV")).clicked() && app.analyze.sql_result.is_some() {
                acts.push(PanelAct::Csv);
            }
            if !app.analyze.sql_history.is_empty() {
                let hist = app.analyze.sql_history.clone();
                egui::ComboBox::from_id_salt("an_sqlhist")
                    .selected_text(lang.tr("Riwayat"))
                    .show_ui(ui, |ui| {
                        for h in hist {
                            if ui.selectable_label(false, &h).clicked() {
                                app.analyze.sql_text = h;
                            }
                        }
                    });
            }
        });
        if !app.analyze.sql_msg.is_empty() {
            ui.label(&app.analyze.sql_msg);
        }
        if let Some(res) = app.analyze.sql_result.clone() {
            ui.separator();
            if res.is_grouped && res.rows.len() > 1 {
                ui.strong(lang.tr("Grafik:"));
                let max_c: usize = res
                    .rows
                    .iter()
                    .filter_map(|r| r.last()?.parse::<usize>().ok())
                    .max()
                    .unwrap_or(1)
                    .max(1);
                let count_idx = res.cols.len().saturating_sub(1);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    for r in res.rows.iter().take(20) {
                        let c: usize =
                            r.get(count_idx).and_then(|s| s.parse().ok()).unwrap_or(0);
                        let bar_len = ((c as f64 / max_c as f64) * 28.0).round() as usize;
                        let label = r[..count_idx].join(" | ");
                        ui.monospace(format!(
                            "{:>8} {:<28} {}",
                            crate::engine::format_count(c as u64),
                            "█".repeat(bar_len),
                            label
                        ));
                    }
                });
                ui.separator();
            }
            ui.strong(lang.f1("Hasil ({} baris tampil):", res.rows.len().min(SQL_VIEW_CAP)));
            egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("an_sqlgrid").striped(true).show(ui, |ui| {
                    for c in &res.cols {
                        ui.strong(c);
                    }
                    ui.end_row();
                    for r in res.rows.iter().take(SQL_VIEW_CAP) {
                        for cell in r {
                            let mut s = cell.clone();
                            if s.chars().count() > 80 {
                                s = s.chars().take(80).collect::<String>() + "…";
                            }
                            ui.monospace(s);
                        }
                        ui.end_row();
                    }
                });
            });
            if res.rows.len() > SQL_VIEW_CAP {
                ui.weak(lang.f1(
                    "…dan {} baris lain (ekspor CSV untuk semua).",
                    res.rows.len() - SQL_VIEW_CAP,
                ));
            }
        }
    }

    fn run_sql(app: &mut AsisLogApp, cur_idx: usize) {
        let lang = app.lang;
        if cur_idx >= app.tabs.len() {
            app.analyze.sql_msg = lang.tr("Tidak ada tab terbuka.").to_string();
            app.analyze.sql_result = None;
            return;
        }
        let q = match squery::parse_query(&app.analyze.sql_text) {
            Ok(q) => q,
            Err(e) => {
                app.analyze.sql_msg = lang.tr_status(&e);
                app.analyze.sql_result = None;
                return;
            }
        };
        let custom = match app.analyze.compiled_active() {
            Ok(c) => c,
            Err(e) => {
                app.analyze.sql_msg = lang.tr_status(&e);
                app.analyze.sql_result = None;
                return;
            }
        };
        let cols = app.analyze.custom_cols();
        let tab = &mut app.tabs[cur_idx];
        let total = if tab.doc.index.complete {
            tab.doc.index.total_lines
        } else {
            tab.doc.line_count_estimate()
        };
        let res = squery::run_on_doc(&mut tab.doc, custom.as_ref(), &q, 0, &cols);
        let mut note = lang.f3(
            "Pindai {} / {} · {} cocok.",
            crate::engine::format_count(res.scanned),
            crate::engine::format_count(total),
            crate::engine::format_count(res.matched),
        );
        if res.truncated_scan {
            note.push_str(&format!(" ({})", lang.tr("dibatasi 2 jt baris")));
        }
        if res.truncated_rows {
            note.push_str(&format!(" ({})", lang.tr("baris dipangkas LIMIT")));
        }
        app.analyze.sql_msg = note;
        app.analyze.sql_result = Some(res);
        let hist_q = app.analyze.sql_text.clone();
        app.analyze.push_sql_hist(hist_q);
        app.cfg_dirty = true;
    }

    fn analyze_merge_ui(app: &mut AsisLogApp, ui: &mut egui::Ui, acts: &mut Vec<PanelAct>) {
        let lang = app.lang;
        ui.label(lang.tr("Gabung N log jadi 1 timeline sortir-cap-waktu (maks 200 rb baris/file, tampil 5 rb). Multi-cari memakai mesin yang sama dengan pencarian tab."));
        ui.horizontal(|ui| {
            if ui.button(lang.tr("Bangun timeline semua tab")).clicked() {
                Self::build_merge(app);
            }
            if ui.button(lang.tr("Ekspor gabungan")).clicked()
                && !app.analyze.merge_rows.is_empty()
            {
                acts.push(PanelAct::Merge);
            }
        });
        if !app.analyze.merge_msg.is_empty() {
            ui.label(&app.analyze.merge_msg);
        }
        if !app.analyze.merge_skew.is_empty() {
            ui.group(|ui| {
                ui.strong(lang.tr("Cakupan & skew waktu:"));
                ui.monospace(&app.analyze.merge_skew);
            });
        }
        if !app.analyze.merge_rows.is_empty() {
            ui.separator();
            ui.strong(lang.f1(
                "Timeline ({} baris):",
                app.analyze.merge_rows.len().min(MERGE_VIEW_CAP),
            ));
            let rows = app.analyze.merge_rows.clone();
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for r in rows.iter().take(MERGE_VIEW_CAP) {
                    ui.horizontal(|ui| {
                        let ts = if r.ts_raw.is_empty() { "-" } else { &r.ts_raw };
                        ui.weak(ts);
                        ui.weak(format!(
                            "{}:{}",
                            r.file,
                            crate::engine::format_count(r.line)
                        ));
                        let mut s = r.text.clone();
                        if s.chars().count() > 110 {
                            s = s.chars().take(110).collect::<String>() + "…";
                        }
                        if ui.link(s).clicked() {
                            app.current = app
                                .tabs
                                .iter()
                                .position(|t| t.doc.file_name == r.file)
                                .unwrap_or(app.current);
                            let idx = app.current;
                            if idx < app.tabs.len() {
                                app.tabs[idx].nav_to(r.line);
                            }
                        }
                    });
                }
            });
        }
        ui.separator();
        ui.strong(lang.tr("Cari di semua tab:"));
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut app.analyze.ms_text);
            ui.checkbox(&mut app.analyze.ms_regex, lang.tr("Regex"));
            ui.checkbox(&mut app.analyze.ms_case, lang.tr("Aa"));
            if ui.button(lang.tr("Cari semua")).clicked() {
                Self::run_multisearch(app);
            }
        });
        if !app.analyze.ms_msg.is_empty() {
            ui.label(&app.analyze.ms_msg);
        }
        if !app.analyze.ms_rows.is_empty() {
            let rows = app.analyze.ms_rows.clone();
            egui::Grid::new("an_msgrid").striped(true).show(ui, |ui| {
                ui.strong(lang.tr("File"));
                ui.strong(lang.tr("Hasil"));
                ui.strong("");
                ui.end_row();
                for (f, n, trunc) in rows.iter() {
                    ui.label(f);
                    ui.label(format!(
                        "{}{}",
                        crate::engine::format_count(*n as u64),
                        if *trunc { "+" } else { "" }
                    ));
                    if ui.small_button(lang.tr("Buka")).clicked() {
                        let q = app.analyze.ms_text.clone();
                        let rx = app.analyze.ms_regex;
                        let cs = app.analyze.ms_case;
                        if let Some(idx) = app.tabs.iter().position(|t| t.doc.file_name == *f)
                        {
                            app.current = idx;
                            let t = &mut app.tabs[idx];
                            t.search_text = q;
                            t.regex_on = rx;
                            t.case_sensitive = cs;
                            let hist = &mut app.history;
                            t.start_search(hist);
                        }
                    }
                    ui.end_row();
                }
            });
            ui.weak(lang.tr("'+' = dipangkas 50 rb/file; buka tab untuk pindaian penuh + panel hasil."));
        }
    }

    fn build_merge(app: &mut AsisLogApp) {
        let lang = app.lang;
        if app.tabs.is_empty() {
            app.analyze.merge_msg = lang.tr("Tidak ada tab terbuka.").to_string();
            return;
        }
        let mut all = Vec::new();
        let mut srcs = Vec::new();
        for tab in app.tabs.iter_mut() {
            let (mut r, s) = merge::collect_rows(&mut tab.doc, MERGE_SCAN_PER_FILE, 512);
            all.append(&mut r);
            srcs.push(s);
        }
        app.analyze.merge_skew = merge::skew_report(&srcs);
        let total = all.len();
        app.analyze.merge_rows = merge::sort_timeline(all);
        app.analyze.merge_msg = lang.f2(
            "Timeline: {} baris dari {} file (maks 200 rb/file).",
            crate::engine::format_count(total as u64),
            srcs.len(),
        );
    }

    fn run_multisearch(app: &mut AsisLogApp) {
        let lang = app.lang;
        if app.analyze.ms_text.is_empty() {
            app.analyze.ms_msg = lang.tr("Isi dulu pola pencarian.").to_string();
            return;
        }
        let docs: Vec<&crate::engine::Doc> = app.tabs.iter().map(|t| &t.doc).collect();
        match merge::search_all(
            &docs,
            &app.analyze.ms_text,
            app.analyze.ms_regex,
            app.analyze.ms_case,
            50_000,
        ) {
            Err(e) => {
                app.analyze.ms_msg = lang.tr_status(&e);
                app.analyze.ms_rows.clear();
            }
            Ok(counts) => {
                let names: Vec<String> =
                    app.tabs.iter().map(|t| t.doc.file_name.clone()).collect();
                let total: usize = counts.iter().map(|(n, _)| n).sum();
                app.analyze.ms_rows = names
                    .into_iter()
                    .zip(counts)
                    .map(|(f, (n, t))| (f, n, t))
                    .collect();
                app.analyze.ms_msg = lang.f2(
                    "Total {} hasil di {} file.",
                    crate::engine::format_count(total as u64),
                    app.analyze.ms_rows.len(),
                );
            }
        }
    }
}
