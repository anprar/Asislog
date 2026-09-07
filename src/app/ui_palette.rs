// English comments: Command palette (Ctrl+Shift+P) fuzzy action registry.

use crate::ui::theme::Tema;
use super::AsisLogApp;

pub struct PaletteAction {
    pub title: &'static str,
    pub shortcut: &'static str,
    pub category: &'static str,
    pub id: &'static str,
}

pub const PALETTE_ACTIONS: &[PaletteAction] = &[
    PaletteAction { title: "Buka file log…", shortcut: "Ctrl+O", category: "File", id: "open_file" },
    PaletteAction { title: "Fokus pencarian", shortcut: "Ctrl+F", category: "Navigasi", id: "focus_search" },
    PaletteAction { title: "Toggle Mode Zen (Kepadatan)", shortcut: "F11", category: "Tampilan", id: "toggle_zen" },
    PaletteAction { title: "Panel belah (dual-pane hasil)", shortcut: "", category: "Tampilan", id: "toggle_split" },
    PaletteAction { title: "Toggle Ikuti log (LIVE)", shortcut: "Ctrl+Shift+F", category: "Log", id: "toggle_follow" },
    PaletteAction { title: "Ke baris / cap waktu…", shortcut: "Ctrl+G", category: "Navigasi", id: "goto_line" },
    PaletteAction { title: "Ekspor hasil pencarian…", shortcut: "Ctrl+E", category: "Ekspor", id: "export_hits" },
    PaletteAction { title: "Tiket Markdown Jira (1-klik)", shortcut: "", category: "Ekspor", id: "export_jira" },
    PaletteAction { title: "Tambah / Hapus Penanda baris", shortcut: "Ctrl+B", category: "Penanda", id: "toggle_bookmark" },
    PaletteAction { title: "Panel Penanda", shortcut: "Ctrl+Shift+B", category: "Penanda", id: "toggle_marks_panel" },
    PaletteAction { title: "Catatan / Scratchpad (Base64/JWT/SQL)", shortcut: "", category: "Alat", id: "open_scratch" },
    PaletteAction { title: "Aturan Sorotan Warna", shortcut: "", category: "Tampilan", id: "open_highlights" },
    PaletteAction { title: "Simpan Workspace…", shortcut: "", category: "Workspace", id: "save_workspace" },
    PaletteAction { title: "Buka Workspace…", shortcut: "", category: "Workspace", id: "open_workspace" },
    PaletteAction { title: "Tampilan Kolom SQL (Toggle)", shortcut: "", category: "Tampilan", id: "toggle_sql_cols" },
    PaletteAction { title: "Panel Histogram Waktu ERROR (Toggle)", shortcut: "", category: "Investigasi", id: "toggle_hist" },
    PaletteAction { title: "Agregasi Top-N Error / Sesi", shortcut: "", category: "Investigasi", id: "open_top_n" },
    PaletteAction { title: "Hex Peek (Mode biner aman)", shortcut: "", category: "Investigasi", id: "open_hex_peek" },
    PaletteAction { title: "Tema: Sistem (Otomatis OS)", shortcut: "", category: "Tema", id: "theme_system" },
    PaletteAction { title: "Tema: Gelap", shortcut: "", category: "Tema", id: "theme_dark" },
    PaletteAction { title: "Tema: Terang", shortcut: "", category: "Tema", id: "theme_light" },
    PaletteAction { title: "Tema: Kontras Tinggi", shortcut: "", category: "Tema", id: "theme_contrast" },
    PaletteAction { title: "Tema: Monokai", shortcut: "", category: "Tema", id: "theme_monokai" },
    PaletteAction { title: "Tema: Senja Biru", shortcut: "", category: "Tema", id: "theme_dusk" },
    PaletteAction { title: "Tema: Solarized Gelap", shortcut: "", category: "Tema", id: "theme_solar_dark" },
    PaletteAction { title: "Tema: Solarized Terang", shortcut: "", category: "Tema", id: "theme_solar_light" },
    PaletteAction { title: "Ganti bahasa / Switch language", shortcut: "", category: "Tampilan", id: "toggle_language" },
    PaletteAction { title: "Zoom: Perbesar (+10%)", shortcut: "Ctrl+=", category: "Tampilan", id: "zoom_in" },
    PaletteAction { title: "Zoom: Perkecil (-10%)", shortcut: "Ctrl+-", category: "Tampilan", id: "zoom_out" },
    PaletteAction { title: "Zoom: Reset (100%)", shortcut: "Ctrl+0", category: "Tampilan", id: "zoom_reset" },
    PaletteAction { title: "Daftar Pintasan Keyboard", shortcut: "F1", category: "Bantuan", id: "open_shortcuts" },
];

impl AsisLogApp {
    pub(crate) fn render_palette(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let mut close = false;
        let mut execute_action: Option<&'static str> = None;

        // Filter actions with fuzzy score (match in both languages)
        let query = self.palette_query.trim().to_lowercase();
        let matched: Vec<&'static PaletteAction> = PALETTE_ACTIONS
            .iter()
            .filter(|act| {
                if query.is_empty() {
                    true
                } else {
                    let title_id = act.title.to_lowercase();
                    let title_en = lang.tr(act.title).to_lowercase();
                    let cat_id = act.category.to_lowercase();
                    let cat_en = lang.tr(act.category).to_lowercase();
                    title_id.contains(&query)
                        || title_en.contains(&query)
                        || cat_id.contains(&query)
                        || cat_en.contains(&query)
                }
            })
            .collect();

        if matched.is_empty() {
            self.palette_selected = 0;
        } else if self.palette_selected >= matched.len() {
            self.palette_selected = matched.len() - 1;
        }

        // Handle keyboard navigation for palette
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !matched.is_empty() {
            self.palette_selected = (self.palette_selected + 1).min(matched.len() - 1);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !matched.is_empty() {
            self.palette_selected = self.palette_selected.saturating_sub(1);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !matched.is_empty() {
            execute_action = Some(matched[self.palette_selected].id);
            close = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        egui::Window::new(lang.tr("Palet Perintah"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
            .fixed_size([540.0, 360.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(">");
                    let re = ui.add(
                        egui::TextEdit::singleline(&mut self.palette_query)
                            .hint_text(lang.tr("Ketik nama perintah..."))
                            .desired_width(460.0),
                    );
                    re.request_focus();
                    if ui.small_button("×").on_hover_text(lang.tr("Tutup (Esc)")).clicked() {
                        close = true;
                    }
                });
                ui.separator();

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if matched.is_empty() {
                            ui.label(lang.tr("Tidak ada aksi yang cocok."));
                        } else {
                            for (i, act) in matched.iter().enumerate() {
                                let is_sel = i == self.palette_selected;
                                ui.horizontal(|ui| {
                                    let resp = ui.selectable_label(is_sel, lang.tr(act.title));
                                    if resp.clicked() {
                                        execute_action = Some(act.id);
                                        close = true;
                                    }
                                    if !act.shortcut.is_empty() {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.weak(act.shortcut);
                                        });
                                    }
                                });
                            }
                        }
                    });
            });

        if let Some(act) = execute_action {
            self.execute_palette_action(act, ctx);
        }
        if close {
            self.palette_open = false;
        }
    }

    fn execute_palette_action(&mut self, id: &str, ctx: &egui::Context) {
        match id {
            "open_file" => self.open_dialog(),
            "focus_search" => {
                if self.zen_mode {
                    self.zen_search_open = true;
                }
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("cari")));
            }
            "toggle_zen" => {
                self.zen_mode = !self.zen_mode;
                self.cfg_dirty = true;
                let lang = self.lang;
                self.global_status = if self.zen_mode {
                    lang.tr("Mode Zen aktif (F11 untuk kembali).").to_string()
                } else {
                    lang.tr("Mode Zen dinonaktifkan.").to_string()
                };
            }
            "toggle_follow" => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.doc.follow = !tab.doc.follow;
                    tab.doc.stick_bottom = tab.doc.follow;
                }
            }
            "goto_line" => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.goto_open = true;
                }
            }
            "export_hits" => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.export_open = true;
                }
            }
            "export_jira" => {
                let lang = self.lang;
                if let Some(tab) = self.current_tab_mut() {
                    let path = std::env::temp_dir().join(format!("tiket-jira-{}.md", std::process::id()));
                    match tab.doc.export_ticket_to_file(&path, &tab.search_text, 10) {
                        Ok(n) => tab.doc.status = lang.f2("Tiket disimpan ({} hasil): {}", n, path.display()),
                        Err(e) => tab.doc.status = lang.tr_status(&e),
                    }
                }
            }
            "toggle_bookmark" => {
                if let Some(tab) = self.current_tab_mut() {
                    let ln = tab.selected_line;
                    tab.doc.toggle_bookmark(ln);
                    tab.marks_dirty = true;
                    tab.refresh_mode_map();
                }
            }
            "toggle_marks_panel" => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.show_bookmarks = !tab.show_bookmarks;
                }
            }
            "toggle_split" => {
                self.split_view = !self.split_view;
                self.cfg_dirty = true;
                if self.split_view {
                    if let Some(t) = self.current_tab_mut() {
                        t.results_collapsed = false;
                    }
                }
            }
            "open_scratch" => self.scratch_open = true,
            "open_highlights" => self.hl_open = true,
            "save_workspace" => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .set_file_name("workspace-asislog.json")
                    .save_file()
                {
                    self.save_workspace_to(&p);
                }
            }
            "open_workspace" => {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                    match crate::store::load_workspace(&p) {
                        Ok(ws) => self.open_workspace(ws),
                        Err(e) => self.global_status = e,
                    }
                }
            }
            "toggle_sql_cols" => {
                self.sql_cols_enabled = !self.sql_cols_enabled;
                self.cfg_dirty = true;
            }
            "toggle_hist" => self.hist_panel_open = !self.hist_panel_open,
            "open_top_n" => {
                self.top_n_open = true;
                if let Some(tab) = self.current_tab_mut() {
                    let errors = crate::engine::top_n::top_errors(&tab.doc, 10);
                    self.top_n_results = errors.into_iter().map(|it| (it.label, it.count)).collect();
                }
            }
            "open_hex_peek" => self.hex_peek_open = true,
            "theme_system" => { self.tema = Tema::Sistem; self.cfg_dirty = true; }
            "theme_dark" => { self.tema = Tema::Gelap; self.cfg_dirty = true; }
            "theme_light" => { self.tema = Tema::Terang; self.cfg_dirty = true; }
            "theme_contrast" => { self.tema = Tema::KontrasTinggi; self.cfg_dirty = true; }
            "theme_monokai" => { self.tema = Tema::Monokai; self.cfg_dirty = true; }
            "theme_dusk" => { self.tema = Tema::SenjaBiru; self.cfg_dirty = true; }
            "theme_solar_dark" => { self.tema = Tema::SolarGelap; self.cfg_dirty = true; }
            "theme_solar_light" => { self.tema = Tema::SolarTerang; self.cfg_dirty = true; }
            "toggle_language" => {
                use crate::i18n::Lang;
                self.set_lang(if self.lang == Lang::En { Lang::Id } else { Lang::En });
            }
            "zoom_in" => self.bump_zoom(ctx, self.zoom + 0.1),
            "zoom_out" => self.bump_zoom(ctx, self.zoom - 0.1),
            "zoom_reset" => self.bump_zoom(ctx, 1.0),
            "open_shortcuts" => self.shortcuts_open = true,
            _ => {}
        }
    }
}
