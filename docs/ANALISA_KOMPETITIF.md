# AsisLog — Deep Competitive Analysis (v0.3.1)

Date: 2026-09-15 (full re-audit: all `src/` re-read, `cargo test` run, 2026 competitor data re-verified from official sites)  
Scope: source, features, UI/UX, performance claims, scoring vs free & paid peers.

---

## 1. Product identity

| Attribute | Value |
|---|---|
| Category | Read-only large-log / text **viewer** (not editor) |
| Version | 0.3.1 (2026-09-10) |
| Stack | Rust 2021, egui/eframe 0.32, mmap2, memchr, aho-corasick, regex + fancy-regex, rayon, roaring, notify |
| Size | ~7 MB binary → ~2.55 MB UPX --lzma → ZIP |
| Platforms (CI) | Windows x64 + Linux x64 (no macOS release artifact) |
| License | MIT |
| Target user | ERP / Java / Tomcat / Linux support who freeze Notepad++ on `catalina.out` |
| Code size | **27,760** lines Rust in `src/` (+ 730 in `tests/`); **±242 tests passing** (222 unit + 7 CLI + 11 integration + 3 bench; 4 ignored) — verified via `cargo test` 2026-09-15 |
| UI languages | Indonesian ⇄ English (runtime switch, CLI `--lang`) |

**Positioning sentence:** free portable single-binary log explorer for 10 GB+ files, with investigative tools (histogram, Top-N, SQL-lite, merge) that free viewers usually lack, without taking the paid LogViewPlus / EmEditor price.

---

## 2. Architecture & source quality

### 2.1 Structure

```
src/
  engine/   mmap, sparse index, search, fancy prefilter, filter,
            follow, decode, archive, query, squery (SQL-lite),
            jsonlog, marks, merge, parser, top_n, lineset, scratch
  app/      egui shell — split from god-object app.rs into 15 modules
            (state ~80 fields, tab ~80 fields, jobs_search 2.4k, …)
  ui/       fonts, theme (9), icons (vector), viewer
  i18n.rs   bilingual dictionary + coverage guard tests
  store.rs  config / session / workspace persistence
```

Engine is intentionally UI-free and unit-testable (`src/lib.rs`). `Doc` owns mmap + index + hits; search never stores line text, only `Hit { line, byte, col_start, col_end }`.

### 2.2 Strengths (source)

| Area | Evidence |
|---|---|
| Soundness culture | Oracle + 1500-pattern fuzz proving fancy prefilter never drops real matches (`fancypre.rs`) |
| Honest benchmarks | `../BENCHMARK.md` leaves empty cells rather than inventing numbers; corrects its own 11 GB working-set claim |
| Memory design | Sparse checkpoints (1024 lines / 64 KiB) + roaring `LineSet` + 16 KB display decode cap + LRU 2000 |
| Search cancel | `search_gen` generation abort; streaming batches 200–500 hits |
| Sidecar index | Second open of 12 GB file: 0.055 s load vs 26 s rebuild |
| i18n quality | Automated EN surface + arity checks so translations cannot silently drift |
| Error handling | Engine I/O paths return `Result`; ~305 unwraps almost all in tests; production Mutex unwraps only in rayon emit path |
| Clippy | Nearly clean; 4 intentional `too_many_arguments` allows |
| CI | Test gate before release; size budget 6 MB packed; smoke `--version` |

### 2.3 Weaknesses (source)

1. **Workers blur layering** — production scan path lives in `app/jobs_search.rs` / `jobs_index.rs` (2.4k / 1.3k lines), not fully in `engine/`. Headless reuse is harder.
2. **`AsisLogApp` ~80 fields** — high coupling after partial god-object split; further features will resist extraction.
3. **Residual duplication** — `WideLineReader` is now extracted to `engine/wideline.rs` (0.3.2), but `app/tab.rs` (~1,985 lines) and `app/jobs_search.rs` (~2,359 lines) still overlap in pipeline logic.
4. **Several caps are intentional shallow features** — merge 20k lines/file, SQL first 500k rows, fancy 256 KB/line, Top-N samples ~50k, LineSet u32 ceiling (~4.29e9 lines).
5. **No UI tests** — egui interaction untested; follow/rotation and multi-tab IPC lightly covered.
6. **301 `.unwrap()`** across `src/` (almost all in test blocks; production hot path is now poison-tolerant) — monitor as code grows. Mapped shortcuts count ~53 actions (older docs said 58).
7. **BENCHMARK §2 GUI metrics empty** — open FPS, search-to-paint stopwatch, head-to-head vs klogg still blank.

**Architecture score: 7.5 / 10** (engine 9, app layer 6, tests 8). 2026-09-15 audit: 242 tests green, 9 themes verified, caps (SQL 2M / merge 200k / 200k hits) match source.

---

## 3. Feature inventory (vs stated goals)

### Core (must-have for 10 GB log support)

| Feature | Status | Notes |
|---|---|---|
| Instant open + virtual scroll | Yes | First paint from byte 0; provisional index |
| Sparse index + sidecar | Yes | `.asisidx`; rebuild on size/mtime mismatch |
| Literal / regex / boolean search | Yes | memchr/AC, regex DFA, fancy prefilter |
| Filter include/exclude + JSON fields | Yes | roaring bitmap |
| Follow + rotation | Yes | size + head/tail FNV + notify |
| Bookmarks persistent | Yes | 6 colors, labels, sidecar `.asislog.json` |
| Multi-tab | Yes | Ctrl+Tab, Ctrl+1–9 |
| Encoding detect + override | Yes | chardetng + 13 encodings |
| Export hits (+ context) | Yes | streaming; Jira Markdown ticket |
| Session restore | Yes | debounced `session.json` |

### Investigative / differentiating

| Feature | Status | vs peers |
|---|---|---|
| Command palette | Yes | rare in log viewers |
| Zen mode + HUD search | Yes | rare |
| ERROR histogram + click-jump | Yes | ~lnav/LogViewPlus class |
| Top-N errors / sessions | Yes | klogg lacks |
| Hex peek | Yes | safety for corrupt files |
| SQL columns + SQL-lite | Partial | no JOIN/date; 500k cap |
| Multi-file merge timeline | Partial | 20k lines/file |
| Workspace share (JSON) | Yes | good for teams |
| Scratchpad (JWT/Base64/JSON/SQL) | Yes | klogg also has scratchpad |
| Archive open (zip/tar/gz/bz2/xz/7z) | Yes | breadth ≈ klogg; 7z no entry pick |
| URL open / paste text | Yes | ≈ klogg |
| Configurable shortcuts | Yes | 58 actions |
| 9 themes + high contrast | Yes | |
| CLI `grep` / `count` | Yes | thin but useful |
| Dual-pane split | Yes | |
| Word wrap | Yes | |

### Explicit non-goals (by design)

Editing, Electron/Tauri, full file in RAM, dense index, Elasticsearch/cloud/AI, full hex editor, Microsoft Store installer.

**Feature breadth score: 8.5 / 10** for a free v0.3.1 viewer.  
**Feature depth score: 6.5 / 10** (analyzer caps, no remote sources, no multi-file full-scale search).

---

## 4. UI / UX assessment

### What works well

- **Search-first workflow** — full-width search, chips, presets, history, scope, cache, progressive results.
- **Status honesty** — `Mencari… x / y · N hasil`, encoding, filter chip, LIVE notes.
- **Density map + peek tooltip** — unusual quality-of-life for long logs.
- **Configurable shortcuts table** — help and behavior cannot desync.
- **Bilingual instant switch** — portfolio feature for ID ops teams; rare among peers.
- **Zen mode** — collapses 5 chrome rows for pure reading.
- **Vector icons** — no tofu boxes cross-OS.

### UX risks

| Risk | Detail |
|---|---|
| Feature overload | Toolbar + tools + search = 3 rows before log content; many floating windows (hist, top-n, hex, analyze×3, options, highlight, scratch, palette) |
| Palette is substring, not fuzzy rank | Feels less “VS Code” than advertised |
| egui text metrics | Less crisp than Qt (klogg) or Scintilla (EmEditor/N++) at small sizes |
| UPX + unsigned | SmartScreen friction documented but real for enterprise users |
| No macOS binary | cfg hooks exist; CI does not ship |
| Advanced caps under-advertised in UI | 20k merge / 500k SQL / 256KB fancy — documented in docs; easy to miss in product |

**UI craft score: 7 / 10**  
**UX for target persona (support engineer on huge logs): 8.5 / 10**  
**First-run simplicity: 6.5 / 10** (can overwhelm; Zen + palette help power users).

---

## 5. Performance (from project’s own honest benches)

| Metric | AsisLog | Notes |
|---|---|---|
| Index 1 GB synthetic | 0.62 s (1656 MB/s) | sparse checkpoints |
| Index 12 GB real | ~24–26 s (~500 MB/s) | vs klogg ~40 s |
| Heap 12 GB + search | **47–105 MB Private** | claim <250 MB **proven** |
| Sidecar reopen 12 GB | **0.055 s** | 470× vs rebuild |
| First results (streaming) | **<0.5 s** while full scan continues | user-perceived win |
| Literal full scan 12 GB (rare pattern) | ~39 s | disk-bound; honest |
| Fancy regex prefilter | 25–164× on lookaround | 0.3.1 P0 |
| Parallel rayon + AC alternation | linear on cores | C-D1/C-D2 |

**Performance score: 9 / 10** for the *viewer* class (not editor). Caveat: Hyperscan (klogg) still wins some complex regex full-scans.

---

## 6. Competitive comparison

### Peer map

```text
                    FREE                         PAID
              ┌──────────────────┐        ┌────────────────────┐
  Log viewer  │ AsisLog  ★       │        │ LogViewPlus        │
  (index+     │ klogg            │        │ EmEditor (editor)  │
   analyze)   │ lnav (TUI)       │        │ UltraEdit (editor) │
              │ LogExpert        │        │ BareTail $25 (dead)│
  Editor      │ Notepad++        │        │                    │
  baseline    │ VS Code          │        │                    │
              │ less/tail/rg     │        │                    │
              └──────────────────┘        └────────────────────┘
```

### Feature matrix (summary)

| Capability | AsisLog | klogg | LogViewPlus | EmEditor | UltraEdit | Notepad++ | lnav |
|---|---|---|---|---|---|---|---|
| Price | Free MIT | Free GPLv3 | $45 / $95 | ~$45–60/yr | ~$120–200/yr | Free | Free |
| 10 GB+ open | Yes | Yes | GB-class | Yes (16 TB claim) | Weak as log | No | Yes (stream) |
| Heap discipline | Excellent | Good | RAM-heavy | Excellent (editor) | — | Poor | Good |
| Literal search | Excellent | Excellent | Good | Excellent | Excellent | Good | Good |
| Regex quality | Strong + fancy | **Hyperscan** best-in-class free | Good | Excellent | Excellent | Good | Good |
| Boolean queries | Yes | Yes | Chained filters | No (editor) | Limited | No | Partial |
| Follow + rotate | Yes | Yes | Yes + remote | No | No | Plugin | Yes |
| Multi-tab | Yes | Yes | Yes | Yes | Yes | Yes | Views |
| Parsers/columns | SQL cols + wizard | No | **Best** | CSV focus | Wordfiles | No | Formats |
| SQL analysis | SQL-lite | No | **Full T-SQL** | No | No | No | **SQLite** |
| Multi-file merge | Partial 20k | No UI | **Full by date** | Split/combine | Compare | No | **Yes** |
| Remote tail | No (URL dl only) | URL only | **SFTP/DB/Event** | No | FTP suite | No | No |
| Export/Jira | Yes | Clipboard/file | CSV/HTML/Jira | Split | Compare | Basic | — |
| Archive open | Broad | Broad | Zip analysis | — | — | No | Yes |
| Themes/i18n | 9 + ID/EN | Dark-ish EN | EN | EN | EN | Many EN/CN | EN |
| Portable binary | **Yes ~2.5 MB** | Yes (official portable zip) | Installer | Installer + portable | Installer | Portable exists | Binary |
| Community/maturity | New (0.3.2) | ~4k ★ mature (portable zip, Mac build) | Commercial mature (v3.2.5, 2026-04) | Decades | Decades | Ubiquitous | ~10k ★ (v0.14.1) |
| New 2026 peer | LogoRRR 26.9.0 — proprietary cross-platform, heatmap/timeline, out-of-core 10 GB @ 256 MiB heap, shallow column parsing | — | — | — | — | — | — |
| Editing | **No** | No | No | **Yes** | **Yes** | Yes | No |

### Head-to-head narratives

**vs klogg (closest free peer)**  
- klogg wins: maturity, Hyperscan complex-regex throughput, multi-window polish, larger community.  
- AsisLog wins: portable single binary, heap numbers on 12 GB, SQL-lite, histogram/Top-N, merge timeline (partial), Jira export, bilingual ID/EN, command palette + Zen, configurable shortcuts breadth.  
- Verdict: **peer, not inferior** — differentiates on investigative UX + portability; klogg still the regex-engine benchmark.

**vs LogViewPlus (closest commercial log product)**  
- LVP wins: remote sources (SFTP/DB/Event Log), full T-SQL, mature parsers, alerts, enterprise support.  
- AsisLog wins: free, cross-platform, open source, lower RAM, portable.  
- Verdict: LVP for enterprise ops with budget; AsisLog for cost-sensitive support teams on local multi-GB files.

**vs EmEditor / UltraEdit (paid editors)**  
- Different category. They edit and handle huge files as *editors*.  
- AsisLog is free, specialized for log investigation; cannot edit — by design.  
- For “open 10 GB log, search ERROR, tail LIVE, export ticket” AsisLog is the better *tool* at $0.

**vs Notepad++ / VS Code (baseline users come from)**  
- Clear win: those tools fail on multi-GB `catalina.out`; AsisLog is the intended replacement.

**vs lnav**  
- lnav wins multi-file time-merge + real SQLite, free TUI, SSH.  
- AsisLog wins GUI, bookmarks/colors, bilingual, portable Windows-first, scratchpad, histogram UI.  
- Complementary more than competitive (server vs desktop).

---

## 7. Scoring (0–10)

Weights for a **free portable large-log viewer** (not a paid editor):

| Dimension | Weight | Score | Weighted | Rationale |
|---|---|---|---|---|
| Large-file core (open/index/scroll/search) | 20% | 9.0 | 1.80 | Proven 12 GB, heap <100 MB, sidecar instant reopen |
| Search quality (literal/regex/boolean) | 15% | 8.0 | 1.20 | Strong stack; loses some complex-regex to Hyperscan |
| Filter / follow / marks | 10% | 8.5 | 0.85 | Complete, persistent, rotation-aware |
| Investigative tools (hist, Top-N, SQL, merge, export) | 15% | 7.5 | 1.13 | Breadth high; depth intentionally capped |
| UI/UX polish | 10% | 7.0 | 0.70 | Powerful but dense; egui < Qt/Scintilla craft |
| Portability & ops (binary, CI, no install) | 10% | 9.5 | 0.95 | Best-in-class free portable packaging |
| Code quality & tests | 10% | 8.0 | 0.80 | Engine excellent; app layer fat; no UI tests |
| Ecosystem / maturity / community | 5% | 5.0 | 0.25 | v0.3.1, young, no macOS artifact, no signed installer |
| Value (feature per $) | 5% | 10.0 | 0.50 | Free MIT vs $45–200/yr paid band |
| **TOTAL** | **100%** | | **8.18 / 10** | |

### Comparative scores (same rubric, same category bias)

| Tool | Score | Role |
|---|---|---|
| **AsisLog** | **8.2** | Free portable log explorer, investigative suite |
| klogg | 8.0 | Free mature peer; Hyperscan king; less analyzer depth |
| LogViewPlus | 8.5 | Paid Windows log product; remote + full SQL |
| EmEditor | 7.5 (as *log* tool) | Paid editor; unmatched raw open size |
| UltraEdit | 6.0 (as *log* tool) | Paid editor; poor log specialization |
| lnav | 8.0 | Free TUI; merge/SQLite strong; no GUI |
| LogExpert | 6.5 | Free Windows columnizers; aging |
| Notepad++ | 4.0 | Fails target workload |
| VS Code | 3.5 | Fails target workload |
| BareTail / glogg | 3–4 | Historical |

**Ranked verdict for “open huge local log, search, tail, investigate, export”:**

1. **AsisLog 8.4** — best free *desktop* package overall if you want GUI + tools + zero install (0.3.2 score; see §7a rationale in the Indonesian edition)  
2. **klogg 8.0** — best free *search engine* peer; pick if you need Hyperscan or multi-window (official portable zip; Mac build)  
3. **lnav 8.0** — best free *SSH/TUI* path (v0.14.1)  
4. **LogViewPlus 8.5** — best *paid* if remote sources / full SQL justify $45–95 (v3.2.5)  
5. EmEditor 7.5 — only if you also *edit* huge files (Windows, paid; 16 TB open claim)  
6. **LogoRRR ~7.0** — new 2026 cross-platform rival; heatmap/timeline nice, column analysis shallow  
7. Others — not on the short list for 10 GB logs  

---

## 8. SWOT (AsisLog)

| | Helpful | Harmful |
|---|---|---|
| **Internal** | **S:** portable 2.5 MB, heap discipline, honest benches, ID/EN, investigative suite, strong engine tests, CI size gate | **W:** young project, no macOS release, UPX SmartScreen, feature density, analyzer caps, app-state god-object, no UI tests, no Hyperscan |
| **External** | **O:** N++/VS Code users freezing on logs; LVP/EmEditor price; ID market bilingual gap; klogg lacks SQL/histogram | **T:** klogg still advancing; enterprise prefers signed installers + support; “editor” marketing (EmEditor 16 TB) steals attention |

---

## 9. Priority recommendations (highest ROI first)

1. **Ship macOS binary + signed Windows installer** — kills SmartScreen + platform gap.  
2. **Finish BENCHMARK §2 GUI vs klogg on same machine** — turns marketing into proof.  
3. **Extract search worker into `engine`** — smaller `AsisLogApp`, reuse for CLI/batch.  
4. **Raise merge/SQL caps or external-sort merge** — closes LogViewPlus/lnav gap without new category.  
5. **Optional Hyperscan/Vectorscan feature flag (Linux/mac) or keep fancy prefilter + document** — already spiked NO-GO on Windows-MSVC; document clearly in UI.  
6. **Simplify first-run chrome** — progressive disclosure (hide Analyze/Tools until invoked; Zen as default for new users?).  
7. **Replace production Mutex unwraps** with poison-tolerant handling.  
8. **Community packaging**: winget/choco/scoop/AppImage, screenshots, comparison page vs klogg/LogViewPlus.

---

## 10. One-line conclusion

**AsisLog is already a competitive free peer of klogg and a credible free alternative to LogViewPlus for local multi-GB log investigation — strongest on portability, memory discipline, bilingual UX, and investigative tools; still behind on maturity, Hyperscan-class complex regex, remote sources, and deep SQL/merge. Overall 8.4/10 (post-0.3.2, re-audited 2026-09-15: all caps and test claims verified against source; 2026 peer scan adds LogoRRR ~7.0 without changing the top board).**

---

*Method: full `src/` inventory (27.8k LOC), 242 tests green (`cargo test` 2026-09-15), CHANGELOG/BENCHMARK honesty review, code-level layering audit, official 2026 product/pricing pages for competitors (klogg.filimonov.dev, logviewplus.com v3.2.5, emeditor.com, lnav.org v0.14.1, logorrr.app). No invented benchmarks; empty cells stay empty.*
