# Changelog

All notable changes to AsisLog are documented here, newest first.
Dates are YYYY-MM-DD. Performance claims always reference measured
numbers in BENCHMARK.md.

## [0.4.0] — 2026-09-16

Performance indexing, honest binary handling, exact search totals, and
scroll/window fixes. Verified against a 12 GB / 338M-line production log.

Fixed

- Startup window now reliably opens maximized on Windows. Previous
  releases could start at a small default size until minimized and
  restored; the conflicting default size was removed and the maximize
  request is re-asserted during the first frames (manual un-maximize
  is still respected).
- Scrolling no longer freezes when follow (LIVE) mode is pinned at the
  bottom. Previously every scroll input near the end of the file snapped
  back, making wheel, scrollbar, and keyboard appear dead. The bottom pin
  now engages only while new rows arrive; any manual scroll releases it.
- Follow tail line counts are now exact for every append shape (clean
  appends previously undercounted by one; fresh partial tails
  overcounted, proven by a full-scan oracle test).
- Mouse wheel no longer double-applies from smoothed echo events.

Added

- Binary file detection. Files with dense NUL bytes (for example
  database blobs misnamed as .log) are refused honestly by the count
  and grep commands with a clear message, and open in the GUI with a
  warning that points to Hex Peek instead of showing meaningless
  line counts.
- Paged search results. When matches exceed the display cap, a "next
  page" button (and F3 at the last stored hit) loads the next page in
  order with bounded memory, until the exact known total is fully
  explorable. Completing the set un-truncates and becomes cacheable.
- Parallel literal grep command. Full-file counts and matches now use
  all CPU cores with byte-identical semantics to the old loop
  (per-line matching, CRLF aware, proven by an oracle test).

Improved

- Parallel indexing: a full index of the 12 GB reference file dropped
  from about 22 seconds to about 9 seconds on a 6-core machine, with
  byte-identical checkpoints (see BENCHMARK.md section 13).
- Search results show the exact total match count in a single pass
  (for example 15,257,710 matches) while display memory stays capped;
  export-all remains unlimited via streaming export.
- CLI output is explicitly flushed, and the README documents the
  correct PowerShell invocation patterns for GUI-subsystem apps.
- Headless GUI measurements: open plus index of a 50 MB file in
  40 ms, first content frame in 2 ms, 25,000-match search done in
  31 ms (see BENCHMARK.md section 14; klogg column left empty until
  measured on the same file).

## [0.3.2] — 2026-09-12

Interface clarity, deeper analyzer caps, and first-run simplification.

Changed

- "Zen mode" renamed to Full Screen everywhere user-visible (F11,
  palette, status, help). Stored config keys unchanged, so existing
  configurations keep working.
- SQL-lite scan cap raised from 500k to 2M rows; timeline merge raised
  from 20k to 200k rows per file.
- Tools (bookmarks, highlights, notes, analysis) moved into a single
  Tools menu so first-run chrome stays slim; new empty state with
  drag-drop hints and basic shortcuts.

Fixed

- Parallel search emitter no longer panics on lock poisoning in the
  hot path.
- Removed a duplicated UTF-16 line reader shared by search and filter
  workers, plus UTF-16 carry tests.

## [0.3.1] — 2026-09-10

Complex regex support (look-around, backreferences) without hangs.

Added

- Required-literal prefilter: lines that cannot match skip decoding
  and backtracking via fast byte scanning. Measured on a 210 MB
  synthetic log: look-around patterns 12–59 s down to under 0.5 s
  with identical hit counts (see BENCHMARK.md section 12).
- Explicit backtrack limit per line: pathological lines report an
  error instead of hanging; search cancel stays per-line.

Noted limits

- Complex patterns scan at most 256 KB per line in search (display
  shows 16 KB; copy and export stay complete). Streaming export has
  no cap: export never loses rows.
- Hardware regex acceleration (Hyperscan/Vectorscan) evaluated and
  rejected for Windows MSVC builds (requires a C++ port).

## [0.3.0] — 2026-09-09

Log analysis panel: parser, SQL-lite, and multi-log merge.

Added

- Parser tab: automatic format detection (JSON, SQL-like columns,
  generic), four built-in presets, a regex wizard with match preview,
  and custom parsers and columns stored in the config file.
- SQL-lite tab: SELECT with WHERE, GROUP BY, ORDER BY count, LIMIT,
  Top-N bar chart, CSV export. Honest limit: first 2M rows of the
  active tab. Not full Transact-SQL (no JOINs or date functions).
- Merge tab: one-click time-sorted timeline across all tabs, server
  skew report, cross-tab pattern search, combined export.
- Full bilingual UI for every new string.

## [0.2.1] — 2026-09-07

Archive parity, investigation tools, and portable release.

Added

- Archive support matching klogg: zip, tar.gz, gz, bz2, xz, 7z with
  magic-byte sniffing and per-format benchmarks.
- Investigation views: per-minute ERROR histogram, Top-N errors and
  sessions, hex peek for binary files, SQL column view, density map
  with hover preview.
- Dual-pane split view, pinnable result snapshots, configurable
  shortcuts with editor, system fonts, full bilingual UI
  (Indonesian/English, menus, palette, CLI language flag), nine
  themes, full-screen mode with search HUD, scratchpad decoders,
  shareable workspaces, Markdown tickets.
- Portable release: single binary compressed with UPX, ZIP archive
  and SHA-256 sums via CI.

## [0.1.0] — 2026-09-04

Initial release: read-only log viewer for 10 GB+ files, proven on a
12 GB / 338M-line file with under 110 MB heap. Sparse index with
sidecar files for instant reopen, literal/regex/boolean search,
filter, LIVE follow with rotation handling, persistent bookmarks,
automatic encoding detection with override, sessions, history,
favorites, count/grep/version CLI, single portable binaries for
Windows and Linux.
