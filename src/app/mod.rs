// English comments: AsisLog egui app (tabs, shortcuts, background jobs).
// Split from single app.rs god-object; behavior unchanged.
#![allow(unused_imports)]

use std::path::PathBuf;

use crate::engine::index::SparseIndex;
use crate::engine::search::Hit;

pub(crate) mod actions;
pub(crate) mod jobs_index;
pub(crate) mod jobs_search;
pub(crate) mod shortcuts;
pub(crate) mod state;
pub(crate) mod tab;
pub(crate) mod ui;
pub(crate) mod ui_analyze;
pub(crate) mod ui_chrome;
pub(crate) mod ui_dialogs;
pub(crate) mod ui_highlight;
pub(crate) mod ui_misc;
pub(crate) mod ui_histogram;
pub(crate) mod ui_palette;
pub(crate) mod ui_panels;
pub(crate) mod ui_search;
pub(crate) mod ui_tools;
pub(crate) mod ui_tools_investigation;
pub(crate) mod ui_viewport;
pub(crate) mod update;

// ---------- background messages ----------

#[derive(Clone, Debug)]
pub(crate) struct IndexUpdate {
    index: SparseIndex,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchBatchMsg {
    gen: u64,
    batch: Vec<Hit>,
    done: bool,
    truncated: bool,
    error: Option<String>,
    /// Bytes scanned so far (for progress: `Mencari… x / y`).
    scanned: u64,
    /// Total file bytes at search start.
    total: u64,
    /// Exact in-scope matches found by THIS job so far (cumulative).
    /// Workers keep counting past the display cap (count-only mode, no
    /// extra Hit storage), so a truncated "15.257.710 hasil" label is
    /// exact, not an estimate. Tail-refresh jobs count the tail only;
    /// the tab adds its merge base (see `merge_base`).
    grand_total: u64,
}

/// Gutter + text responses for one log row (for distinct click targets).
pub(crate) struct RowResp {
    g: egui::Response,
    t: egui::Response,
}

/// Hasil unduhan URL di thread latar.
pub(crate) enum DlMsg {
    Done(PathBuf),
    Failed(String),
}

/// Payload peta marker latar: (bucket_bits, file_size, time_hist).
pub(crate) type MarkerUpdate = (Vec<u8>, u64, Option<TimeHist>);

// Compatibility re-exports (paths used before the split).
pub use jobs_index::{build_time_hist, TimeHist, MARKER_BUCKETS};
pub use state::AsisLogApp;
pub use tab::ViewMode;
pub(crate) use actions::*;
pub(crate) use jobs_index::*;
pub(crate) use jobs_search::*;
pub(crate) use state::*;
pub(crate) use tab::*;
