// English comments: library root (engine testable without window).
pub mod app;
pub mod engine;
pub mod i18n;
pub mod store;
pub mod ui;

/// Feature log embedded at compile time (shown in the About dialog).
/// Embedded, not read at runtime: the portable binary stays self-contained.
pub const CHANGELOG_TEXT: &str = include_str!("../CHANGELOG.md");
