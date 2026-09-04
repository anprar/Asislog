# Prompt: Build AsisLog

Copy everything below the line into a coding agent (Cursor, Claude Code, Codex, Aider, etc.). The agent must implement the app, not write a design essay.

---

## Identity

Build **AsisLog**, a portable desktop log/text viewer for huge files (several GB, target 10 GB+). It is a **viewer**, never an editor.

Users are ERP / Java / Tomcat / Linux support people who currently freeze Notepad++ on `catalina.out` and similar `.log` / `.txt` files. They need instant open, fast search, filter, follow, and jump-to-match with low RAM.

UI language: **Indonesian** (menus, buttons, status, errors). Code, comments, crate names, and commit messages: **English**.

Window title: `AsisLog`. Executable name: `asislog` / `asislog.exe`.

## Non-goals (do not implement)

- Editing, save-as of the original file, undo
- Electron, Tauri, WebView, DOM virtual lists
- Loading the whole file into a `String` / `Vec<u8>` copy
- Dense index of every line offset if that would cost hundreds of MB
- Elasticsearch, database, AI, cloud
- Full hex editor, minimap of entire file, JSON AST of whole file
- Syntax highlighting of off-screen lines
- Installer / Microsoft Store packaging in MVP

## Stack (mandatory)

- Rust, edition 2021+, `cargo build --release` produces a **single portable binary**
- UI: `eframe` + `egui` (immediate mode). No web frontend.
- I/O: memory-map the file (`memmap2`). OS pages the bytes; do not `read()` the whole file.
- Search: `memchr` / `aho-corasick` for literals; `regex` only when user enables Regex.
- Optional: `rayon` for chunked search, `notify` or size-poll for follow, `encoding_rs` for decode.

If a crate fights mmap/zero-copy, replace the crate. Do not “just collect all lines into Vec<String>”.

## Architecture

One process, two logical parts:

1. **Engine (sync-safe, background threads)**
   - `Doc`: mmap handle, file size, mtime, encoding, sparse checkpoints, line_count estimate, follow state
   - `checkpoints: Vec<(line_u64, byte_u64)>` every **1024 lines or 64 KiB**, whichever comes first
   - Never store full text of hits. `Hit { line, byte, col_start, col_end }`
   - `filter_map: Vec<u64>` maps view-row → original line number
   - `search_gen: u64` so old search jobs abort when the query changes
   - Viewport API only: `line_at(line) -> byte range`, `lines(start, count) -> Vec<LineView>`
   - Decode **only visible lines** (lossy OK). Invalid UTF-8 → replacement char, never panic

2. **UI (egui)**
   - Each frame paints ~50–100 visible rows at **fixed row height**, no wrap in MVP
   - Virtual scroll: `scroll_offset` → line or byte. Do not instantiate widgets for 50 million lines
   - Gutter shows **original line number**
   - Main view + results pane split
   - Status bar: index %, line count, file size, match count, RAM if cheap to read, encoding, follow on/off

Open must be instant: show the first screen from byte 0 while indexing runs. Allow jump toward EOF using `byte ≈ size * scrollbar` before the index is complete.

## Data / file rules

- Original log is **read-only**
- Optional sidecar `filename.log.asisidx` (version, size, mtime, checkpoints). Rebuild if size/mtime mismatch. Sidecar may be deleted by the user
- Clipboard copy cap **16 MB**. Larger selection → “Simpan ke file…”
- Tabs: one `Doc` per tab. Global LRU of decoded lines (~2000 entries)

Encoding detection: BOM first, then 64 KiB sample. Support UTF-8, Windows-1252 / Latin-1 fallback, UTF-16 LE/BE if BOM present. Let user override from toolbar.

## UI layout (Indonesian labels)

```
AsisLog  —  catalina.out
[ Buka ] [ Encoding ▼ ] [ Ikuti ] [ Regex ] [ Huruf besar/kecil ]
[ Cari  ________________________________  ]  [ Prev ] [ Next ]  234 hasil
┌ Viewport log (monospace, no wrap) ──────────────────────────┐
│ 1842291  2026-09-03 13:41:02 ERROR OrderService ...         │
└─────────────────────────────────────────────────────────────┘
┌ Hasil pencarian / filter ───────────────────────────────────┘
│ #12  baris 1842291  ERROR  NullPointerException             │
└─────────────────────────────────────────────────────────────┘
Indeks 100% · 48,2 juta baris · 9,7 GB · 234 hasil · Ikuti mati
```

Shortcuts:

- `Ctrl+O` buka
- `Ctrl+F` fokus cari
- `F3` / `Shift+F3` next/prev hit
- `Ctrl+G` ke baris
- `Ctrl+End` / `Ctrl+Home`
- `Ctrl+Shift+F` toggle Ikuti (follow)
- `Esc` batal pencarian
- Click a result row → scroll main view to that line, highlight match, show ±20 lines of context

Dark-first, monospace log view. Highlight **only on-screen** lines:

- `ERROR` / `FATAL` / `Exception` / `Caused by:` → red
- `WARN` / `WARNING` → yellow
- `INFO` → dim
- Current match → stronger background

## Features and exact logic

### 1. Open / close / tabs
- Native file dialog + drag-drop of `.log` `.txt` `.out` `.err`
- mmap, start indexer thread, render immediately
- If file cannot be mapped, show Indonesian error, do not crash
- Multiple tabs; closing a tab drops mmap and cancels its jobs

### 2. Sparse index
- Background thread scans for `\n`
- Handle `\r\n` and final line without newline
- Emit progress every ~50 ms to UI
- `goto_line(n)`: binary search checkpoints, then sequential scan
- `goto_percent(p)`: use line count if ready, else byte offset then snap to line start
- Persist sidecar when index completes

### 3. Search (does not hide lines)
- Debounce input ~150 ms
- Literal path default (SIMD/memchr). Regex path only if checkbox on; invalid regex → status error, no panic
- Scan 4–8 MiB chunks; stream batches of 200–500 hits
- Results pane fills incrementally
- Next/prev walk the hit list; do not rescan the file
- Changing query increments `search_gen` and drops old results

### 4. Filter (hides non-matching lines in main view)
- Separate from search, or “Terapkan sebagai filter” on current query
- Support simple tokens and optional `-` prefix for exclude (e.g. `ERROR -DEBUG`)
- Rebuild `filter_map`; gutter still original line numbers
- Empty filter = show all
- Filter does not copy file bytes

### 5. Follow / tail
- Poll size ~300–500 ms or filesystem watch
- Size increase → index only the new tail
- Size decrease or identity change → treat as rotation, reopen from start, warn in status
- Auto-scroll only if `stick_bottom == true` (user was at EOF). Scrolling up clears stick. Manual “Ikuti” re-enables it

### 6. Bookmarks
- Toggle bookmark on current line
- Side list: label + line number; click jumps via stored byte if still valid else re-resolve line
- Do not write into the log file. Optional tiny sidecar later; memory-only is OK for MVP

### 7. Selection / copy / export
- Drag selects line range by bytes
- Copy = decode that range, cap 16 MB
- Export: (a) hits only (b) hits + N context lines (default 10) streaming to a new file
- Never rewrite the source log

### 8. Go to
- Dialog: line number, or `50%`, or timestamp string if a timestamp-looking prefix exists on sampled lines
- Timestamp jump: if first/last parsed times are monotonic, binary-search lines; else linear search with timeout and partial result

## Performance contracts (acceptance)

Treat these as tests the agent should actually try to meet on a generated ~1–2 GB file, and design so 10 GB stays the same shape:

- Open first paint: well under 1 s for a local SSD file regardless of size
- RAM: far below file size; design budget **< 250 MB** for a 10 GB log with search hits in the thousands (not millions of stored strings)
- Scroll: no per-frame full-file scan
- Search: first batch of hits appears without freezing UI; cancel works
- `cargo build --release` yields one binary; running it does not require Rust, Python, or Node

Add a `tests/` or `engine` unit tests for: newline indexing on `\n` and `\r\n`, checkpoint lookup, search cancel generation, filter exclude token, follow truncate/rotate.

## Project layout

```
asislog/
  Cargo.toml
  README.md          # Indonesian short usage + how to build portable
  src/
    main.rs          # eframe start
    app.rs           # egui app, tabs, shortcuts
    engine/
      mod.rs
      mmap.rs
      index.rs
      search.rs
      filter.rs
      follow.rs
      decode.rs
    ui/
      mod.rs
      viewer.rs
      results.rs
      status.rs
      dialogs.rs
  .github/workflows/release.yml
```

`release.yml`: build `--release` for `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-gnu` (Linux musl if easy). Upload artifacts named `asislog-windows.exe` and `asislog-linux`.

`Cargo.toml` package name `asislog`, version `0.1.0`.

README must include:

```
cargo build --release
# Windows: target/release/asislog.exe
# Linux:   target/release/asislog
```

User does not install Rust to **run** the binary.

## Implementation order (do this order, keep the app runnable after each step)

1. Skeleton eframe window titled AsisLog, Indonesian empty state “Buka file log…”
2. mmap + paint first N visible lines, virtual scroll by byte
3. Sparse indexer + line gutter + goto line + status progress
4. Search literal + results pane + F3
5. Regex option + cancel
6. Filter + exclude token
7. Follow/tail + stick-to-bottom
8. Highlight rules on viewport, bookmarks, copy/export
9. Drag-drop, encoding override, sidecar index, CI workflow

Do not stall on polish before step 4 works.

## Coding rules

- No unwrap on I/O in release paths; map to Indonesian `Status` / modal
- No cloning the mmap into UI
- No `include_str!` of sample 10 GB files
- Keep modules small; engine must be testable without opening a window
- Prefer clear names: `AsisLogApp`, `Doc`, `Indexer`, `SearchJob`
- If something is slow, fix the engine; do not add a loading spinner over a full-file read

## Definition of done

- `cargo test` passes
- `cargo build --release` produces portable AsisLog
- Can open a multi-hundred-MB / GB text file, scroll, search, filter, follow, jump to a hit, copy a small selection, without loading the file as one string
- UI in Indonesian, binary name AsisLog

Start implementing now. After the skeleton compiles, continue through the implementation order until search + virtual scroll work, then the rest.

---
