// English comments: i18n Language switch (Indonesian / English), portable, no installer.
// UI default is Indonesian (backward compatible). English via toolbar switch,
// persisted in config.json as "id" / "en".

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
    pub fn tr(self, id: &str) -> &str {
        if self == Lang::Id {
            return id;
        }
        match id {
            // ---- toolbar / chrome ----
            "Buka" => "Open",
            "Arsip" => "Archive",
            "Semua" => "All",
            "Riwayat v" => "History v",
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
            "URL/teks v" => "URL/text v",
            "Buka URL…" => "Open URL…",
            "Tempel teks…" => "Paste text…",
            "Workspace v" => "Workspace v",
            "Simpan workspace…" => "Save workspace…",
            "Buka workspace…" => "Open workspace…",
            "Ganti tab v" => "Switch tab v",
            "Keluar Zen (F11)" => "Exit Zen (F11)",
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
            "Mode Zen: sembunyikan 5 baris kontrol ke 1 baris ramping (F11)" => {
                "Zen mode: collapse 5 control rows into 1 slim row (F11)"
            }
            "Cari (Ctrl+F)" => "Search (Ctrl+F)",
            "Palet (Ctrl+Shift+P)" => "Palette (Ctrl+Shift+P)",
            "Palet" => "Palette",
            "Zen (F11)" => "Zen (F11)",
            "Tema" => "Theme",
            "Bahasa" => "Language",
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
            "Preset v" => "Presets v",
            "Bawaan:" => "Built-in:",
            "Belum ada simpanan." => "No saved presets.",
            "Hapus preset" => "Delete preset",
            "Simpan pencarian saat ini…" => "Save current search…",
            "Cari teks, exception, request ID, atau regex…" => {
                "Search text, exception, request ID, or regex…"
            }
            "Peka huruf besar/kecil (Alt+C)" => "Case sensitive (Alt+C)",
            "Perlakukan query sebagai regex (Alt+R)" => "Treat query as regex (Alt+R)",
            "Bersihkan pencarian (Esc)" => "Clear search (Esc)",
            "Hasil sebelumnya (Shift+F3)" => "Previous result (Shift+F3)",
            "Hasil berikutnya (F3)" => "Next result (F3)",
            "Mencari… {} / {} · {} hasil" => "Searching… {} / {} · {} results",
            "Mencari… {} hasil" => "Searching… {} results",
            "Tidak ada kecocokan \"{}\"" => "No matches for \"{}\"",
            "{} hasil ({})" => "{} results ({})",
            "(dibatasi 200 rb)" => "(limited to 200k)",
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
            "Pencarian (Zen)" => "Search (Zen)",
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
            "Salin v" => "Copy v",
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
            "Salin blok v" => "Copy block v",
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
            "Mode Zen aktif (F11 untuk kembali)." => "Zen mode on (F11 to exit).",
            "Mode Zen dinonaktifkan." => "Zen mode off.",
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
            "Investigasi" => "Investigate",
            "Bantuan" => "Help",
            "Jadikan filter pencarian" => "Make search filter",
            // ---- palette titles (Buka file log… already above) ----
            "Fokus pencarian" => "Focus search",
            "Toggle Mode Zen (Kepadatan)" => "Toggle Zen mode (density)",
            "Toggle Ikuti log (LIVE)" => "Toggle follow log (LIVE)",
            "Ke baris / cap waktu…" => "Go to line / timestamp…",
            "Ekspor hasil pencarian…" => "Export search results…",
            "Tiket Markdown Jira (1-klik)" => "Jira Markdown ticket (1-click)",
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
            ("Mode Zen aktif", "Zen mode on"),
            ("Mode Zen dinonaktifkan", "Zen mode off"),
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
    ("Mengekspor {} / {} hasil…", "Exporting {} / {} results…"),
    ("Penanda baris {} akan dihapus permanen (tak bisa dibatalkan).",
     "Bookmark on line {} will be permanently deleted (cannot be undone)."),
    ("Penanda ditambahkan di baris {}.", "Bookmark added on line {}."),
    ("Tiket disimpan ({} hasil): {}", "Ticket saved ({} results): {}"),
];

/// Match `msg` against one ID template; on success rebuild the EN
/// template with captures in order. Literals match exactly (anchored).
fn try_template(id_tpl: &str, en_tpl: &str, msg: &str) -> Option<String> {
    let re = template_regex(id_tpl);
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
    for (id_tpl, en_tpl) in TEMPLATE_PAIRS {
        if let Some(out) = try_template(id_tpl, en_tpl, msg) {
            return Some(out);
        }
    }
    None
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
    fn quick_set_and_font_names() {
        assert_eq!(Lang::Id.quick_set_name(), "Cepat");
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
        "src/engine/decode.rs",
        "src/engine/follow.rs",
        "src/store.rs",
        "src/app/actions.rs",
        "src/app/tab.rs",
        "src/app/jobs_index.rs",
        "src/app/jobs_search.rs",
        "src/app/state.rs",
        "src/app/ui.rs",
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
            // Raw strings r".." / r#".."# (byte strings b".." handled below).
            if (c == 'r' || c == 'b')
                && i + 1 < b.len()
                && (b[i + 1] == b'"' || b[i + 1] == b'#')
            {
                let is_byte = c == 'b';
                let mut j = i + 1;
                let mut hashes = 0;
                while j < b.len() && b[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' && !(is_byte && hashes > 0) {
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
                            std::iter::once(b'"').chain(std::iter::repeat(b'#').take(hashes)).collect();
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
