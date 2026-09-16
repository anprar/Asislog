// English comments: i18n Language switch (Indonesian / English), portable, no installer.
// UI default is Indonesian (backward compatible). English via toolbar switch,
// persisted in config.json as "id" / "en".
// Third+ locales arrive as EXTERNAL JSON overrides (path in config):
// {"Buka": "Open", ...} consulted before the built-in table, so a dozen
// locales ship without code changes. Values are leaked once (bounded).

use std::collections::HashMap;

/// External locale overrides: Indonesian source -> replacement text.
/// Applied to BOTH built-in languages (an override replaces the string).
static LOCALE_OVR: std::sync::LazyLock<
    std::sync::RwLock<std::collections::HashMap<String, &'static str>>,
> = std::sync::LazyLock::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));

fn locale_override(id: &str) -> Option<&'static str> {
    LOCALE_OVR.read().ok()?.get(id).copied()
}

/// Install/replace the whole override table (e.g. from a locale file).
/// Empty values are dropped (fall back to built-in).
pub fn set_locale_overrides(map: std::collections::HashMap<String, String>) {
    let mut ovr = HashMap::new();
    for (k, v) in map {
        if v.trim().is_empty() {
            continue;
        }
        ovr.insert(k, Box::leak(v.into_boxed_str()) as &'static str);
    }
    if let Ok(mut w) = LOCALE_OVR.write() {
        *w = ovr;
    }
}

/// Drop all overrides (back to built-in ID/EN).
pub fn clear_locale_overrides() {
    if let Ok(mut w) = LOCALE_OVR.write() {
        w.clear();
    }
}

/// Number of active overrides (shown in Options).
pub fn locale_override_count() -> usize {
    LOCALE_OVR.read().map(|r| r.len()).unwrap_or(0)
}

/// Load overrides from a JSON object file `{"source": "text", ...}`.
/// Returns the number of entries installed.
pub fn load_locale_file(path: &std::path::Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Gagal membaca locale: {}", e))?;
    let map: std::collections::HashMap<String, String> = serde_json::from_str(&text)
        .map_err(|_| String::from("Locale JSON tidak valid (perlu {\"sumber\": \"teks\"})."))?;
    let n = map.len();
    set_locale_overrides(map);
    Ok(n)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    Id,
    En,
}

impl Lang {
    pub fn from_key(s: &str) -> Lang {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" | "english" | "inggris" => Lang::En,
            _ => Lang::Id,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Lang::Id => "id",
            Lang::En => "en",
        }
    }

    /// Display name in its own language (for the combo box).
    pub fn label(self) -> &'static str {
        match self {
            Lang::Id => "Indonesia",
            Lang::En => "English",
        }
    }

    /// Best-effort OS locale for FIRST RUN only (no saved `lang` in config).
    /// Env-based, zero new dependencies (portable constraint): explicit
    /// `ASISLOG_LANG` wins, then `LANGUAGE` (colon-separated priority),
    /// `LC_ALL`, `LANG`, `LC_MESSAGES`. `en*` → En, `id*`/`ms*` → Id,
    /// `C`/`POSIX`/empty → keep looking, any other known locale → En
    /// (lingua franca beats an untranslated default), nothing found →
    /// Id (historical default).
    /// Known limitation: Windows display language is invisible without OS
    /// APIs (no std-only way); Windows users without these vars set keep
    /// the Indonesian default and switch once in the toolbar.
    pub fn detect_system_lang() -> Lang {
        Self::detect_from_env(&["ASISLOG_LANG", "LANGUAGE", "LC_ALL", "LANG", "LC_MESSAGES"])
    }

    fn detect_from_env(keys: &[&str]) -> Lang {
        for key in keys {
            let Ok(v) = std::env::var(key) else { continue };
            for part in v.split(':') {
                let tag = part.trim().to_ascii_lowercase();
                let lang = tag.split(['_', '.', '@']).next().unwrap_or("");
                match lang {
                    "en" => return Lang::En,
                    "id" | "ms" => return Lang::Id,
                    "" | "c" | "posix" => continue,
                    _ => return Lang::En,
                }
            }
        }
        Lang::Id
    }

    /// Name for an auto-created quick-label highlight set.
    /// Stored in config as ordinary data; old configs may hold either
    /// language's name, so callers must accept both (see toggle_label).
    pub fn quick_set_name(self) -> &'static str {
        match self {
            Lang::Id => "Cepat",
            Lang::En => "Quick",
        }
    }

    /// Display name for a stored monospace-font choice.
    /// Storage stays stable (`Bawaan` from old configs); only display
    /// is localized, in both directions.
    pub fn font_name(self, stored: &str) -> &str {
        match (self, stored) {
            (Lang::En, "Bawaan") => "Default",
            (Lang::Id, "Default") => "Bawaan",
            _ => stored,
        }
    }

    /// Format a translated template with `{}` placeholders.
    /// Use instead of `format!(lang.tr(..), ..)` (which Rust rejects:
    /// format string must be a literal). Replaces `{}` left-to-right.
    pub fn f1(self, id: &str, a: impl ToString) -> String {
        self.tr(id).replacen("{}", &a.to_string(), 1)
    }
    pub fn f2(self, id: &str, a: impl ToString, b: impl ToString) -> String {
        let t = self.tr(id).replacen("{}", &a.to_string(), 1);
        t.replacen("{}", &b.to_string(), 1)
    }
    pub fn f3(self, id: &str, a: impl ToString, b: impl ToString, c: impl ToString) -> String {
        let t = self.tr(id).replacen("{}", &a.to_string(), 1);
        let t = t.replacen("{}", &b.to_string(), 1);
        t.replacen("{}", &c.to_string(), 1)
    }
    pub fn f4(
        self,
        id: &str,
        a: impl ToString,
        b: impl ToString,
        c: impl ToString,
        d: impl ToString,
    ) -> String {
        let t = self.tr(id).replacen("{}", &a.to_string(), 1);
        let t = t.replacen("{}", &b.to_string(), 1);
        let t = t.replacen("{}", &c.to_string(), 1);
        t.replacen("{}", &d.to_string(), 1)
    }

    /// Translate an Indonesian source string (or template with `{}`/`{:...}`).
    /// Unknown keys fall back to the Indonesian source (forward compatible).
    /// External locale overrides win over everything (both languages).
    pub fn tr(self, id: &str) -> &str {
        if let Some(s) = locale_override(id) {
            return s;
        }
        if self == Lang::Id {
            return id;
        }
        match id {
            // ---- toolbar / chrome ----
            "Buka" => "Open",
            "Arsip" => "Archive",
            "Semua" => "All",
            "Riwayat" => "History",
            "Favorit:" => "Favorites:",
            "Terakhir dibuka:" => "Recent:",
            "Belum ada riwayat." => "No history yet.",
            "Belum ada file. Seret .log / .txt ke sini atau tekan Buka." => {
                "No file yet. Drag .log / .txt here or press Open."
            }
            "Lepas favorit" => "Remove favorite",
            "Jadikan favorit" => "Make favorite",
            "Bersihkan riwayat" => "Clear history",
            "Bersihkan riwayat?" => "Clear history?",
            "URL/teks" => "URL/text",
            "Buka URL…" => "Open URL…",
            "Tempel teks…" => "Paste text…",
            "Workspace" => "Workspace",
            "Simpan workspace…" => "Save workspace…",
            "Buka workspace…" => "Open workspace…",
            "Ganti tab" => "Switch tab",
            "Keluar Layar Penuh (F11)" => "Exit Full Screen (F11)",
            "Kembalikan bilah kontrol lengkap (F11)" => "Restore full toolbar (F11)",
            "Buka HUD pencarian melayang" => "Open floating search HUD",
            "Unduh http(s) ke temp lalu buka" => "Download http(s) to temp then open",
            "Tempel teks (Ctrl+V) lalu buka sebagai file" => "Paste text (Ctrl+V) then open as file",
            "Simpan tab + filter + set sorotan ke 1 file JSON" => {
                "Save tabs + filter + highlight set to 1 JSON file"
            }
            "Buka N log + filter + set sorotan dari file" => {
                "Open N logs + filter + highlight set from file"
            }
            "Daftar pintasan (F1)" => "Shortcut list (F1)",
            "Layar Penuh: sembunyikan 5 baris kontrol ke 1 baris ramping (F11)" => {
                "Full screen: collapse 5 control rows into 1 slim row (F11)"
            }
            "Cari (Ctrl+F)" => "Search (Ctrl+F)",
            "Palet (Ctrl+Shift+P)" => "Palette (Ctrl+Shift+P)",
            "Palet" => "Palette",
            "Layar Penuh (F11)" => "Full Screen (F11)",
            "Tema" => "Theme",
            "Tema:" => "Theme:",
            "Font Log:" => "Log Font:",
            "Font UI:" => "UI Font:",
            "Bahasa" => "Language",
            "Bahasa:" => "Language:",
            "Bahasa/Language" => "Language",
            "Tab: {}" => "Tab: {}",
            "File favorit tak ditemukan: {}" => "Favorite file not found: {}",
            "File tidak ditemukan, dihapus dari riwayat: {}" => {
                "File not found, removed from history: {}"
            }
            "Seluruh riwayat file dibuka akan dikosongkan." => {
                "All opened-file history will be cleared."
            }
            "Riwayat file dikosongkan." => "File history cleared.",
            "File tidak ditemukan: {}" => "File not found: {}",
            "Membuka {}." => "Opening {}.",
            "Siap" => "Ready",
            "Siap. Buka file log untuk mulai." => "Ready. Open a log file to start.",
            // ---- empty state ----
            "Buka file log…" => "Open log file…",
            "Buka file log" => "Open log file",
            "Penampil portabel untuk file .log / .txt / .out yang sangat besar." => {
                "Portable viewer for very large .log / .txt / .out files."
            }
            // ---- tools bar ----
            "Ke baris…" => "Go to line…",
            "Ke nomor baris, persen, akhir, atau cap waktu (Ctrl+G)" => {
                "Go to line number, percent, end, or timestamp (Ctrl+G)"
            }
            "Kembali ke lokasi sebelumnya (Alt+Left)" => "Back to previous location (Alt+Left)",
            "Maju ke lokasi berikutnya (Alt+Right)" => "Forward to next location (Alt+Right)",
            "Tidak ada lokasi sebelumnya." => "No previous location.",
            "Tidak ada lokasi berikutnya." => "No next location.",
            "Penanda" => "Bookmarks",
            "Ekspor hasil…" => "Export results…",
            "Simpan hasil pencarian ke file baru" => "Save search results to a new file",
            "Sorotan…" => "Highlights…",
            "Aturan highlight kustom (hanya viewport)" => "Custom highlight rules (viewport only)",
            "Catatan" => "Notes",
            "Scratchpad: catatan + base64/JWT/JSON/SQL" => "Scratchpad: notes + base64/JWT/JSON/SQL",
            "LIVE" => "LIVE",
            "LIVE jeda" => "LIVE paused",
            "Ikuti akhir file" => "Follow tail",
            "Mengikuti ekor file (klik untuk berhenti)" => "Following file tail (click to stop)",
            "Terjeda karena menggulir ke atas (klik untuk kembali ke ekor)" => {
                "Paused because you scrolled up (click to jump back to tail)"
            }
            "Pantau akhir file / tail (Ctrl+Shift+F)" => "Watch file tail (Ctrl+Shift+F)",
            "Otomatis" => "Automatic",
            "Encoding file (Otomatis = deteksi BOM + sampel)" => {
                "File encoding (Automatic = BOM + sample detection)"
            }
            // ---- search ----
            "Preset" => "Presets",
            "Bawaan:" => "Built-in:",
            "Belum ada simpanan." => "No saved presets.",
            "Hapus preset" => "Delete preset",
            "Simpan pencarian saat ini…" => "Save current search…",
            "Cari teks, exception, request ID, atau regex…" => {
                "Search text, exception, request ID, or regex…"
            }
            "Peka huruf besar/kecil (Alt+C)" => "Case sensitive (Alt+C)",
            "Perlakukan query sebagai regex (Alt+R)" => "Treat query as regex (Alt+R)",
            "Peka huruf besar/kecil" => "Case sensitive",
            "Perlakukan query sebagai regex" => "Treat query as regex",
            "Mode regex" => "Regex mode",
            "Keluar aplikasi" => "Quit application",
            "Muat ulang file" => "Reload file",
            "Awal file (tanpa Ctrl)" => "Start of file (no Ctrl)",
            "Akhir file (tanpa Ctrl)" => "End of file (no Ctrl)",
            "Satu layar ke bawah (Spasi)" => "One screen down (Space)",
            "Satu layar ke atas (Shift+Spasi)" => "One screen up (Shift+Space)",
            "Tab berikut (Ctrl+PgDn)" => "Next tab (Ctrl+PgDn)",
            "Tab sebelum (Ctrl+PgUp)" => "Previous tab (Ctrl+PgUp)",
            "Buka file log dulu — belum ada tab terbuka." => {
                "Open a log file first — no tab open yet."
            }
            "Dimuat ulang: {}." => "Reloaded: {}.",
            "Bersihkan pencarian (Esc)" => "Clear search (Esc)",
            "Hasil sebelumnya (Shift+F3)" => "Previous result (Shift+F3)",
            "Hasil berikutnya (F3)" => "Next result (F3)",
            "Mencari… {} / {} · {} hasil" => "Searching… {} / {} · {} results",
            "Mencari… {} hasil" => "Searching… {} results",
            "Tidak ada kecocokan \"{}\"" => "No matches for \"{}\"",
            "{} hasil ({})" => "{} results ({})",
            "(dibatasi 200 rb)" => "(limited to 200k)",
            "(dibatasi {})" => "(limited {})",
            "Muat {} berikutnya" => "Load next {}",
            "Pindai {} hasil berikutnya (total eksak {} — tanpa batas RAM)" => {
                "Scan the next {} results ({} exact total — memory-bounded)"
            },
            "{} baris ({} rentang)" => "{} lines ({} ranges)",
            "Sorot" => "Highlight",
            "Saring" => "Filter",
            "Tampilkan semua baris, sorot kecocokan (F3 untuk lompat)" => {
                "Show all lines, highlight matches (F3 to jump)"
            }
            "Tampilkan HANYA baris yang cocok dengan pencarian" => {
                "Show ONLY lines matching the search"
            }
            "Jadikan filter" => "Make filter",
            "Konversi query pencarian ke filter permanen" => {
                "Convert search query to a permanent filter"
            }
            "Pencarian regex tidak dapat dijadikan filter token." => {
                "Regex search cannot be converted to a token filter."
            }
            "Filter diterapkan: {}" => "Filter applied: {}",
            "Gagal konversi ke filter: {}" => "Failed to convert to filter: {}",
            "Query pencarian tidak valid: {}" => "Invalid search query: {}",
            "Batalkan" => "Cancel",
            "Hentikan pindaian yang berjalan (Esc)" => "Stop the running scan (Esc)",
            "Pencarian dibatalkan." => "Search cancelled.",
            "Pencarian dibatalkan (Esc)." => "Search cancelled (Esc).",
            "Cakupan: {}-{}" => "Scope: {}-{}",
            "Hapus cakupan" => "Clear scope",
            "Logika:" => "Logic:",
            "Hapus \"{}\" dari query" => "Remove \"{}\" from query",
            "Ekspresi kompleks (OR/kurung) dievaluasi penuh." => {
                "Complex expression (OR/parens) fully evaluated."
            }
            "Riwayat:" => "History:",
            "Filter" => "Filter",
            "Terapkan" => "Apply",
            "Bersihkan" => "Clear",
            "Ke semua tab" => "To all tabs",
            "Terapkan filter ini ke semua tab (korelasi)" => {
                "Apply this filter to all tabs (correlate)"
            }
            "Tampil: {}" => "Show: {}",
            "Semua baris (atau hasil filter)" => "All lines (or filter results)",
            "Hanya baris hasil pencarian" => "Only search result lines",
            "Hanya baris penanda" => "Only bookmark lines",
            "Mode tampil viewport" => "Viewport display mode",
            "Cakupan…" => "Scope…",
            "Batasi pencarian ke rentang baris (hemat untuk file besar)" => {
                "Limit search to a line range (saves time on huge files)"
            }
            "Rentang waktu…" => "Time range…",
            "Tampil" => "Show",
            "Filter menyembunyikan baris yang tidak cocok.\n\
                         Token dipisah spasi; semua token inclusions harus ada (AND).\n\
                         Awalan - berarti kecualikan.\n\
                         key=value cocokkan field baris JSON (mis. level=ERROR).\n\n\
                         Contoh:\n  ERROR            hanya baris error\n  ERROR -DEBUG     error tanpa debug\n  level=ERROR      field JSON level\n  OrderService     teks spesifik" => {
                "Filter hides non-matching lines.\n\
                 Tokens are space-separated; all inclusion tokens must match (AND).\n\
                 The - prefix excludes.\n\
                 key=value matches a JSON line field (e.g. level=ERROR).\n\n\
                 Examples:\n  ERROR            error lines only\n  ERROR -DEBUG     errors without debug\n  level=ERROR      JSON field level\n  OrderService     specific text"
            }
            "Tampilkan hanya baris dalam rentang cap waktu" => {
                "Show only lines within a timestamp range"
            }
            "Filter: pisahkan token dengan spasi, awalan - mengecualikan. Contoh: ERROR -DEBUG" => {
                "Filter: separate tokens with spaces, - prefix excludes. Example: ERROR -DEBUG"
            }
            "Filter aktif: {} · {}/{} baris" => "Active filter: {} · {}/{} lines",
            "Hapus" => "Delete",
            "Pencarian (Layar Penuh)" => "Search (Full Screen)",
            "Cari ..." => "Search ...",
            "Huruf besar/kecil" => "Case",
            "Regex" => "Regex",
            "Tutup (Esc)" => "Close (Esc)",
            "… hasil" => "… results",
            "{} hasil" => "{} results",
            "Prev" => "Prev",
            "Next" => "Next",
            // ---- panels / status ----
            "LIVE dijeda - kembali ke akhir untuk melanjutkan" => {
                "LIVE paused - go back to the end to resume"
            }
            "memantau tiap 500 ms" => "watching every 500 ms",
            "Mengindeks {}% - {} baris terdeteksi" => "Indexing {}% - {} lines found",
            "File: {}" => "File: {}",
            "{} baris" => "{} lines",
            "~{} baris" => "~{} lines",
            "Cari: {} hasil" => "Search: {} results",
            "Filter: aktif ({})" => "Filter: on ({})",
            "Filter: mati" => "Filter: off",
            "Pos: baris {} ({}{}%)" => "Pos: line {} ({}{}%)",
            "Hasil ({})" => "Results ({})",
            "Bagi" => "Split",
            "Dual-pane: panel hasil selalu terbuka lebar" => "Split view: results pane stays wide open",
            "Simpan hasil" => "Keep results",
            "Bekukan hasil ini sebagai snapshot" => "Pin these results as a snapshot",
            "Tidak ada hasil untuk disimpan." => "No results to keep.",
            "Live" => "Live",
            "Kembali ke hasil live" => "Back to live results",
            "Hapus snapshot ini" => "Delete this snapshot",
            "Panel belah (dual-pane hasil)" => "Split panel (dual-pane results)",
            "Bantuan / daftar pintasan" => "Help / shortcut list",
            "Layar Penuh" => "Full Screen",
            "Hasil berikutnya" => "Next result",
            "Hasil sebelumnya" => "Previous result",
            "Hasil berikut/plin (gaya vi)" => "Next/prev result (vi style)",
            "Awal file" => "Start of file",
            "Akhir file" => "End of file",
            "History mundur" => "History back",
            "History maju" => "History forward",
            "Penanda sebelumnya" => "Previous bookmark",
            "Penanda berikutnya" => "Next bookmark",
            "Tab sebelumnya" => "Previous tab",
            "Perkecil" => "Zoom out",
            "Reset zoom" => "Reset zoom",
            "Tekan tombol… (Esc batal)" => "Press a key… (Esc cancels)",
            "Reset" => "Reset",
            "Pintasan {} sudah dipakai oleh {}." => "Shortcut {} is already used by {}.",
            "Esc, 1–9, dan navigasi viewport tetap (tidak dapat diubah)." => "Esc, 1–9, and viewport navigation stay fixed.",
            "dipilih #{}/{}" => "selected #{}/{}",
            "Bersihkan pencarian dan hentikan worker" => "Clear search and stop worker",
            "Tampilkan panel hasil" => "Show results panel",
            "Ciutkan panel hasil" => "Collapse results panel",
            "Tampilkan ±20 baris" => "Show ±20 lines",
            "Salin hasil" => "Copy results",
            "Salin semua hasil (maks 16 MB) ke papan klip" => {
                "Copy all results (max 16 MB) to clipboard"
            }
            "Tidak ada hasil untuk disalin." => "No results to copy.",
            "Hasil disalin sebagian (16 MB). Gunakan Ekspor untuk sisanya." => {
                "Results partially copied (16 MB). Use Export for the rest."
            }
            "Hasil disalin ke papan klip." => "Results copied to clipboard.",
            "Belum ada hasil. Ketik kata kunci di kolom Cari." => {
                "No results yet. Type a keyword in the Search box."
            }
            // ---- bookmarks panel ----
            "Ctrl+B tandai - F2 ubah label - Alt+Atas/Bawah pindah" => {
                "Ctrl+B mark - F2 rename - Alt+Up/Down jump"
            }
            "Saring penanda…" => "Filter bookmarks…",
            "Belum ada. Klik nomor baris / Ctrl+B untuk menandai." => {
                "None yet. Click a line number / Ctrl+B to mark."
            }
            "Lompat ke baris {}" => "Jump to line {}",
            "Ubah" => "Edit",
            "Ubah label (F2)" => "Edit label (F2)",
            "Hapus penanda?" => "Delete bookmark?",
            "Penanda baris {} akan dihapus permanen (tak bisa dibatalkan)." => {
                "Bookmark on line {} will be permanently deleted (cannot be undone)."
            }
            "Penanda baris {} dihapus." => "Bookmark on line {} deleted.",
            "Penanda baris {} diperbarui." => "Bookmark on line {} updated.",
            "Penanda ditambahkan di baris {}." => "Bookmark added on line {}.",
            // ---- dialogs ----
            "Ke baris / persen / waktu (Ctrl+G)" => "Go to line / percent / time (Ctrl+G)",
            "Contoh: 38166903 · 50% · akhir · 2026-09-03 13:41:02" => {
                "Example: 38166903 · 50% · end · 2026-09-03 13:41:02"
            }
            "Semua tab (korelasi waktu/baris)" => "All tabs (time/line correlate)",
            "Pergi" => "Go",
            "Batal" => "Cancel",
            "Simpan hasil ke file…" => "Save results to file…",
            "{} hasil." => "{} results.",
            "Konteks (baris sekitar):" => "Context (surrounding lines):",
            "Ekspor berjalan di latar; dialog boleh ditutup." => {
                "Export runs in background; dialog may be closed."
            }
            "Batalkan ekspor" => "Cancel export",
            "Hanya hasil" => "Results only",
            "Hasil + konteks" => "Results + context",
            "Tiket Markdown (Jira)" => "Markdown ticket (Jira)",
            "Hasil + konteks sebagai Markdown siap paste" => {
                "Results + context as paste-ready Markdown"
            }
            "Ekspor dibatalkan." => "Export cancelled.",
            "Ekspor SEMUA cocok (streaming)" => "Export ALL matches (streaming)",
            "Tanpa batas tampil — tulis langsung ke disk." => {
                "No display cap — writes straight to disk."
            }
            "Tampil dipangkas — ekspor biasa ikut terpangkas." => {
                "Display is truncated — plain export is truncated too."
            }
            "Mengekspor streaming di latar…" => "Streaming export in background…",
            "Query kosong — isi kolom Cari dulu." => "Empty query — fill the Search field first.",
            "Pembaruan & crash" => "Updates & crashes",
            "URL cek versi (kosong = mati):" => "Version check URL (empty = off):",
            "File teks polos berisi versi terbaru, mis. 0.2.0." => {
                "Plain text file holding the newest version, e.g. 0.2.0."
            }
            "Cek otomatis saat start" => "Check automatically at startup",
            "Periksa sekarang" => "Check now",
            "Pengecekan versi berjalan…" => "Version check running…",
            "Masih berjalan — tunggu selesai." => "Still running — please wait.",
            "Versi baru tersedia: {} (kini {})." => "New version available: {} (current {}).",
            "Sudah versi terbaru ({})." => "Already newest ({}).",
            "Gagal cek versi: {}" => "Version check failed: {}",
            "Locale eksternal: {} override." => "External locale: {} overrides.",
            "Locale eksternal (JSON):" => "External locale (JSON):",
            "Muat file…" => "Load file…",
            "Gagal muat locale: {}" => "Failed to load locale: {}",
            "Laporan crash ditemukan" => "Crash reports found",
            "AsisLog pernah crash. Isi membantu diagnosis; kirim ke pengelola bila perlu." => {
                "AsisLog crashed before. Contents help diagnosis; send to the maintainer if needed."
            }
            "Salin isi" => "Copy contents",
            "Buka folder crash" => "Open crash folder",
            "Hapus laporan" => "Delete reports",
            "Nanti" => "Later",
            "Crash disalin." => "Crash copied.",
            "Laporan crash dihapus." => "Crash reports deleted.",
            "Cakupan pencarian (baris)" => "Search scope (lines)",
            "Cari hanya dalam rentang baris ini. Hemat untuk file besar." => {
                "Search only within this line range. Saves time on huge files."
            }
            "Dari" => "From",
            "Sampai" => "To",
            "Cakupan {}-{} aktif; ketik query untuk mencari." => {
                "Scope {}-{} active; type a query to search."
            }
            "Rentang tidak valid. Contoh: 1000000 sampai 2000000." => {
                "Invalid range. Example: 1000000 to 2000000."
            }
            "Rentang baris tidak valid." => "Invalid line range.",
            "Rentang kosong pada file ini." => "Empty range in this file.",
            "Waktu awal tidak valid. Contoh: 2026-08-24 13:00:00" => {
                "Invalid start time. Example: 2026-08-24 13:00:00"
            }
            "Waktu akhir tidak valid." => "Invalid end time.",
            "Waktu akhir harus setelah waktu awal." => "End time must be after start time.",
            "Tidak ditemukan." => "Not found.",
            "Indeks sisi dimuat." => "Sidecar index loaded.",
            "Zip kosong." => "Empty zip.",
            "Tar kosong / tanpa file teks." => "Empty tar / no text files.",
            "Entri tar tidak ditemukan." => "Tar entry not found.",
            "7z kosong / tanpa file teks." => "Empty 7z / no text files.",
            "7z butuh kata sandi (tidak didukung)." => "7z needs a password (unsupported).",
            "Filter dikosongkan — menampilkan semua baris." => {
                "Filter cleared — showing all lines."
            }
            "Filter: memindai baris baru…" => "Filter: scanning new lines…",
            "Ekspor masih berjalan; tunggu selesai atau Batalkan." => {
                "Export still running; wait or Cancel."
            }
            "Memfilter…" => "Filtering…",
            "Membuka… mengindeks latar." => "Opening… indexing in background.",
            " (dibatasi 2 jt)" => " (capped at 2M)",
            "Persen harus 0–100." => "Percent must be 0–100.",
            "Nomor baris minimal 1." => "Line number must be at least 1.",
            "Tiket disimpan ({} hasil): {}" => "Ticket saved ({} results): {}",
            "Kunci {}: {}" => "Key {}: {}",
            "Impor" => "Import",
            "Simpan pencarian sebagai preset" => "Save search as preset",
            "Nama preset:" => "Preset name:",
            "Simpan" => "Save",
            "Nama preset tidak boleh kosong." => "Preset name cannot be empty.",
            // ---- misc dialogs ----
            "Ubah label penanda" => "Edit bookmark label",
            "Baris {}" => "Line {}",
            "Label:" => "Label:",
            "Warna" => "Color",
            "Tampilkan rentang waktu" => "Show time range",
            "Contoh: 2026-08-24 13:00:00 sampai 2026-08-24 14:00:00" => {
                "Example: 2026-08-24 13:00:00 to 2026-08-24 14:00:00"
            }
            "Tampilkan rentang" => "Show range",
            "Buka dari URL" => "Open from URL",
            "Contoh: https://server/app.log" => "Example: https://server/app.log",
            "Unduh & buka" => "Download & open",
            "URL kosong." => "URL is empty.",
            "Mengunduh…" => "Downloading…",
            "Tempel teks sebagai file" => "Paste text as file",
            "Tempel (Ctrl+V), lalu buka sebagai file temp." => {
                "Paste (Ctrl+V), then open as a temp file."
            }
            "Buka sebagai file" => "Open as file",
            "Teks kosong." => "Text is empty.",
            "Berkas versi kosong." => "Version file is empty.",
            "Gagal menulis temp: {}" => "Failed to write temp: {}",
            "Scratchpad (catatan + transform)" => "Scratchpad (notes + transform)",
            "Catatan, token, JSON, JWT, SQL…" => "Notes, tokens, JSON, JWT, SQL…",
            "{} karakter · {} kata · {} baris" => "{} chars · {} words · {} lines",
            "JSON rapi" => "Pretty JSON",
            "Base64 decode" => "Base64 decode",
            "JWT decode" => "JWT decode",
            "SQL rapi" => "Tidy SQL",
            "Tutup" => "Close",
            "Pintasan AsisLog (F1)" => "AsisLog shortcuts (F1)",
            "Fokus ke kolom Cari" => "Focus the Search box",
            "Hasil berikutnya / sebelumnya" => "Next / previous result",
            "Hasil berikut / sebelum (di luar kolom ketik)" => {
                "Next / previous result (outside text fields)"
            }
            "Label warna dari query aktif" => "Color label from active query",
            "Ke baris / persen / akhir / waktu" => "Go to line / percent / end / time",
            "Ekspor hasil pencarian" => "Export search results",
            "Awal / akhir file" => "Start / end of file",
            "Pindah tab (berlaku juga saat mengetik)" => "Switch tab (works while typing)",
            "Ikuti akhir file (LIVE)" => "Follow file tail (LIVE)",
            "Tandai baris aktif" => "Bookmark active line",
            "Panel penanda" => "Bookmarks panel",
            "History mundur / maju" => "History back / forward",
            "Penanda sebelumnya / berikutnya" => "Previous / next bookmark",
            "Zoom UI" => "UI zoom",
            "Gulir viewport (di luar kolom ketik)" => "Scroll viewport (outside text fields)",
            "Tutup dialog teratas; lalu batalkan pencarian" => {
                "Close top dialog; then cancel search"
            }
            "Ya, hapus" => "Yes, delete",
            "Galat" => "Error",
            // ---- viewport ----
            "Baris #{}" => "Line #{}",
            "Klik untuk lompat ke baris ini" => "Click to jump to this line",
            "Teal: hasil pencarian ({})\nBiru: penanda ({})\nMerah: bucket ERROR ({} dari 512)\nKuning: bucket WARN ({} dari 512)\nArsir merah: kepadatan ERROR/menit\nHijau: hasil aktif - Putih: posisi viewport\nKlik: lompat ke posisi" => {
                "Teal: search results ({})\nBlue: bookmarks ({})\nRed: ERROR buckets ({} of 512)\nYellow: WARN buckets ({} of 512)\nRed shading: ERROR/min density\nGreen: active result - White: viewport position\nClick: jump to position"
            }
            "Klik untuk lompat ke baris waktu ini" => "Click to jump to this time",
            "Ke akhir file (Ctrl+End)" => "Go to end of file (Ctrl+End)",
            "Baris {} / {} · {}%" => "Line {} / {} · {}%",
            "Salin" => "Copy",
            "Salin baris ini" => "Copy this line",
            "Baris disalin ke papan klip." => "Line copied to clipboard.",
            "Salin 50 baris" => "Copy 50 lines",
            "50 baris disalin." => "50 lines copied.",
            "Salin + nomor (50 baris)" => "Copy with numbers (50 lines)",
            "Format \"nomor: isi\"" => "Format \"number: content\"",
            "Simpan 200 baris ke file…" => "Save 200 lines to file…",
            "Tulis 200 baris dari posisi ini ke file baru" => {
                "Write 200 lines from here to a new file"
            }
            "Disimpan {} baris ke {}." => "Saved {} lines to {}.",
            "Path + baris disalin." => "Path + line copied.",
            "{}/{} · {}%" => "{}/{} · {}%",
            "Salin sebagai path" => "Copy as path",
            "Salin \"file:baris\" untuk referensi" => "Copy \"file:line\" for reference",
            "Salin blok" => "Copy block",
            "Salin blok SQL" => "Copy SQL block",
            "Statement --INSERT-…/INSERT INTO… s.d. go" => "Statement --INSERT-…/INSERT INTO… to go",
            "Salin blok transaksi" => "Copy transaction block",
            "BEGIN TRANSACTION s.d. COMMIT/ROLLBACK/go" => "BEGIN TRANSACTION to COMMIT/ROLLBACK/go",
            "Salin blok checkpoint" => "Copy checkpoint block",
            "--START CHECKPOINT s.d. --FINISH CHECKPOINT" => "--START CHECKPOINT to --FINISH CHECKPOINT",
            "Ekspor blok SQL…" => "Export SQL block…",
            "Ekspor blok transaksi…" => "Export transaction block…",
            "Ekspor blok checkpoint…" => "Export checkpoint block…",
            "Kolom SQL" => "SQL columns",
            "Tampilan Kolom SQL: pisahkan Timestamp / Sesi / Aksi / Kueri secara terstruktur" => {
                "SQL columns view: split Timestamp / Session / Action / Query structurally"
            }
            // ---- palette ----
            "Palet Perintah" => "Command palette",
            "Ketik nama perintah..." => "Type a command name...",
            "Tidak ada aksi yang cocok." => "No matching actions.",
            "Layar Penuh aktif (F11 untuk kembali)." => "Full Screen on (F11 to exit).",
            "Layar Penuh dinonaktifkan." => "Full Screen off.",
            // ---- highlight ----
            "Set highlight (viewport)" => "Highlight sets (viewport)",
            "Set bernama per produk; aturan hanya untuk baris terlihat." => {
                "Named sets per product; rules apply to visible lines only."
            }
            "Set:" => "Set:",
            "Buat" => "Create",
            "Nama set tidak boleh kosong." => "Set name cannot be empty.",
            "Set dengan nama itu sudah ada." => "A set with that name already exists.",
            "Ekspor set…" => "Export set…",
            "Impor set…" => "Import set…",
            "Hapus set" => "Delete set",
            "Belum ada set. Buat set dulu di atas." => "No sets yet. Create one above first.",
            "Tambah aturan ke set aktif:" => "Add rule to active set:",
            "Nama" => "Name",
            "Pola" => "Pattern",
            "Peka huruf" => "Case sensitive",
            "Baris penuh" => "Whole line",
            "Variasi warna" => "Vary color",
            "Warna sedikit beda per teks cocok" => "Slightly different color per matched text",
            "Hanya grup tangkap" => "Capture groups only",
            "Regex saja: sorot grup (a|b) bukan seluruh cocok" => "Regex only: highlight groups (a|b), not the whole match",
            "Tambah" => "Add",
            "Buat/pilih set dulu sebelum menambah aturan." => {
                "Create/select a set before adding rules."
            }
            "Set '{}' diekspor ke {}." => "Set '{}' exported to {}.",
            "Gagal menulis: {}" => "Failed to write: {}",
            "Gagal menyusun: {}" => "Failed to build: {}",
            "Set '{}' diimpor{}." => "Set '{}' imported{}.",
            ", {} aturan salah dilewati" => ", {} invalid rules skipped",
            "File bukan set highlight yang valid." => "File is not a valid highlight set.",
            "Gagal membaca: {}" => "Failed to read: {}",
            "Pola sorotan tidak boleh kosong." => "Highlight pattern cannot be empty.",
            "teks" => "text",
            "Hapus aturan" => "Delete rule",
            "Regex sorotan tidak valid: {}" => "Invalid highlight regex: {}",
            // ---- histogram / top-n / hex ----
            "Histogram ERROR per Menit (Klik bar untuk melompat)" => {
                "ERROR per Minute Histogram (Click a bar to jump)"
            }
            "Tutup panel histogram" => "Close histogram panel",
            "Tidak ada data ERROR tercatat dalam histogram." => {
                "No ERROR data recorded in histogram."
            }
            "Bin #{}/{} · {} ERROR" => "Bin #{}/{} · {} ERRORs",
            "Histogram waktu sedang dihitung di latar belakang atau file tidak memiliki cap waktu." => {
                "Time histogram is computing in background or file has no timestamps."
            }
            "Agregasi Top-N (Error & Sesi)" => "Top-N aggregation (Errors & Sessions)",
            "Top Error" => "Top errors",
            "Top Sesi" => "Top sessions",
            "File" => "File",
            "hasil" => "results",
            "Navigasi" => "Navigate",
            "Tampilan" => "View",
            "Log" => "Log",
            "Ekspor" => "Export",
            "Alat" => "Tools",
            "Atau seret file ke jendela ini · Ctrl+O · Ctrl+Shift+P untuk palet" => {
                "Or drop a file here · Ctrl+O · Ctrl+Shift+P for palette"
            }
            "F11 Layar Penuh · Ctrl+F cari · F1 pintasan" => {
                "F11 Full Screen · Ctrl+F search · F1 shortcuts"
            }
            "Investigasi" => "Investigate",
            "Bantuan" => "Help",
            "Jadikan filter pencarian" => "Make search filter",
            // ---- palette titles (Buka file log… already above) ----
            "Fokus pencarian" => "Focus search",
            "Toggle Layar Penuh (Kepadatan)" => "Toggle Full Screen (density)",
            "Toggle Ikuti log (LIVE)" => "Toggle follow log (LIVE)",
            "Ke baris / cap waktu…" => "Go to line / timestamp…",
            "Ekspor hasil pencarian…" => "Export search results…",
            "Tiket Markdown Jira (1-klik)" => "Jira Markdown ticket (1-click)",
            "Lipat Baris / Word Wrap (Toggle)" => "Word wrap (toggle)",
            "Cari Cepat (QuickFind)" => "Quick find",
            "Buka Folder File (Explorer)" => "Open file folder (Explorer)",
            "Buka File di Aplikasi Default" => "Open file in default app",
            "Tambah / Hapus Penanda baris" => "Add / remove line bookmark",
            "Panel Penanda" => "Bookmarks panel",
            "Catatan / Scratchpad (Base64/JWT/SQL)" => "Notes / Scratchpad (Base64/JWT/SQL)",
            "Aturan Sorotan Warna" => "Color highlight rules",
            "Simpan Workspace…" => "Save workspace…",
            "Buka Workspace…" => "Open workspace…",
            "Tampilan Kolom SQL (Toggle)" => "SQL columns view (toggle)",
            "Panel Histogram Waktu ERROR (Toggle)" => "ERROR time histogram panel (toggle)",
            "Agregasi Top-N Error / Sesi" => "Top-N errors / sessions",
            "Hex Peek (Mode biner aman)" => "Hex Peek (safe binary mode)",
            "Tema: Sistem (Otomatis OS)" => "Theme: System (OS auto)",
            "Tema: Gelap" => "Theme: Dark",
            "Tema: Terang" => "Theme: Light",
            "Tema: Kontras Tinggi" => "Theme: High contrast",
            "Tema: Monokai" => "Theme: Monokai",
            "Tema: Senja Biru" => "Theme: Blue dusk",
            "Tema: Solarized Gelap" => "Theme: Solarized dark",
            "Tema: Solarized Terang" => "Theme: Solarized light",
            "Zoom: Perbesar (+10%)" => "Zoom: in (+10%)",
            "Zoom: Perkecil (-10%)" => "Zoom: out (-10%)",
            "Zoom: Reset (100%)" => "Zoom: reset (100%)",
            "Daftar Pintasan Keyboard" => "Keyboard shortcuts",
            "Klik tombol di atas untuk menganalisis 10 error atau sesi terbanyak." => {
                "Click a button above to analyze the top 10 errors or sessions."
            }
            "Hex Peek (Mode Inspeksi Biner Aman)" => "Hex Peek (Safe binary inspection)",
            "Menampilkan 512 byte mulai offset 0x{}:" => {
                "Showing 512 bytes from offset 0x{}:"
            }
            // ---- analyzer: parser / SQL-lite / merge ----
            "Analisis Log (Parser · SQL · Gabung)" => "Log analysis (Parser · SQL · Merge)",
            "Analisis Log (Parser/SQL/Gabung)" => "Log analysis (Parser/SQL/Merge)",
            "Analisis" => "Analyze",
            "Parser" => "Parser",
            "SQL-lite" => "SQL-lite",
            "Gabung" => "Merge",
            "Deteksi format tab ini" => "Detect this tab's format",
            "Tab kosong / belum terindeks." => "Empty tab / not indexed yet.",
            "Otomatis (JSON → SQL → generik)" => "Auto (JSON → SQL → generic)",
            "Kustom" => "Custom",
            "Otomatis: tiap baris dicoba JSON → kolom SQL → pola generik; kolom kustom di bawah tetap ditambahkan ke tabel SQL." => {
                "Auto: each line is tried as JSON → SQL columns → generic pattern; custom columns below are still added to the SQL table."
            }
            "Parser kustom tersimpan:" => "Saved custom parsers:",
            "Belum ada. Buat lewat wizard di bawah." => "None yet. Create one with the wizard below.",
            "Pakai" => "Use",
            "Hapus parser" => "Delete parser",
            "Wizard parser:" => "Parser wizard:",
            "Contoh baris:" => "Sample line:",
            "Ambil baris terpilih" => "Take selected line",
            "Pola (regex + (?P<nama>...) atau singkatan {TS} {LVL} {MSG} {kolom} {kolom:regex}):" => {
                "Pattern (regex + (?P<name>...) or shorthand {TS} {LVL} {MSG} {col} {col:regex}):"
            }
            "Uji pola" => "Test pattern",
            "Simpan parser" => "Save parser",
            "Pola valid tapi TIDAK cocok dengan contoh." => {
                "Pattern is valid but does NOT match the sample."
            }
            "Cocok! Kolom: {}" => "Match! Columns: {}",
            "Parser tersimpan. Kolom: {}" => "Parser saved. Columns: {}",
            "Kolom: {}" => "Columns: {}",
            "Kolom kustom (tambah ke tabel SQL, pisah koma):" => {
                "Custom columns (added to the SQL table, comma-separated):"
            }
            "Mis. host, status, session — diambil dari parser/JSON/k=v bila ada, kosong bila tidak." => {
                "E.g. host, status, session — taken from parser/JSON/k=v when present, empty otherwise."
            }
            "Dialek SQL-lite (bukan Transact-SQL penuh): SELECT koloms / * / COUNT(*) · WHERE AND OR NOT = != ~ !~ > < >= <= · GROUP BY · ORDER BY count DESC · LIMIT. Tanpa SELECT = filter WHERE saja. Pindai dibatasi 2 jt baris pertama tab aktif." => {
                "SQL-lite dialect (not full Transact-SQL): SELECT cols / * / COUNT(*) · WHERE AND OR NOT = != ~ !~ > < >= <= · GROUP BY · ORDER BY count DESC · LIMIT. No SELECT = WHERE filter only. Scan limited to the active tab's first 2M lines."
            }
            "Jalankan di tab aktif" => "Run on active tab",
            "Ekspor CSV" => "Export CSV",
            "Tidak ada tab terbuka." => "No tabs open.",
            "Grafik:" => "Chart:",
            "Hasil ({} baris tampil):" => "Results ({} rows shown):",
            "…dan {} baris lain (ekspor CSV untuk semua)." => {
                "…and {} more rows (export CSV for all)."
            }
            "CSV tersimpan: {} baris." => "CSV saved: {} rows.",
            "Pindai {} / {} · {} cocok." => "Scanned {} / {} · {} matched.",
            "dibatasi 2 jt baris" => "capped at 2M lines",
            "baris dipangkas LIMIT" => "rows trimmed by LIMIT",
            "Gabung N log jadi 1 timeline sortir-cap-waktu (maks 200 rb baris/file, tampil 5 rb). Multi-cari memakai mesin yang sama dengan pencarian tab." => {
                "Merge N logs into 1 timestamp-sorted timeline (max 200k lines/file, 5k shown). Multi-search uses the same engine as tab search."
            }
            "Bangun timeline semua tab" => "Build timeline of all tabs",
            "Ekspor gabungan" => "Export merge",
            "Gabungan tersimpan: {} baris." => "Merge saved: {} rows.",
            "Cakupan & skew waktu:" => "Coverage & time skew:",
            "Timeline ({} baris):" => "Timeline ({} rows):",
            "Timeline: {} baris dari {} file (maks 200 rb/file)." => {
                "Timeline: {} rows from {} files (max 200k/file)."
            }
            "Cari di semua tab:" => "Search all tabs:",
            "Cari semua" => "Search all",
            "Isi dulu pola pencarian." => "Enter a search pattern first.",
            "Total {} hasil di {} file." => "Total {} results in {} files.",
            "File {} tidak lagi terbuka." => "File {} is no longer open.",
            "'+' = dipangkas 50 rb/file; buka tab untuk pindaian penuh + panel hasil." => {
                "'+' = truncated at 50k/file; open the tab for a full scan + results panel."
            }
            "Terdeteksi: {} ({}% dari {} baris sampel)." => {
                "Detected: {} ({}% of {} sample lines)."
            }
            "Nama parser tidak boleh kosong." => "Parser name cannot be empty.",
            "Pola harus punya minimal 1 grup bernama (?P<nama>...)." => {
                "Pattern needs at least 1 named group (?P<name>...)."
            }
            // ---- misc status ----
            "Ketik query dulu, lalu tekan 1-9 untuk label warna." => {
                "Type a query first, then press 1-9 for a color label."
            }
            "Label {} dihapus." => "Label {} removed.",
            "Set penuh (50 aturan). Hapus dulu yang tak perlu." => {
                "Set is full (50 rules). Delete unused ones first."
            }
            "Label {}: \"{}\" ({}). Tekan lagi untuk hapus." => {
                "Label {}: \"{}\" ({}). Press again to remove."
            }
            "Tidak ada tab untuk disimpan." => "No tabs to save.",
            "Workspace disimpan ke {}." => "Workspace saved to {}.",
            "Workspace '{}': {} dibuka{}." => "Workspace '{}': {} opened{}.",
            ", {} file hilang, dilewati" => ", {} missing files skipped",
            "Sesi dipulihkan: {} tab{}." => "Session restored: {} tab(s){}.",
            ", {} file tak ditemukan, dilewati" => ", {} files not found, skipped",
            "Zoom {}%." => "Zoom {}%.",
            "Tidak ada penanda di baris aktif. Tekan Ctrl+B dulu." => {
                "No bookmark on active line. Press Ctrl+B first."
            }
            "Hasil dari cache (pola sama)." => "Results from cache (same pattern).",
            // ---- view modes (Semua already translated above) ----
            "Hasil" => "Results",
            // ---- themes ----
            "Sistem (otomatis)" => "System (auto)",
            "Gelap" => "Dark",
            "Terang" => "Light",
            "Terang kontras" => "Light contrast",
            "Kontras tinggi" => "High contrast",
            "Senja biru" => "Blue dusk",
            "Solarized gelap" => "Solarized dark",
            "Solarized terang" => "Solarized light",
            // ---- bookmark colors ----
            "Bawaan" => "Default",
            "Sistem" => "System",
            "Biru" => "Blue",
            "Hijau" => "Green",
            "Kuning" => "Yellow",
            "Merah" => "Red",
            "Ungu" => "Purple",
            // ---- highlight colors ----
            "Oranye" => "Orange",
            "Teal" => "Teal",
            "Pink" => "Pink",
            "Cokelat" => "Brown",
            // ---- misc ----
            "Disalin dengan nomor baris." => "Copied with line numbers.",
            "Disalin sebagian (16 MB)." => "Partially copied (16 MB).",
            "Tidak ada baris untuk disalin." => "No lines to copy.",
            "Rentang terlalu besar (maks ~1 juta baris)." => "Range too large (max ~1M lines).",
            "Baris di luar jangkauan." => "Line out of range.",
            "Nomor baris tidak valid." => "Invalid line number.",
            "Persen tidak valid. Contoh: 50%" => "Invalid percent. Example: 50%",
            "Cap waktu tidak ditemukan." => "Timestamp not found.",
            "Tidak dikenali. Gunakan nomor baris, persen (50%), atau cap waktu." => {
                "Unrecognized. Use a line number, percent (50%), or timestamp."
            }
            "Masukkan nomor baris, persen (mis. 50%), akhir, atau cap waktu." => {
                "Enter a line number, percent (e.g. 50%), end, or timestamp."
            }
            "Lompat ke semua tab: {} ok, {} gagal." => "Jump in all tabs: {} ok, {} failed.",
            "Direktori config tidak ditemukan." => "Config directory not found.",
            "Gagal membuat direktori config: {}" => "Failed to create config dir: {}",
            "Gagal menulis config: {}" => "Failed to write config: {}",
            "Gagal menyusun config: {}" => "Failed to build config: {}",
            "Gagal menulis sesi: {}" => "Failed to write session: {}",
            "Gagal menyusun sesi: {}" => "Failed to build session: {}",
            "Gagal membaca workspace: {}" => "Failed to read workspace: {}",
            "Workspace tidak valid: {}" => "Invalid workspace: {}",
            "Gagal menyusun workspace: {}" => "Failed to build workspace: {}",
            "Gagal menulis workspace: {}" => "Failed to write workspace: {}",
            "Gagal memetakan ulang '{}': {}" => "Failed to remap '{}': {}",
            "Bahasa diganti ke English. / Language switched to English." => {
                "Language switched to English. / Bahasa diganti ke English."
            }
            "Bahasa diganti ke Indonesia. / Language switched to Indonesian." => {
                "Language switched to Indonesian. / Bahasa diganti ke Indonesia."
            }
            // ---- P0/P1 additions (wrap, selection, quickfind, options, tabs) ----
            "Lipat (W)" => "Wrap (W)",
            "Word wrap: baris panjang dilipat ke lebar jendela" => {
                "Word wrap: long lines fold to the window width"
            }
            "Lipat baris (word wrap)" => "Word wrap",
            "Lipat baris (word wrap) default untuk tab baru" => {
                "Word wrap by default for new tabs"
            }
            "Seleksi disalin ke papan klip." => "Selection copied to clipboard.",
            "Pilih semua baris" => "Select all lines",
            "Cari cepat (QuickFind)" => "Quick find",
            "Cari cepat:" => "Quick find:",
            "Ketik untuk cari instan… (Enter berikutnya)" => {
                "Type to find instantly… (Enter for next)"
            }
            "Sebelumnya (Shift+Enter)" => "Previous (Shift+Enter)",
            "Berikutnya (Enter)" => "Next (Enter)",
            "Sampai akhir file — kembali ke awal." => "Reached end of file — wrapped to start.",
            "Sampai awal file — kembali ke akhir." => "Reached start of file — wrapped to end.",
            "Tambah seleksi ke pencarian (OR)" => "Add selection to search (OR)",
            "Kecualikan seleksi dari pencarian" => "Exclude selection from search",
            "Ganti pencarian dengan seleksi" => "Replace search with selection",
            "Tidak ada teks terpilih untuk ditambahkan (klik baris / pilih kata dulu)." => {
                "No selected text to add (click a line / select a word first)."
            }
            "+ Seleksi (OR)" => "+ Selection (OR)",
            "- Seleksi" => "- Selection",
            "Tambah teks terpilih ke query pencarian (Shift+A)" => {
                "Add selected text to the search query (Shift+A)"
            }
            "Kecualikan teks terpilih dari pencarian (Shift+E)" => {
                "Exclude selected text from the search (Shift+E)"
            }
            "Folder" => "Folder",
            "Buka folder file ini di File Explorer" => "Open this file's folder in File Explorer",
            "Buka di aplikasi" => "Open in app",
            "Buka file ini di aplikasi default OS" => "Open this file in the OS default app",
            "Path penuh" => "Full path",
            "Salin path penuh file ini" => "Copy this file's full path",
            "Path penuh disalin." => "Full path copied.",
            "Folder dibuka di Explorer." => "Folder opened in Explorer.",
            "Folder dibuka di Finder." => "Folder opened in Finder.",
            "Folder dibuka." => "Folder opened.",
            "Gagal membuka Explorer: {}" => "Failed to open Explorer: {}",
            "Gagal membuka Finder: {}" => "Failed to open Finder: {}",
            "Gagal membuka folder: {}" => "Failed to open folder: {}",
            "Dibuka di aplikasi default." => "Opened in the default app.",
            "Gagal membuka aplikasi default: {}" => "Failed to open default app: {}",
            "Path tanpa folder." => "Path has no folder.",
            "File sudah terbuka — pindah ke tab-nya." => "File already open — switched to its tab.",
            "Tutup tab ini" => "Close this tab",
            "Tutup tab lainnya" => "Close other tabs",
            "Tutup semua tab" => "Close all tabs",
            "Buka folder file" => "Open file folder",
            "Pengaturan" => "Settings",
            "Pengaturan AsisLog…" => "AsisLog settings…",
            "Pengaturan AsisLog (Ctrl+,)" => "AsisLog settings (Ctrl+,)",
            "Pengaturan disimpan." => "Settings saved.",
            "Pemantauan file (LIVE)" => "File watching (LIVE)",
            "Interval poll (ms):" => "Poll interval (ms):",
            "Lebih kecil = lebih responsif, lebih besar = hemat CPU. Default 250 ms." => {
                "Smaller = more responsive, larger = saves CPU. Default 250 ms."
            }
            "Ada pintasan dobel — dua aksi akan terpicu bersamaan." => {
                "Duplicate shortcut — two actions would fire together."
            }
            "DOBEL: dipakai aksi lain juga" => "DUPLICATE: also used by another action",
            "Tutup tab kini" => "Close current tab",
            "Tab 1" => "Tab 1",
            "Tab 2" => "Tab 2",
            "Tab 3" => "Tab 3",
            "Tab 4" => "Tab 4",
            "Tab 5" => "Tab 5",
            "Tab 6" => "Tab 6",
            "Tab 7" => "Tab 7",
            "Tab 8" => "Tab 8",
            "Tab terakhir" => "Last tab",
            // P1-12 state machine + P1-13 options penuh + multi-select + tab.
            "Mencari…" => "Searching…",
            "Auto-refresh…" => "Auto-refreshing…",
            "Statis" => "Static",
            "Dibatasi" => "Truncated",
            "Mesin pencari" => "Search engine",
            "Thread (0=auto):" => "Threads (0=auto):",
            "Ganti thread berlaku setelah restart bila pencarian pernah jalan." => {
                "Thread change applies after restart if a search already ran."
            }
            "Maks hasil (0=default):" => "Max hits (0=default):",
            "Chunk pindai MiB (0=default):" => "Scan chunk MiB (0=default):",
            "Cache pola (0=default):" => "Pattern cache (0=default):",
            "Default: 200 rb hasil, 4 MiB chunk, 8 pola." => {
                "Defaults: 200K hits, 4 MiB chunk, 8 patterns."
            }
            "Tiap hasil ±24 B RAM; 10 jt ≈ 240 MB. Jutaan match: pakai Ekspor SEMUA (streaming)." => {
                "Each hit ≈24 B RAM; 10M ≈ 240 MB. For millions of matches use Export ALL (streaming)."
            }
            "Watch native (OS event) + polling sebagai fallback." => {
                "Native watch (OS events) + polling fallback."
            }
            "Watch native: aktif (event OS memicu poll segera)." => {
                "Native watch: on (OS events trigger an immediate poll)."
            }
            "Watch native: mati (polling saja)." => {
                "Native watch: off (polling only)."
            }
            "Pengaturan disimpan. Thread baru berlaku setelah restart." => {
                "Settings saved. New thread count applies after restart."
            }
            "Pindahkan ke kiri" => "Move left",
            "Pindahkan ke kanan" => "Move right",
            "Ubah nama tab…" => "Rename tab…",
            "Ubah nama tab" => "Rename tab",
            "Nama tab (kosong = nama file):" => "Tab name (empty = file name):",
            "Ctrl+klik: tambah/hapus baris ke seleksi." => {
                "Ctrl+click: add/remove lines to selection."
            }
            "seleksi kata" => "word selection",
            "seleksi teks" => "text selection",
            "Tentang AsisLog" => "About AsisLog",
            "Tentang AsisLog (versi + log fitur)" => "About AsisLog (version + feature log)",
            "Batas & catatan jujur:" => "Limits & honest notes:",
            "SQL-lite: maks 2 jt baris pertama · Gabung timeline: 200 rb baris/file · Regex fancy: 256 KB/baris · Hyperscan/Vectorscan tidak tersedia di Windows (NO-GO). Ekspor streaming tanpa batas tampil." => {
                "SQL-lite: max first 2M lines · Timeline merge: 200k lines/file · Fancy regex: 256 KB/line · Hyperscan/Vectorscan not available on Windows (NO-GO). Streaming export is uncapped."
            }
            "Versi disalin." => "Version copied.",
            "Salin versi" => "Copy version",
            "Log fitur (CHANGELOG.md):" => "Feature log (CHANGELOG.md):",
            "Versi {}" => "Version {}",
            _ => id,
        }
    }

    /// Translate an already-formatted status line (engine messages).
    /// Order: exact key → prefix → full-template match → fallback.
    /// Templates let formatted messages (`format!` values baked in) translate
    /// without threading UI language into the engine: literals must match
    /// exactly, gaps (`{}`) capture free text in order.
    pub fn tr_status(self, msg: &str) -> String {
        if self == Lang::Id {
            return msg.to_string();
        }
        // Exact static hits first.
        let exact = self.tr(msg);
        if exact != msg {
            return exact.to_string();
        }
        const PREFIXES: &[(&str, &str)] = &[
            // NOTE: no prefix here may shadow a TEMPLATE_PAIRS shape with a
            // worse partial translation (locked by test). Prefix rests must
            // already be language-neutral (paths, numbers, English IO errors).
            ("Gagal membuka Explorer", "Failed to open Explorer"),
            ("Gagal membuka Finder", "Failed to open Finder"),
            ("Gagal membuka folder", "Failed to open folder"),
            ("Gagal membuka aplikasi default", "Failed to open default app"),
            ("Membuka ", "Opening "),
            ("File favorit tak ditemukan", "Favorite file not found"),
            ("Gagal ", "Failed "),
            ("Tidak ada ", "No "),
            ("Belum ada ", "No "),
            ("Hasil disalin", "Results copied"),
            ("Baris disalin", "Line copied"),
            ("50 baris disalin", "50 lines copied"),
            ("Filter diterapkan: ", "Filter applied: "),
            ("Rentang tidak valid", "Invalid range"),
            ("Pencarian dibatalkan", "Search cancelled"),
            ("Ekspor dibatalkan", "Export cancelled"),
            ("Ekspor berjalan", "Export running"),
            ("Mencari…", "Searching…"),
            ("Mengindeks ", "Indexing "),
            ("Mengunduh…", "Downloading…"),
            ("LIVE dijeda", "LIVE paused"),
            ("memantau tiap 500 ms", "watching every 500 ms"),
            ("Layar Penuh aktif", "Full Screen on"),
            ("Layar Penuh dinonaktifkan", "Full Screen off"),
            ("Zoom ", "Zoom "),
            ("File dipotong/dirotasi", "File truncated/rotated"),
            ("File bertambah", "File grew"),
            ("Disalin ", "Copied "),
            ("Ketik query dulu", "Type a query first"),
            ("Kembali ke lokasi", "Back to location"),
            ("Maju ke lokasi", "Forward to location"),
            ("Riwayat file dikosongkan", "File history cleared"),
            ("Nama preset tidak boleh kosong", "Preset name cannot be empty"),
            ("Nama set tidak boleh kosong", "Set name cannot be empty"),
            ("Set dengan nama itu sudah ada", "A set with that name already exists"),
            ("Set penuh", "Set is full"),
            ("Buat/pilih set dulu", "Create/select a set first"),
            ("Pencarian regex tidak dapat dijadikan filter", "Regex search cannot be converted"),
            ("Query pencarian tidak valid", "Invalid search query"),
            ("Gagal konversi ke filter", "Failed to convert to filter"),
            ("Boolean search belum mendukung UTF-16", "Boolean search does not support UTF-16 yet"),
            ("Regex kompleks belum mendukung UTF-16", "Complex regex does not support UTF-16 yet"),
            ("Regex tidak valid", "Invalid regex"),
            ("URL kosong", "URL is empty"),
            ("Teks kosong", "Text is empty"),
            ("URL harus http", "URL must be http"),
            ("File kosong", "Empty file"),
            ("File sumber berubah", "Source file changed"),
            ("Sidecar penanda rusak", "Bookmark sidecar corrupt"),
            ("Versi sidecar penanda beda", "Bookmark sidecar version mismatch"),
            ("Bukan base64 yang valid", "Not valid base64"),
            ("Bukan JSON valid", "Not valid JSON"),
            ("Segmen ", "Segment "),
            ("Akhir blok", "End of block"),
            ("Awal blok", "Start of block"),
            ("Awal transaksi", "Start of transaction"),
            ("Akhir blok tidak ditemukan", "End of block not found"),
            ("Awal checkpoint", "Start of checkpoint"),
            ("Blok SQL baris", "SQL block line"),
            ("Transaksi baris", "Transaction line"),
            ("Checkpoint baris", "Checkpoint line"),
            ("Pilihan melebihi 16 MB", "Selection exceeds 16 MB"),
            ("Hasil dari cache", "Results from cache"),
            ("Lompat ke baris", "Jump to line"),
            ("Pos: baris", "Pos: line"),
            ("Cari: ", "Search: "),
            ("Pilihan melebihi", "Selection exceeds"),
            ("Seleksi disalin", "Selection copied"),
            ("Folder dibuka", "Folder opened"),
            ("Dibuka di aplikasi", "Opened in app"),
            ("Path penuh disalin", "Full path copied"),
            ("File sudah terbuka", "File already open"),
            ("Tutup tab", "Close tab"),
            ("Pengaturan disimpan", "Settings saved"),
        ];
        for (id_pre, en_pre) in PREFIXES {
            if let Some(rest) = msg.strip_prefix(id_pre) {
                return format!("{}{}", en_pre, rest);
            }
        }
        if let Some(out) = match_template(msg) {
            return out;
        }
        msg.to_string()
    }
}

/// Formatted status templates (ID → EN), most-specific first.
/// Used by `tr_status` when neither exact key nor prefix matches, so
/// engine/app-core `format!` messages translate without threading
/// language into the engine. Gaps capture free text (numbers, paths,
/// queries) left-to-right. ORDER MATTERS: longer shapes before their
/// prefixes (locked by test).
const TEMPLATE_PAIRS: &[(&str, &str)] = &[
    ("Dari {}: {} ({} entri)", "From {}: {} ({} entries)"),
    ("Dari {}: {}", "From {}: {}"),
    ("Filter: {} baris cocok (+{} baru).", "Filter: {} lines match (+{} new)."),
    ("Filter aktif: {} baris cocok.", "Active filter: {} matching lines."),
    ("Sesi dipulihkan: {} tab{}.", "Session restored: {} tab(s){}."),
    ("Workspace '{}': {} dibuka{}.", "Workspace '{}': {} opened{}."),
    ("Workspace disimpan ke {}.", "Workspace saved to {}."),
    ("Membuka {}.", "Opening {}."),
    ("File tidak ditemukan: {}", "File not found: {}"),
    ("File favorit tak ditemukan: {}", "Favorite file not found: {}"),
    ("File tidak ditemukan, dihapus dari riwayat: {}", "File not found, removed from history: {}"),
    (", {} file hilang, dilewati", ", {} missing files skipped"),
    (", {} file tak ditemukan, dilewati", ", {} files not found, skipped"),
    ("Cakupan {}-{} aktif; ketik query untuk mencari.", "Scope {}-{} active; type a query to search."),
    ("Cakupan: {}-{}", "Scope: {}-{}"),
    ("Set '{}' diekspor ke {}.", "Set '{}' exported to {}."),
    ("Set '{}' diimpor{}.", "Set '{}' imported{}."),
    (", {} aturan salah dilewati", ", {} invalid rules skipped"),
    ("Bukan file: '{}'", "Not a file: '{}'"),
    ("Baris melebihi {} baris.", "Line exceeds {} lines."),
    ("Rentang waktu: {} baris{}.", "Time range: {} lines{}."),
    ("Penanda baris {} diperbarui.", "Bookmark on line {} updated."),
    ("Penanda baris {} dihapus.", "Bookmark on line {} deleted."),
    ("Lompat ke semua tab: {} ok, {} gagal.", "Jump in all tabs: {} ok, {} failed."),
    ("Tiket ({} baris konteks) disimpan ke {}.", "Ticket ({} lines of context) saved to {}."),
    ("Diekspor {} baris ke {}.", "Exported {} lines to {}."),
    ("Disimpan {} baris ke {}.", "Saved {} lines to {}."),
    ("Indeks selesai: {} baris.", "Index finished: {} lines."),
    ("Filter diterapkan: {}", "Filter applied: {}"),
    ("Label {} dihapus.", "Label {} removed."),
    ("Label {}: \"{}\" ({}). Tekan lagi untuk hapus.", "Label {}: \"{}\" ({}). Press again to remove."),
    ("Kunci {}: {}", "Key {}: {}"),
    ("+{} baris baru", "+{} new lines"),
    ("Zip tidak valid: {}", "Invalid zip: {}"),
    ("Tar tidak valid: {}", "Invalid tar: {}"),
    ("Bz2 tidak valid: {}", "Invalid bz2: {}"),
    ("Xz tidak valid: {}", "Invalid xz: {}"),
    ("7z tidak valid: {}", "Invalid 7z: {}"),
    ("Mengekspor {} / {} hasil…", "Exporting {} / {} results…"),
    ("Mengekspor streaming: {} ditulis…", "Streaming export: {} written…"),
    ("Query tidak valid: {}", "Invalid query: {}"),
    ("Penanda baris {} akan dihapus permanen (tak bisa dibatalkan).",
     "Bookmark on line {} will be permanently deleted (cannot be undone)."),
    ("Penanda ditambahkan di baris {}.", "Bookmark added on line {}."),
    ("Tiket disimpan ({} hasil): {}", "Ticket saved ({} results): {}"),
    ("Sampai akhir file — kembali ke awal.", "Reached end of file — wrapped to start."),
    ("Sampai awal file — kembali ke akhir.", "Reached start of file — wrapped to end."),
    ("Gagal membuka Explorer: {}", "Failed to open Explorer: {}"),
    ("Gagal membuka Finder: {}", "Failed to open Finder: {}"),
    ("Gagal membuka folder: {}", "Failed to open folder: {}"),
    ("Gagal membuka aplikasi default: {}", "Failed to open default app: {}"),
    ("Pola parser tidak valid: {}", "Invalid parser pattern: {}"),
    ("Regex parser gagal: {}", "Parser regex failed: {}"),
    ("Grup '{}' tanpa pola.", "Group '{}' has no pattern."),
    ("'{}' kata kunci, bukan kolom.", "'{}' is a keyword, not a column."),
    ("File biner terdeteksi ({} NUL di 8 KB pertama). Hitungan baris tak valid — gunakan Hex Peek.",
     "Binary file detected ({} NULs in first 8 KB). Line counts are invalid — use Hex Peek."),
    ("Muat {} berikutnya", "Load next {}"),
    ("Pindai {} hasil berikutnya (total eksak {} — tanpa batas RAM)",
     "Scan the next {} results ({} exact total — memory-bounded)"),
];

/// Match `msg` against one ID template; on success rebuild the EN
/// template with captures in order. Literals match exactly (anchored).
fn try_template(re: &regex::Regex, en_tpl: &str, msg: &str) -> Option<String> {
    let caps = re.captures(msg)?;
    if caps.len() - 1 != en_tpl.matches("{}").count() {
        return None;
    }
    let mut out = en_tpl.to_string();
    for i in 1..caps.len() {
        out = out.replacen("{}", &caps[i], 1);
    }
    Some(out)
}

fn template_regex(id_tpl: &str) -> regex::Regex {
    let mut pat = String::from("^");
    let mut first = true;
    for part in id_tpl.split("{}") {
        if !first {
            pat.push_str("(.*?)");
        }
        first = false;
        pat.push_str(&regex::escape(part));
    }
    pat.push('$');
    regex::Regex::new(&pat).expect("valid template regex")
}

fn match_template(msg: &str) -> Option<String> {
    for (re, en_tpl) in compiled_templates() {
        if let Some(out) = try_template(re, en_tpl, msg) {
            return Some(out);
        }
    }
    None
}

/// Compiled template table, built once. Without this, every status-bar
/// frame would recompile ~40 regexes for each template miss (notably the
/// multi-second follow-note window) — pure waste for immutable patterns.
fn compiled_templates() -> &'static [(regex::Regex, &'static str)] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        TEMPLATE_PAIRS
            .iter()
            .map(|(id_tpl, en_tpl)| (template_regex(id_tpl), *en_tpl))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_indonesian() {
        assert_eq!(Lang::default(), Lang::Id);
        assert_eq!(Lang::from_key(""), Lang::Id);
        assert_eq!(Lang::from_key("id"), Lang::Id);
        assert_eq!(Lang::from_key("en"), Lang::En);
        assert_eq!(Lang::from_key("English"), Lang::En);
    }

    #[test]
    fn fallback_returns_source() {
        let en = Lang::En;
        assert_eq!(en.tr("Kalimat tak dikenal XYZ 123"), "Kalimat tak dikenal XYZ 123");
        assert_eq!(Lang::Id.tr("Buka"), "Buka");
    }

    #[test]
    fn locale_override_wins_and_resets() {
        // Overrides beat BOTH built-in languages, then clear cleanly.
        // NOTE: tests run in parallel in one process — use exotic keys no
        // other test touches, and always clear at the end.
        let mut m = HashMap::new();
        m.insert("Kunci proba XYZ".to_string(), "Probe key XYZ".to_string());
        m.insert("Kosong proba".to_string(), "   ".to_string()); // dropped
        set_locale_overrides(m);
        assert_eq!(locale_override_count(), 1);
        assert_eq!(Lang::Id.tr("Kunci proba XYZ"), "Probe key XYZ");
        assert_eq!(Lang::En.tr("Kunci proba XYZ"), "Probe key XYZ");
        assert_eq!(Lang::En.tr("Buka"), "Open"); // built-in untouched
        clear_locale_overrides();
        assert_eq!(locale_override_count(), 0);
        assert_eq!(Lang::En.tr("Kunci proba XYZ"), "Kunci proba XYZ");
    }

    #[test]
    fn locale_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nl.json");
        std::fs::write(&p, r#"{"Tutup proba": "Sluiten", "bad": 123}"#).unwrap();
        // Non-object values are rejected, not half-loaded.
        assert!(load_locale_file(&p).is_err());
        assert_eq!(locale_override_count(), 0);
        std::fs::write(&p, r#"{"Tutup proba": "Sluiten"}"#).unwrap();
        assert_eq!(load_locale_file(&p).unwrap(), 1);
        assert_eq!(Lang::Id.tr("Tutup proba"), "Sluiten");
        clear_locale_overrides();
        assert!(load_locale_file(&dir.path().join("hilang.json")).is_err());
    }

    #[test]
    fn core_buttons_translate() {
        let en = Lang::En;
        assert_eq!(en.tr("Buka"), "Open");
        assert_eq!(en.tr("Simpan"), "Save");
        assert_eq!(en.tr("Batal"), "Cancel");
        assert_eq!(en.tr("Tutup"), "Close");
        assert_eq!(en.tr("Hapus"), "Delete");
        assert_eq!(en.tr("Saring"), "Filter");
        assert_eq!(en.tr("Semua"), "All");
        assert_eq!(en.tr("Hasil"), "Results");
        assert_eq!(en.tr("Penanda"), "Bookmarks");
        assert_eq!(en.tr("Gelap"), "Dark");
        assert_eq!(en.tr("Terang"), "Light");
    }

    #[test]
    fn templates_keep_placeholders() {
        let en = Lang::En;
        assert!(en.tr("Membuka {}.").contains("{}"));
        assert!(en.tr("Baris {} / {} · {}%").contains("{}"));
        assert!(en.tr("Pos: baris {} ({}{}%)").contains("{}"));
    }

    #[test]
    fn status_prefix_translation() {
        let en = Lang::En;
        assert_eq!(en.tr_status("Siap"), "Ready");
        assert!(en.tr_status("Membuka c:/a.log.").starts_with("Opening "));
        assert!(en.tr_status("Gagal menulis sesi: x").starts_with("Failed "));
        // Unknown passes through untouched.
        assert_eq!(en.tr_status("XYZ-unik-123"), "XYZ-unik-123");
    }

    #[test]
    fn system_locale_detection() {
        // No other test reads locale env; still save/restore defensively.
        let keys = ["ASISLOG_LANG", "LANGUAGE", "LC_ALL", "LANG", "LC_MESSAGES"];
        let saved: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        let clear = || {
            for k in keys {
                std::env::remove_var(k);
            }
        };
        clear();
        // Nothing set → historical Indonesian default.
        assert_eq!(Lang::detect_system_lang(), Lang::Id);
        std::env::set_var("LANG", "en_US.UTF-8");
        assert_eq!(Lang::detect_system_lang(), Lang::En);
        std::env::set_var("LANG", "id_ID.UTF-8");
        assert_eq!(Lang::detect_system_lang(), Lang::Id);
        // LANGUAGE priority list: first known wins; C skipped.
        std::env::set_var("LANGUAGE", "C:en");
        std::env::remove_var("LANG");
        assert_eq!(Lang::detect_system_lang(), Lang::En);
        // Unknown locale → English (not an untranslated default).
        std::env::set_var("LANG", "de_DE.UTF-8");
        std::env::remove_var("LANGUAGE");
        assert_eq!(Lang::detect_system_lang(), Lang::En);
        // Explicit override beats everything.
        std::env::set_var("ASISLOG_LANG", "id");
        assert_eq!(Lang::detect_system_lang(), Lang::Id);
        clear();
        for (k, v) in saved {
            if let Some(v) = v {
                std::env::set_var(k, v);
            }
        }
    }

    #[test]
    fn quick_set_and_font_names() {        assert_eq!(Lang::Id.quick_set_name(), "Cepat");
        assert_eq!(Lang::En.quick_set_name(), "Quick");
        // Quick-label rule names round-trip per language.
        assert_eq!(Lang::Id.f2("Kunci {}: {}", 1, "ERROR"), "Kunci 1: ERROR");
        assert_eq!(Lang::En.f2("Kunci {}: {}", 1, "ERROR"), "Key 1: ERROR");
        // Font storage stable, display localized both ways.
        assert_eq!(Lang::En.font_name("Bawaan"), "Default");
        assert_eq!(Lang::Id.font_name("Bawaan"), "Bawaan");
        assert_eq!(Lang::Id.font_name("Default"), "Bawaan");
        assert_eq!(Lang::En.font_name("Consolas"), "Consolas");
    }

    #[test]
    fn template_table_translates_samples() {
        let en = Lang::En;
        assert_eq!(
            en.tr_status("Filter aktif: 1.234 baris cocok."),
            "Active filter: 1.234 matching lines."
        );
        assert_eq!(
            en.tr_status("Filter: 5 baris cocok (+3 baru)."),
            "Filter: 5 lines match (+3 new)."
        );
        assert_eq!(
            en.tr_status("Dari zip: catalina.out (3 entri)"),
            "From zip: catalina.out (3 entries)"
        );
        assert_eq!(en.tr_status("Dari gzip: /tmp/a.log"), "From gzip: /tmp/a.log");
        assert_eq!(en.tr_status("Sesi dipulihkan: 2 tab."), "Session restored: 2 tab(s).");
        assert_eq!(
            en.tr_status("Rentang waktu: 5.000 baris (dibatasi 2 jt)."),
            "Time range: 5.000 lines (dibatasi 2 jt)."
        );
        assert_eq!(en.tr_status("+128 baris baru"), "+128 new lines");
        assert_eq!(en.tr_status("Zip kosong."), "Empty zip.");
        assert_eq!(en.tr_status("Memfilter…"), "Filtering…");
        // ID mode is identity, even for templates.
        assert_eq!(Lang::Id.tr_status("Filter aktif: 1.234 baris cocok."), "Filter aktif: 1.234 baris cocok.");
    }

    #[test]
    fn template_table_first_match_wins_in_order() {        // Lock pair ordering: every template must resolve via ITSELF,
        // never shadowed by an earlier pair (e.g. 3-slot "Dari … entri"
        // must win over 2-slot "Dari …"). Arity ID==EN is enforced too.
        for (idx, (id_tpl, en_tpl)) in TEMPLATE_PAIRS.iter().enumerate() {
            assert_eq!(
                id_tpl.matches("{}").count(),
                en_tpl.matches("{}").count(),
                "arity drift in pair {}: {:?}",
                idx,
                id_tpl
            );
            let got = match_template(id_tpl)
                .unwrap_or_else(|| panic!("pair {} cannot match itself: {:?}", idx, id_tpl));
            assert_eq!(got, *en_tpl, "pair {} self-match", idx);
        }
        // Explicit shadowing probes for the risky overlaps.
        assert_eq!(
            match_template("Dari zip: catalina.out (3 entri)").as_deref(),
            Some("From zip: catalina.out (3 entries)")
        );
        assert_eq!(
            match_template("Dari gzip: /tmp/a.log").as_deref(),
            Some("From gzip: /tmp/a.log")
        );
        assert_eq!(
            match_template("Filter: 5 baris cocok (+3 baru).").as_deref(),
            Some("Filter: 5 lines match (+3 new).")
        );
    }

    #[test]
    fn template_cache_covers_all_pairs_and_is_stable() {
        // The compiled table must mirror TEMPLATE_PAIRS exactly, and
        // repeated translation must be deterministic (cache reuse, no
        // recompilation side effects).
        assert_eq!(compiled_templates().len(), TEMPLATE_PAIRS.len());
        let en = Lang::En;
        for _ in 0..3 {
            assert_eq!(
                en.tr_status("Dari zip: catalina.out (3 entri)"),
                "From zip: catalina.out (3 entries)"
            );
            assert_eq!(en.tr_status("+128 baris baru"), "+128 new lines");
        }
    }

    #[test]
    fn every_template_wins_end_to_end_via_tr_status() {
        // The real invariant: no PREFIX may shadow a template with a worse
        // partial translation. Substitute every slot with "9" and require
        // the full EN rebuild through the public path.
        let en = Lang::En;
        for (id_tpl, en_tpl) in TEMPLATE_PAIRS {
            let slots = id_tpl.matches("{}").count();
            let mut sample = id_tpl.to_string();
            for _ in 0..slots {
                sample = sample.replacen("{}", "9", 1);
            }
            let mut expect = en_tpl.to_string();
            for _ in 0..slots {
                expect = expect.replacen("{}", "9", 1);
            }
            assert_eq!(en.tr_status(&sample), expect, "shadowed template: {:?}", id_tpl);
        }
    }

    /// Source files whose string literals can surface in the status bar,
    /// dialogs, or other user-visible chrome (via `tr`/`tr_status`/`fN`
    /// at render). `main.rs` is excluded: its CLI branches are bilingual
    /// by construction (ID branch intentionally Indonesian).
    const SURFACE_FILES: &[&str] = &[
        "src/engine/mod.rs",
        "src/engine/archive.rs",
        "src/engine/marks.rs",
        "src/engine/scratch.rs",
        "src/engine/query.rs",
        "src/engine/parser.rs",
        "src/engine/squery.rs",
        "src/engine/merge.rs",
        "src/engine/decode.rs",
        "src/engine/follow.rs",
        "src/store.rs",
        "src/app/actions.rs",
        "src/app/tab.rs",
        "src/app/jobs_index.rs",
        "src/app/jobs_search.rs",
        "src/app/state.rs",
        "src/app/ui.rs",
        "src/app/ui_analyze.rs",
        "src/app/ui_chrome.rs",
        "src/app/ui_dialogs.rs",
        "src/app/ui_highlight.rs",
        "src/app/ui_histogram.rs",
        "src/app/ui_misc.rs",
        "src/app/ui_palette.rs",
        "src/app/ui_panels.rs",
        "src/app/ui_search.rs",
        "src/app/ui_tools.rs",
        "src/app/ui_tools_investigation.rs",
        "src/app/ui_viewport.rs",
        "src/ui/dialogs.rs",
        "src/ui/results.rs",
        "src/ui/status.rs",
    ];

    /// Indonesian markers: any literal containing one of these is assumed
    /// user-visible and must resolve in EN (exact key or prefix).
    const ID_MARKERS: &[&str] = &[
        "Gagal", "Tidak", "Belum", "Berhasil", "Sesi", "Workspace", "Penanda",
        "Filter", "Ekspor", "Disalin", "Disimpan", "Tiket", "Indeks", "Membuka",
        "Mencari", "Memfilter", "Mengindeks", "Mengunduh", "Baris", "Cakupan",
        "Rentang", "Lompat", "Label", "Kunci", "Hasil", "Saring", "Bukan",
        "Segmen", "Sidecar", "Versi", "Ikuti", "LIVE", "Pos:", "Cari:", "File:",
        "Dari ", "Zip", "Tar", "Bz", "Xz", "7z", "Entri", "Direktori",
        "Cap waktu", "Nomor", "Persen", "Masukkan", "Pola", "Pilihan", "Contoh",
        "Arsip", "Aktif", "Mati", "Siap", "Ditemukan", "Dibatasi", "Terbatas",
        "Mode ", "Set ", "Tambah", "Ubah", "Warna", "Nama", "Peka", "Konteks",
        "Histogram", "Agregasi", "Pintasan", "Galat", "URL", "Tempel", "Unduh",
        "Buka", "Cari", "Riwayat", "Preset", "Sorotan", "Hapus", "Batal",
        "Simpan", "Tutup", "Tampil", "Kolom", "Pergi", "Terapkan", "Bersihkan",
        "Saring", "Ganti", "Keluar", "Font", "Bahasa", "Semua", "Logika",
        "Favorit", "Otomatis", "Akhir", "Awal", "Sampai", "dipilih", "memantau",
    ];

    /// Literals that are deliberately NOT translated (with reasons).
    /// Keep this list minimal; every entry is user-visible by design.
    /// NOTE: words identical in EN ("Filter", "LIVE", …) can never pass a
    /// `tr_status(x) != x` check by construction — they are listed here
    /// instead of the dictionary, where they would be dead arms.
    const COVERAGE_ALLOW: &[&str] = &[
        // Ticket export content: stable format across languages so tickets
        // stay greppable/shareable regardless of the author's UI language.
        "## Baris {} {{#{}}}",
        "- Konteks: +-{} baris",
        "- File:",
        "- Hasil:",
        // Identical in English: direct-rendered, never via tr_status.
        "Workspace",
        "LIVE",
        "Filter",
        "Label:",
        "Font: {}",
        "Tab: {}",
        // Self-bilingual palette title: shown as-is, found by typing
        // either "bahasa" or "language" (fuzzy matches both halves).
        "Ganti bahasa / Switch language",
        // Internal temp-dir fragment, never displayed.
        "{}-7z",
    ];

    /// Extract `"..."` string literals from Rust source, skipping comments,
    /// char literals, lifetimes and `#[cfg(test)]` modules (test data is
    /// not user-visible). Byte-safe for multi-byte UTF-8 (`…`, `·`, `±`).
    fn source_literals(src: &str) -> Vec<String> {
        // Cut test modules: first #[cfg(test)] starts non-shipped code.
        let src = match src.find("#[cfg(test)]") {
            Some(i) => &src[..i],
            None => src,
        };
        let b = src.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i] as char;
            if c == '/' && i + 1 < b.len() && b[i + 1] == b'/' {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && i + 1 < b.len() && b[i + 1] == b'*' {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            // Byte strings b".." are never UI text: skip entirely
            // (this also keeps magic constants like b"7z\xBC.." out).
            if c == 'b' && i + 1 < b.len() && b[i + 1] == b'"' {
                let mut j = i + 2;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            // Raw strings r".." / r#".."# (may hold UI text with backslashes).
            if c == 'r'
                && i + 1 < b.len()
                && (b[i + 1] == b'"' || b[i + 1] == b'#')
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while j < b.len() && b[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' {
                    j += 1;
                    let start = j;
                    if hashes == 0 {
                        while j < b.len() && b[j] != b'"' {
                            if b[j] == b'\\' {
                                j += 1;
                            }
                            j += 1;
                        }
                        out.push(String::from_utf8_lossy(&b[start..j]).into_owned());
                        i = j + 1;
                        continue;
                    } else {
                        let closer: Vec<u8> =
                            std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
                        let mut k = j;
                        let mut found = None;
                        while k + closer.len() <= b.len() {
                            if &b[k..k + closer.len()] == closer.as_slice() {
                                found = Some(k);
                                break;
                            }
                            k += 1;
                        }
                        if let Some(k) = found {
                            out.push(String::from_utf8_lossy(&b[start..k]).into_owned());
                            i = k + closer.len();
                            continue;
                        }
                    }
                }
            }
            if c == '"' {
                let start = i + 1;
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                out.push(String::from_utf8_lossy(&b[start..j]).into_owned());
                i = j + 1;
                continue;
            }
            if c == '\'' {
                // Char literal 'x' / '\n' vs lifetime 'static: only the
                // former is quote-closed within 4 chars.
                let mut j = i + 1;
                if j < b.len() && b[j] == b'\\' {
                    j += 1;
                }
                j += 1; // the char itself
                if j < b.len() && b[j] == b'\'' {
                    i = j + 1; // real char literal, skip
                } else {
                    i += 1; // lifetime or stray quote, ignore
                }
                continue;
            }
            i += 1;
        }
        out
    }

    /// Resolve Rust escape/continuation sequences the way the compiler
    /// does for the compared value: `\`-newline eats the newline AND the
    /// next line's leading whitespace; then `\"`, `\n`, `\t`, `\\`.
    fn unescape(s: &str) -> String {
        let mut t = String::with_capacity(s.len());
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\\' {
                match it.peek() {
                    Some('\r') | Some('\n') => {
                        if it.peek() == Some(&'\r') {
                            it.next();
                        }
                        if it.peek() == Some(&'\n') {
                            it.next();
                        }
                        while matches!(it.peek(), Some(' ' | '\t')) {
                            it.next();
                        }
                        continue;
                    }
                    _ => t.push(c),
                }
            } else {
                t.push(c);
            }
        }
        t.replace("\\\"", "\"")
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\r", "\r")
            .replace("\\\\", "\\")
    }

    /// Guard: every Indonesian user-surface literal resolves in EN.
    /// Add a dictionary key or a `tr_status` prefix — never extend the
    /// allowlist for chrome/status text.
    #[test]
    fn all_user_surface_templates_covered_in_english() {
        let en = Lang::En;
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut misses: Vec<String> = Vec::new();
        for rel in SURFACE_FILES {
            let text = std::fs::read_to_string(root.join(rel))
                .unwrap_or_else(|_| panic!("cannot read {}", rel));
            for lit in source_literals(&text) {
                let lit = unescape(&lit);
                if lit.chars().count() < 4 {
                    continue;
                }
                if !ID_MARKERS.iter().any(|m| lit.contains(m)) {
                    continue;
                }
                if COVERAGE_ALLOW.iter().any(|a| lit.contains(a)) {
                    continue;
                }
                if en.tr_status(&lit) == lit {
                    misses.push(format!("{}: {:?}", rel, lit));
                }
            }
        }
        assert!(
            misses.is_empty(),
            "Indonesian literals without EN coverage (add dict key/prefix):\n{}",
            misses.join("\n")
        );
    }

    /// Guard: every `lang.fN("template", …)` call site passes exactly as
    /// many args as the template (ID and EN) has `{}` slots. A mismatch
    /// would render a raw `{}` to the user instead of failing to compile.
    /// Templates identical in EN live in IDENTITY_TEMPLATES (reviewed).
    #[test]
    fn format_call_templates_match_arity() {
        const IDENTITY_TEMPLATES: &[&str] = &[
            // Pure numbers/symbols: nothing to translate.
            "{}/{} · {}%",
            // Version label: "v" + version + info glyph, identical in EN.
            "v{} · i",
        ];
        let en = Lang::En;
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src_root = root.join("src");
        let mut bad: Vec<String> = Vec::new();
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut stack = vec![src_root];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    files.push(p);
                }
            }
        }
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let text = match text.find("#[cfg(test)]") {
                Some(i) => text[..i].to_string(),
                None => text,
            };
            let b = text.as_bytes();
            let mut i = 0;
            while i + 4 < b.len() {
                if b[i] == b'.'
                    && b[i + 1] == b'f'
                    && b[i + 2].is_ascii_digit()
                    && b[i + 2] >= b'1'
                    && b[i + 2] <= b'4'
                    && b[i + 3] == b'('
                {
                    let n = (b[i + 2] - b'0') as usize;
                    let mut j = i + 4;
                    while j < b.len() && (b[j] == b' ' || b[j] == b'\n' || b[j] == b'\r' || b[j] == b'\t') {
                        j += 1;
                    }
                    if j < b.len() && b[j] == b'"' {
                        // Byte ranges + lossy decode: pushing bytes as chars
                        // would split multi-byte UTF-8 (… · ±).
                        let start = j + 1;
                        j += 1;
                        while j < b.len() && b[j] != b'"' {
                            if b[j] == b'\\' {
                                j += 1;
                            }
                            j += 1;
                        }
                        let tpl = unescape(&String::from_utf8_lossy(&b[start..j]));
                        // Skip i18n's own fN definitions (fn f1/f2/.. match
                        // the same shape but carry no template).
                        let ctx_start = text[..i].rfind("fn f").map(|k| k + 4).unwrap_or(0);
                        let is_def = text[ctx_start..i].trim().is_empty()
                            && text[..i].trim_end().ends_with("fn");
                        if !is_def {
                            let id_count = tpl.matches("{}").count();
                            if id_count != n {
                                bad.push(format!(
                                    "{}: f{} has {} slots: {:?}",
                                    path.strip_prefix(&root).unwrap().display(),
                                    n,
                                    id_count,
                                    tpl
                                ));
                            } else if en.tr(&tpl) == tpl && !IDENTITY_TEMPLATES.contains(&tpl.as_str()) {
                                // No dictionary arm: EN direct-render would
                                // leak Indonesian (fallback is silent).
                                bad.push(format!(
                                    "{}: f{} template has no EN arm: {:?}",
                                    path.strip_prefix(&root).unwrap().display(),
                                    n,
                                    tpl
                                ));
                            } else {
                                let ent = en.tr(&tpl);
                                if ent != tpl && ent.matches("{}").count() != n {
                                    bad.push(format!(
                                        "{}: EN translation arity drift: {:?} -> {:?}",
                                        path.strip_prefix(&root).unwrap().display(),
                                        tpl,
                                        ent
                                    ));
                                }
                            }
                        }
                        i = j + 1;
                        continue;
                    }
                }
                i += 1;
            }
        }
        assert!(
            bad.is_empty(),
            "fN/template arity mismatches (raw {{}} would reach the user):\n{}",
            bad.join("\n")
        );
    }
}
