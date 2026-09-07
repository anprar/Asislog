// English comments: AsisLogApp actions: shortcuts, goto, blocks, copy, time range (split from app.rs; behavior unchanged).
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
    /// Where is the keyboard focus? Returns (text_field_focused, dialog_open).
    /// Viewport/global shortcuts must yield when the user is typing in a
    /// field (arrows/Home/End/Tab belong to the caret) or when any dialog
    /// is open (shortcuts would act behind the dialog).
    pub(crate) fn focus_state(&self, ctx: &egui::Context) -> (bool, bool) {
        let field = ["cari", "saring", "tandai-saring"]
            .iter()
            .any(|id| ctx.memory(|m| m.focused() == Some(egui::Id::new(*id))));
        let dialog = self.hl_open
            || self.preset_save_open
            || self.rename_open
            || self.range_open
            || self.shortcuts_open
            || self.url_open
            || self.paste_open
            || self.scratch_open
            || self.palette_open
            || self.zen_search_open
            || self.hist_panel_open
            || self.top_n_open
            || self.hex_peek_open
            || self.confirm.is_some()
            || self.global_error.is_some()
            || self
                .tabs
                .get(self.current)
                .map(|t| t.goto_open || t.export_open || t.scope_open)
                .unwrap_or(false);
        (field, dialog)
    }

    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = ctx.input(|i| i.modifiers.ctrl);
        let shift = ctx.input(|i| i.modifiers.shift);
        // Ctrl+O
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::O)) {
            self.open_dialog();
        }
        // F1: jendela daftar pintasan (berlaku tanpa tab).
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.shortcuts_open = true;
        }
        // F11: toggle Mode Zen (C-B1)
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            self.zen_mode = !self.zen_mode;
            self.cfg_dirty = true;
            self.global_status = if self.zen_mode {
                String::from("Mode Zen aktif (F11 untuk kembali).")
            } else {
                String::from("Mode Zen dinonaktifkan.")
            };
        }
        // Ctrl+Shift+P: Command Palette (C-C3)
        if ctrl && shift && ctx.input(|i| i.key_pressed(egui::Key::P)) {
            self.palette_open = !self.palette_open;
            self.palette_query.clear();
            self.palette_selected = 0;
        }
        // Ctrl+F fokus cari: kami tandai lewat status (fokus widget di bawah via id)
        if ctrl && !shift && ctx.input(|i| i.key_pressed(egui::Key::F)) {
            if self.zen_mode {
                self.zen_search_open = true;
            }
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("cari")));
        }
        // Angka & n/N milik editor saat mengetik di kolom teks / dialog terbuka.
        // (Dihitung SEBELUM pinjam tab: current_tab_mut meminjam seluruh self.)
        let (field_focused, dialog_open) = self.focus_state(ctx);
        let typing = field_focused || dialog_open;
        let Some(tab) = self.current_tab_mut() else { return };
        let mut sess_touch = false;
        // F3 / Shift+F3: boleh saat mengetik query (tangan di keyboard),
        // tapi jangan di balik dialog yang terbuka.
        if !dialog_open && ctx.input(|i| i.key_pressed(egui::Key::F3)) && !tab.doc.hits.is_empty() {
            let n = tab.doc.hits.len();
            let cur = tab.current_hit.unwrap_or(0);
            let nxt = if shift {
                cur.saturating_sub(1).min(n - 1)
            } else {
                (cur + 1).min(n - 1)
            };
            tab.jump_to_hit(nxt);
        }
        // Ctrl+G: buka dialog (jangan menumpuk di atas dialog lain).
        if !dialog_open && ctrl && ctx.input(|i| i.key_pressed(egui::Key::G)) {
            tab.goto_open = true;
        }
        // Ctrl+E: dialog ekspor hasil
        if !dialog_open && ctrl && ctx.input(|i| i.key_pressed(egui::Key::E)) {
            tab.export_open = true;
        }
        // Ctrl+Home / Ctrl+End: milik caret saat mengetik di kolom teks.
        if !typing && ctrl && ctx.input(|i| i.key_pressed(egui::Key::Home)) {
            tab.top_row = 0;
            tab.selected_line = tab.row_to_line(0).unwrap_or(1);
            tab.doc.stick_bottom = false; // pergi dari ekor = jeda LIVE
        }
        if !typing && ctrl && ctx.input(|i| i.key_pressed(egui::Key::End)) {
            let total = tab.total_view_rows();
            tab.top_row = total.saturating_sub(tab.last_visible.max(10));
            tab.doc.stick_bottom = true;
        }
        // Ctrl+Shift+F: cermin tombol LIVE (aktif / jeda-lanjut / mati).
        if !typing && ctrl && shift && ctx.input(|i| i.key_pressed(egui::Key::F)) {
            if tab.doc.follow && !tab.doc.stick_bottom {
                tab.doc.stick_bottom = true;
            } else {
                tab.doc.follow = !tab.doc.follow;
                tab.doc.stick_bottom = tab.doc.follow;
            }
            sess_touch = true;
        }
        // Alt+Left / Alt+Right: history navigasi (bukan saat mengetik).
        let alt = ctx.input(|i| i.modifiers.alt);
        if !typing && alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) && !tab.go_hist(true) {
            tab.doc.status = String::from("Tidak ada lokasi sebelumnya.");
        }
        if !typing && alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) && !tab.go_hist(false) {
            tab.doc.status = String::from("Tidak ada lokasi berikutnya.");
        }
        // Alt+Up / Alt+Down: penanda sebelumnya/berikutnya
        if !typing && alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !tab.go_mark(true) {
            tab.doc.status = String::from("Belum ada penanda.");
        }
        if !typing && alt && ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !tab.go_mark(false) {
            tab.doc.status = String::from("Belum ada penanda.");
        }
        // Esc: tutup SATU dialog teratas (prioritas tetap); bila tak ada
        // dialog yang terbuka, batalkan pencarian berjalan TANPA menghapus
        // query maupun hasil yang sudah ada.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            let tab_handled = {
                let t = &mut *tab;
                if t.scope_open {
                    t.scope_open = false;
                    true
                } else if t.export_open {
                    t.export_open = false;
                    true
                } else if t.goto_open {
                    t.goto_open = false;
                    true
                } else {
                    false
                }
            };
            if !tab_handled {
                // Modal konfirmasi / palet / HUD paling atas; Esc = Batal/Tutup.
                if self.palette_open {
                    self.palette_open = false;
                } else if self.zen_search_open {
                    self.zen_search_open = false;
                } else if self.top_n_open {
                    self.top_n_open = false;
                } else if self.hex_peek_open {
                    self.hex_peek_open = false;
                } else if self.hist_panel_open {
                    self.hist_panel_open = false;
                } else if self.confirm.is_some() {
                    self.confirm = None;
                } else if self.global_error.is_some() {
                    self.global_error = None;
                } else if self.shortcuts_open {
                    self.shortcuts_open = false;
                } else if self.scratch_open {
                    self.scratch_open = false;
                } else if self.paste_open {
                    self.paste_open = false;
                } else if self.url_open {
                    self.url_open = false;
                } else if self.range_open {
                    self.range_open = false;
                } else if self.rename_open {
                    self.rename_open = false;
                } else if self.hl_open {
                    self.hl_open = false;
                } else if self.preset_save_open {
                    self.preset_save_open = false;
                } else if let Some(t) = self.current_tab_mut() {
                    // Batalkan pindaian worker; query dan hasil yang sudah
                    // terkumpul tetap tampil.
                    t.cancel_search_worker();
                    t.doc.status = String::from("Pencarian dibatalkan (Esc).");
                }
            }
        }
        if sess_touch {
            self.session_dirty = true;
        }
        // Ctrl+B: toggle penanda di baris aktif (+ simpan sidecar)
        if !dialog_open && ctrl && !shift && ctx.input(|i| i.key_pressed(egui::Key::B)) {
            let idx = self.current;
            if let Some(t) = self.tabs.get_mut(idx) {
                let ln = t.selected_line;
                t.doc.toggle_bookmark(ln);
                t.refresh_mode_map();
            }
            Self::save_marks(idx, &mut self.tabs);
        }
        // Ctrl+Shift+B: buka/tutup panel penanda
        if !dialog_open && ctrl && shift && ctx.input(|i| i.key_pressed(egui::Key::B)) {
            if let Some(t) = self.tabs.get_mut(self.current) {
                t.show_bookmarks = !t.show_bookmarks;
            }
        }
        // F2: ubah label penanda di baris aktif
        if !dialog_open && ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            let idx = self.current;
            let found = self.tabs.get(idx).and_then(|t| {
                let ln = t.selected_line;
                t.doc.bookmarks.iter().find(|b| b.line == ln).map(|b| {
                    (ln, b.label.clone(), b.color)
                })
            });
            match found {
                Some((ln, label, color)) => {
                    self.rename_line = ln;
                    self.rename_label = label;
                    self.rename_color = color;
                    self.rename_open = true;
                }
                None => {
                    if let Some(t) = self.tabs.get_mut(idx) {
                        t.doc.status = String::from(
                            "Tidak ada penanda di baris aktif. Tekan Ctrl+B dulu.",
                        );
                    }
                }
            }
        }
        // Ctrl+Tab / Ctrl+Shift+Tab: pindah tab, berlaku juga saat mengetik
        // di kolom teks (seperti peramban); Tab polos tetap milik caret/fokus.
        if ctrl && !alt && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let n = self.tabs.len();
            if n > 1 {
                if shift {
                    self.current = (self.current + n - 1) % n;
                } else {
                    self.current = (self.current + 1) % n;
                }
                self.session_dirty = true;
            }
        }
        // Zoom Ctrl+= / Ctrl+- / Ctrl+0 (jangan saat dialog terbuka).
        if !dialog_open && ctrl && !shift && !alt {
            if ctx.input(|i| i.key_pressed(egui::Key::Equals)) {
                self.bump_zoom(ctx, self.zoom + 0.1);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.bump_zoom(ctx, self.zoom - 0.1);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
                self.bump_zoom(ctx, 1.0);
            }
        }
        // Label warna 1-9 dari query aktif (bukan saat mengetik).
        if !typing && !ctrl && !alt {
            const NUMS: [egui::Key; 9] = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ];
            for (i, k) in NUMS.iter().enumerate() {
                if ctx.input(|ii| ii.key_pressed(*k)) {
                    self.toggle_label(i);
                    break;
                }
            }
            // n / N: hasil berikut/sebelum tanpa panel.
            if ctx.input(|i| i.key_pressed(egui::Key::N)) {
                if let Some(t) = self.tabs.get_mut(self.current) {
                    if !t.doc.hits.is_empty() {
                        let n = t.doc.hits.len();
                        let c = t.current_hit.unwrap_or(0);
                        if shift {
                            t.jump_to_hit(c.saturating_sub(1).min(n - 1));
                        } else {
                            t.jump_to_hit((c + 1).min(n - 1));
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn goto_execute(&mut self, tab_idx: usize) {
        let input = self.tabs[tab_idx].goto_input.clone();
        let all = self.goto_all && self.tabs.len() > 1;
        // Terapkan ke tab kini + (opsional) semua tab lain untuk korelasi.
        let targets: Vec<usize> = if all {
            (0..self.tabs.len()).collect()
        } else {
            vec![tab_idx]
        };
        let mut ok = 0;
        let mut fail = 0;
        for i in targets {
            let tab = &mut self.tabs[i];
            let r: Result<(), String> = match parse_goto(&input) {
                Ok(GotoTarget::Line(n)) => {
                    let max = if tab.doc.index.complete {
                        tab.doc.index.total_lines
                    } else {
                        tab.doc.line_count_estimate()
                    };
                    // `akhir` dikirim sebagai u64::MAX: selesaikan ke baris terakhir.
                    let n = if n == u64::MAX { max.max(1) } else { n };
                    if n > max && tab.doc.index.complete {
                        Err(format!("Baris melebihi {} baris.", format_count(max)))
                    } else {
                        tab.nav_to(n);
                        Ok(())
                    }
                }
                Ok(GotoTarget::Percent(p)) => {
                    let ln = tab.doc.goto_percent_line(p);
                    tab.nav_to(ln);
                    Ok(())
                }
                Ok(GotoTarget::Timestamp(ts)) => match goto_timestamp(tab, &ts) {
                    Ok(ln) => {
                        tab.nav_to(ln);
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            match r {
                Ok(()) => {
                    tab.goto_msg.clear();
                    tab.goto_open = false;
                    ok += 1;
                }
                Err(e) => {
                    // Pada mode semua-tab, tab gagal tetap lanjut; pesan di tab kini.
                    tab.goto_open = !all;
                    if i == tab_idx {
                        tab.goto_msg = e;
                    }
                    fail += 1;
                }
            }
        }
        if all {
            self.global_status = format!("Lompat ke semua tab: {} ok, {} gagal.", ok, fail);
        }
    }
}

/// Timestamp jump: binary search bila monotonik, else linear dengan timeout.
pub(crate) fn goto_timestamp(tab: &mut TabState, ts: &str) -> Result<u64, String> {
    // Parse target as unix seconds; unparseable prefix falls back to linear.
    let target = Doc::parse_timestamp_prefix(ts);
    let target = match target {
        Some(t) => t,
        None => {
            // Fallback: cari substring linear (timeout 3 dtk).
            return goto_substring_linear(tab, ts);
        }
    };
    let total = if tab.doc.index.complete {
        tab.doc.index.total_lines
    } else {
        tab.doc.line_count_estimate().min(2_000_000)
    };
    if total == 0 {
        return Err(String::from("File kosong."));
    }
    // Sample first/last parsed times.
    let first = (1..=total.min(200))
        .filter_map(|ln| tab.doc.get_line_text(ln))
        .filter_map(|s| Doc::parse_timestamp_prefix(&s))
        .next();
    let last = (total.saturating_sub(200).max(1)..=total)
        .rev()
        .filter_map(|ln| tab.doc.get_line_text(ln))
        .filter_map(|s| Doc::parse_timestamp_prefix(&s))
        .next();
    let monotonic = match (first, last) {
        (Some(a), Some(b)) => a <= b,
        _ => false,
    };
    if monotonic {
        // binary search lines
        let mut lo = 1u64;
        let mut hi = total;
        let mut best: Option<u64> = None;
        let start = Instant::now();
        while lo <= hi {
            if start.elapsed() > Duration::from_secs(5) {
                break;
            }
            let mid = lo + (hi - lo) / 2;
            let t = tab
                .doc
                .get_line_text(mid)
                .and_then(|s| Doc::parse_timestamp_prefix(&s));
            match t {
                Some(v) if v < target => lo = mid + 1,
                Some(v) if v > target => {
                    best = Some(mid);
                    hi = mid.saturating_sub(1);
                    if hi == 0 {
                        break;
                    }
                }
                Some(_) => return Ok(mid),
                None => {
                    // baris tanpa cap waktu: cari tetangga terdekat (langkah kecil)
                    // Sederhana: geser lo maju.
                    lo = mid + 1;
                    if lo > total {
                        break;
                    }
                }
            }
        }
        best.ok_or_else(|| String::from("Cap waktu tidak ditemukan."))
    } else {
        // linear dengan timeout
        let start = Instant::now();
        let mut ln = 1u64;
        while ln <= total {
            if start.elapsed() > Duration::from_secs(3) {
                return Err(String::from(
                    "Pencarian waktu kehabisan waktu — hasil sebagian tidak ditemukan.",
                ));
            }
            if let Some(s) = tab.doc.get_line_text(ln) {
                if let Some(v) = Doc::parse_timestamp_prefix(&s) {
                    if v >= target {
                        return Ok(ln);
                    }
                }
            }
            ln += 1;
            if ln > 500_000 {
                break;
            }
        }
        Err(String::from("Cap waktu tidak ditemukan."))
    }
}

pub(crate) fn goto_substring_linear(tab: &mut TabState, needle: &str) -> Result<u64, String> {
    let start = Instant::now();
    let total = if tab.doc.index.complete {
        tab.doc.index.total_lines.min(1_000_000)
    } else {
        200_000
    };
    for ln in 1..=total {
        if start.elapsed() > Duration::from_secs(3) {
            return Err(String::from("Pencarian kehabisan waktu."));
        }
        if let Some(s) = tab.doc.get_line_text(ln) {
            if s.contains(needle) {
                return Ok(ln);
            }
        }
    }
    Err(String::from("Tidak ditemukan."))
}

/// Selesaikan blok pada baris aktif: 0 = SQL, 1 = transaksi, 2 = checkpoint.
pub(crate) fn resolve_block(tab: &mut TabState, which: u8) -> Result<(u64, u64, String), String> {
    match which {
        0 => tab
            .doc
            .sql_block_range(tab.selected_line)
            .map(|(a, b)| {
                (
                    a,
                    b,
                    format!("Blok SQL baris {}–{}", format_count(a), format_count(b)),
                )
            }),
        1 => tab
            .doc
            .block_range(tab.selected_line, BlockKind::Transaction),
        _ => tab
            .doc
            .block_range(tab.selected_line, BlockKind::Checkpoint),
    }
}

pub(crate) fn copy_block(tab: &mut TabState, ctx: &egui::Context, which: u8) {
    match resolve_block(tab, which) {
        Ok((a, b, desc)) => match tab.doc.copy_range_text(a, b) {
            Ok(s) => {
                ctx.copy_text(s);
                tab.doc.status = format!("{} disalin.", desc);
            }
            Err(e) => tab.doc.status = e,
        },
        Err(e) => tab.doc.status = e,
    }
}

pub(crate) fn export_block(tab: &mut TabState, which: u8, fname: &str) {    let (a, b, desc) = match resolve_block(tab, which) {
        Ok(v) => v,
        Err(e) => {
            tab.doc.status = e;
            return;
        }
    };
    let Some(p) = rfd::FileDialog::new().set_file_name(fname).save_file() else {
        return;
    };
    match tab.doc.export_range_to_file(&p, a, b) {
        Ok(n) => {
            tab.doc.status = format!("{} diekspor ({} baris) ke {}.", desc, n, p.display())
        }
        Err(e) => tab.doc.status = e,
    }
}

/// Salin N baris dari baris aktif dengan nomor baris ("ln: teks").
pub(crate) fn copy_numbered(tab: &mut TabState, ctx: &egui::Context, count: u64) {
    let a = tab.selected_line;
    let mut out = String::new();
    let mut over = false;
    for ln in a..a.saturating_add(count.max(1)) {
        let Some(t) = tab.doc.get_line_text(ln) else { break };
        let row = format!("{}: {}\n", ln, t);
        if out.len() + row.len() > crate::engine::COPY_CAP_BYTES {
            over = true;
            break;
        }
        out.push_str(&row);
    }
    if out.is_empty() {
        tab.doc.status = String::from("Tidak ada baris untuk disalin.");
    } else {
        ctx.copy_text(out);
        tab.doc.status = if over {
            String::from("Disalin sebagian (16 MB).")
        } else {
            String::from("Disalin dengan nomor baris.")
        };
    }
}

/// Unduh URL ke temp di thread latar (lapor via channel).
pub(crate) fn spawn_download(url: String, tx: mpsc::Sender<DlMsg>) {
    std::thread::spawn(move || {
        let t = url.trim();
        if !(t.starts_with("http://") || t.starts_with("https://")) {
            let _ = tx.send(DlMsg::Failed(String::from("URL harus http:// atau https://.")));
            return;
        }
        let leaf = t
            .rsplit(['/', '?'])
            .next()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        let leaf: String = leaf
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .take(80)
            .collect();
        let leaf = if leaf.is_empty() { String::from("unduhan.log") } else { leaf };
        let mut dir = std::env::temp_dir();
        dir.push("asislog-dl");
        if std::fs::create_dir_all(&dir).is_err() {
            let _ = tx.send(DlMsg::Failed(String::from("Gagal membuat direktori temp.")));
            return;
        }
        let out = dir.join(leaf);
        let resp = match ureq::get(t).call() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(DlMsg::Failed(format!("Gagal mengunduh: {}", e)));
                return;
            }
        };
        let mut body = resp.into_body().into_reader();
        let mut f = match std::fs::File::create(&out) {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(DlMsg::Failed(format!("Gagal menulis temp: {}", e)));
                return;
            }
        };
        match std::io::copy(&mut body, &mut f) {
            Ok(_) => {
                let _ = tx.send(DlMsg::Done(out));
            }
            Err(e) => {
                let _ = tx.send(DlMsg::Failed(format!("Gagal mengunduh: {}", e)));
            }
        }
    });
}

/// Warna dot penanda, sadar-tema.
pub(crate) fn mark_color(c: BookmarkColor, dark: bool) -> egui::Color32 {
    // Varian terang digelapkan agar terbaca di latar terang.
    match c {
        BookmarkColor::Default => viewer::gutter_color(dark),
        BookmarkColor::Blue => {
            if dark {
                egui::Color32::from_rgb(90, 160, 255)
            } else {
                egui::Color32::from_rgb(30, 90, 180)
            }
        }
        BookmarkColor::Green => {
            if dark {
                egui::Color32::from_rgb(90, 220, 120)
            } else {
                egui::Color32::from_rgb(20, 130, 40)
            }
        }
        BookmarkColor::Yellow => {
            if dark {
                egui::Color32::from_rgb(255, 200, 60)
            } else {
                egui::Color32::from_rgb(150, 110, 0)
            }
        }
        BookmarkColor::Red => {
            if dark {
                egui::Color32::from_rgb(255, 110, 110)
            } else {
                egui::Color32::from_rgb(180, 30, 30)
            }
        }
        BookmarkColor::Purple => {
            if dark {
                egui::Color32::from_rgb(200, 150, 255)
            } else {
                egui::Color32::from_rgb(110, 60, 170)
            }
        }
    }
}

pub(crate) fn highlight_keys() -> &'static [(&'static str, &'static str)] {
    crate::store::highlight_color_names()
}
pub(crate) fn key_index(k: &str) -> usize {
    highlight_keys()
        .iter()
        .position(|(kk, _)| *kk == k)
        .unwrap_or(0)
}

pub(crate) fn color_name_id(key: &str) -> &'static str {
    highlight_keys()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, n)| *n)
        .unwrap_or("Kuning")
}

/// Language-aware display name for a highlight color key.
pub(crate) fn color_name_lang(key: &str, lang: crate::i18n::Lang) -> &'static str {
    crate::store::highlight_color_names_for(lang)
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, n)| *n)
        .unwrap_or(lang.tr("Kuning"))
}

/// Nama file aman dari nama set (ASCII saja, maks 40 char).
pub(crate) fn sanitize_name(s: &str) -> String {
    let out: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == ' ')
        .take(40)
        .collect();
    let out = out.trim().replace(' ', "-").to_lowercase();
    if out.is_empty() {
        String::from("highlight")
    } else {
        out
    }
}

/// Terapkan rentang waktu sebagai filter baris [lo, hi] (maks 2 juta baris).
/// Mengembalikan pesan status Indonesia. Baris tanpa timestamp dilewati
/// oleh pencarian biner/linear di `goto_timestamp`.
pub(crate) fn apply_time_range(tab: &mut TabState, start: &str, end: &str) -> Result<String, String> {
    let t0 = Doc::parse_timestamp_prefix(start.trim()).ok_or_else(|| {
        String::from("Waktu awal tidak valid. Contoh: 2026-08-24 13:00:00")
    })?;
    let t1 = Doc::parse_timestamp_prefix(end.trim())
        .ok_or_else(|| String::from("Waktu akhir tidak valid."))?;
    if t1 < t0 {
        return Err(String::from("Waktu akhir harus setelah waktu awal."));
    }
    let total = if tab.doc.index.complete {
        tab.doc.index.total_lines
    } else {
        tab.doc.line_count_estimate().min(2_000_000)
    };
    if total == 0 {
        return Err(String::from("File kosong."));
    }
    let lo = goto_timestamp(tab, start.trim())?;
    // Batas atas: baris pertama >= t1 (atau total bila tak ketemu).
    let mut hi = match goto_timestamp(tab, end.trim()) {
        Ok(l) => {
            // Samakan presisi: bila baris l tepat == t1, ikutkan; bila sudah
            // melewati t1, mundur satu baris.
            match tab
                .doc
                .get_line_text(l)
                .and_then(|s| Doc::parse_timestamp_prefix(&s))
            {
                Some(v) if v > t1 => l.saturating_sub(1).max(lo),
                _ => l,
            }
        }
        Err(_) => total,
    };
    hi = hi.min(total);
    if hi < lo {
        return Err(String::from("Rentang kosong pada file ini."));
    }
    const CAP: u64 = 2_000_000;
    let mut truncated = false;
    if hi - lo + 1 > CAP {
        hi = lo + CAP - 1;
        truncated = true;
    }
    tab.doc.filter_map = crate::engine::LineSet::from_lines(lo..=hi);
    tab.doc.filter_active = true;
    tab.doc.filter = parse_filter("", tab.case_sensitive);
    tab.doc.filter.raw = format!("waktu {} s.d. {}", start.trim(), end.trim());
    tab.top_row = 0;
    tab.nav_to(lo);
    Ok(format!(
        "Rentang waktu: {} baris{}.",
        format_count(hi - lo + 1),
        if truncated { " (dibatasi 2 jt)" } else { "" }
    ))
}

