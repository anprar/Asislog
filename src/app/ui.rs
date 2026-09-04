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
        self.render_tools(ctx, cur_idx);
        self.render_search(ctx, cur_idx);
        self.render_panels(ctx, cur_idx);
        self.render_viewport(ctx, cur_idx);
        self.render_dialogs_main(ctx, cur_idx);
        self.render_highlight(ctx);
        self.render_misc(ctx, cur_idx);
        ctx.request_repaint_after(Duration::from_millis(120));
    }
}
