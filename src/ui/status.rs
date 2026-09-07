// English comments: status bar formatting lives in app/ui_panels.rs
// (this module's legacy status_text helper was removed as dead code).

#[cfg(test)]
mod tests {
    use crate::engine::{format_count, format_size};

    #[test]
    fn status_number_formatting_smoke() {
        // Smoke: formatting helpers use dot thousands.
        assert_eq!(format_count(1_000_000), "1.000.000");
        assert!(format_size(1024).contains("KB"));
    }
}
