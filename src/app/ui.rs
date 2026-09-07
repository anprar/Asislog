// English comments: eframe::App impl + update orchestrator (split from app.rs).
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

impl eframe::App for AsisLogApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Tulis sesi + config saat keluar (best effort, tanpa panel).
        self.save_session();
        if self.cfg_dirty {
            self.save_config();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background(ctx);
        self.render_toolbar(ctx);
        if self.tabs.is_empty() {
            self.render_empty(ctx);
            ctx.request_repaint_after(Duration::from_millis(400));
            return;
        }
        let cur_idx = self.current.min(self.tabs.len() - 1);
        if !self.zen_mode {
            self.render_tools(ctx, cur_idx);
            self.render_search(ctx, cur_idx);
        } else if self.zen_search_open {
            self.render_zen_search(ctx, cur_idx);
        }
        self.render_panels(ctx, cur_idx);
        self.render_viewport(ctx, cur_idx);
        self.render_dialogs_main(ctx, cur_idx);
        self.render_highlight(ctx);
        self.render_misc(ctx, cur_idx);
        if self.hist_panel_open {
            self.render_histogram_panel(ctx, cur_idx);
        }
        if self.top_n_open {
            self.render_top_n_panel(ctx, cur_idx);
        }
        if self.hex_peek_open {
            self.render_hex_peek_panel(ctx, cur_idx);
        }
        if self.palette_open {
            self.render_palette(ctx);
        }
        // Repaint hanya saat ada yang bergerak: indeks/search/filter/marker
        // latar, debounce tertunda, follow aktif, unduhan, atau catatan
        // follow yang harus kedaluwarsa. Idle = tanpa repaint paksa
        // (egui tetap repaint saat ada input); jaring pengaman 2 dtk agar
        // tak ada indikator yang macet bila satu kasus terlewat.
        let busy = self.dl_rx.is_some()
            || self.tabs.iter().any(|t| {
                !t.doc.index.complete
                    || t.doc.search_in_progress
                    || t.debounce_at.is_some()
                    || t.doc.follow
                    || t.index_rx.is_some()
                    || t.search_rx.is_some()
                    || t.filter_rx.is_some()
                    || t.marker_rx.is_some()
                    || t.follow_note.is_some()
            });
        if busy {
            ctx.request_repaint_after(Duration::from_millis(120));
        } else {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
}
