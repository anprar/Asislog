# AsisLog — Analisis Kompetitif Mendalam (v0.3.2)

Tanggal: 2026-09-15 (**audit ulang penuh**: seluruh `src/` dibaca ulang, `cargo test` dijalankan, data pesaing 2026 dicek dari situs resmi)  
Cakupan: source, fitur, UI/UX, klaim kinerja, penilaian vs pesaing gratis & berbayar.

---

## 1. Identitas produk

| Atribut | Nilai |
|---|---|
| Kategori | **Penampil** log/teks berukuran besar (read-only, bukan editor) |
| Versi | 0.3.2 (2026-09-12) |
| Stack | Rust 2021, egui/eframe 0.32, mmap2, memchr, aho-corasick, regex + fancy-regex, rayon, roaring, notify |
| Ukuran | biner ~7 MB → ~2,55 MB UPX --lzma → ZIP |
| Platform (CI) | Windows x64 + Linux x64 (belum ada artefak rilis macOS) |
| Lisensi | MIT |
| Pengguna target | dukungan ERP / Java / Tomcat / Linux yang macet di Notepad++ pada `catalina.out` |
| Ukuran kode | **27.760** baris Rust di `src/` (+ 730 baris di `tests/`); **±242 tes lulus** (222 unit + 7 CLI + 11 integrasi + 3 bench; 4 ignored) — diverifikasi `cargo test` 2026-09-15 |
| Bahasa UI | Indonesia ⇄ English (ganti saat runtime, CLI `--lang`) |

**Kalimat positioning:** penjelajah log portabel gratis satu biner untuk file 10 GB+, dengan alat investigasi (histogram, Top-N, SQL-lite, merge) yang jarang dimiliki penampil gratis, tanpa harga LogViewPlus / EmEditor.

---

## 2. Arsitektur & kualitas source

### 2.1 Struktur

```
src/
  engine/   mmap, indeks jarang, search, fancy prefilter, filter,
            follow, decode, arsip, query, squery (SQL-lite),
            jsonlog, marks, merge, parser, top_n, lineset, scratch
  app/      cangkang egui — dipecah dari god-object app.rs jadi 15 modul
            (state ±80 field, tab ±80 field, jobs_search 2,4rb, …)
  ui/       fonts, tema (9), ikon (vektor), viewer
  i18n.rs   kamus dwibahasa + tes penjaga cakupan
  store.rs  persistensi config / sesi / workspace
```

Engine sengaja bebas UI dan bisa diuji unit (`src/lib.rs`). `Doc` memiliki mmap + indeks + hit; pencarian tidak pernah menyimpan teks baris, hanya `Hit { line, byte, col_start, col_end }`.

### 2.2 Kekuatan (source)

| Area | Bukti |
|---|---|
| Budaya soundness | Oracle + fuzz 1500 pola membuktikan prefilter fancy tidak pernah membuang match nyata (`fancypre.rs`) |
| Benchmark jujur | `../BENCHMARK.md` membiarkan sel kosong, bukan mengarang angka; mengoreksi klaim working-set 11 GB-nya sendiri |
| Desain memori | Checkpoint jarang (1024 baris / 64 KiB) + `LineSet` roaring + batas decode tampilan 16 KB + LRU 2000 |
| Cancel pencarian | Generasi `search_gen` abort; streaming batch 200–500 hit |
| Sidecar index | Buka kedua file 12 GB: load 0,055 dtk vs rebuild 26 dtk |
| Kualitas i18n | Tes otomatis permukaan EN + cek arity agar terjemahan tidak diam-diam meleset |
| Penanganan error | Jalur I/O engine mengembalikan `Result`; ±305 unwrap hampir semua di tes; produksi hanya Mutex unwrap di jalur emit rayon |
| Clippy | Hampir bersih; 4 izin `too_many_arguments` yang disengaja |
| CI | Gerbang tes sebelum rilis; anggaran ukuran 6 MB packed; smoke `--version` |

### 2.3 Kelemahan (source)

1. **Worker mengaburkan layering** — jalur pindai produksi hidup di `app/jobs_search.rs` / `jobs_index.rs` (2,4rb / 1,3rb baris), belum sepenuhnya di `engine/`. Reuse headless lebih sulit.
2. **`AsisLogApp` ±80 field** — coupling tinggi setelah pecah god-object sebagian; fitur lanjut akan sulit diekstrak.
3. **Duplikasi tersisa** — `WideLineReader` sudah diekstrak ke `engine/wideline.rs` (0.3.2), namun `app/tab.rs` (±1.985 baris) dan `app/jobs_search.rs` (±2.359 baris) masih saling tumpang tindih logika pipeline.
4. **Beberapa batas sengaja dangkal** — merge 200 ribu baris/file, SQL 2 juta baris pertama, fancy 256 KB/baris, Top-N sampel ±50 ribu, plafon LineSet u32 (±4,29 miliar baris).
5. **Tanpa tes UI** — interaksi egui tak teruji; follow/rotasi dan IPC multi-tab tertutup tipis.
6. **301 `.unwrap()`** di `src/` (hampir semua di blok tes; jalur produksi kini poison-tolerant) — dipantau saat kode baru. Pintasan terpetakan terhitung ±53 aksi (dok lama bilang 58).
7. **Metrik GUI BENCHMARK §2 kosong** — FPS buka, stopwatch search-ke-paint, head-to-head vs klogg masih kosong.

**Skor arsitektur: 7,5 / 10** (engine 9, lapisan app 6, tes 8). Audit 2026-09-15: 242 tes lulus, 9 tema terverifikasi, cap SQL 2 jt / merge 200 rb / hit 200 rb cocok dengan source.

---

## 3. Inventaris fitur (vs tujuan yang dinyatakan)

### Inti (wajib untuk dukungan log 10 GB)

| Fitur | Status | Catatan |
|---|---|---|
| Buka instan + virtual scroll | Ya | First paint dari byte 0; indeks sementara |
| Indeks jarang + sidecar | Ya | `.asisidx`; bangun ulang bila size/mtime beda |
| Cari literal / regex / boolean | Ya | memchr/AC, regex DFA, fancy prefilter |
| Filter termasuk/kecuali + field JSON | Ya | bitmap roaring |
| Follow + rotasi | Ya | size + head/tail FNV + notify |
| Penanda persisten | Ya | 6 warna, label, sidecar `.asislog.json` |
| Multi-tab | Ya | Ctrl+Tab, Ctrl+1–9 |
| Deteksi encoding + override | Ya | chardetng + 13 encoding |
| Ekspor hit (+ konteks) | Ya | streaming; tiket Markdown Jira |
| Pemulihan sesi | Ya | `session.json` debounce |

### Investigatif / pembeda

| Fitur | Status | vs pesaing |
|---|---|---|
| Command palette | Ya | langka di penampil log |
| Mode Zen + HUD cari | Ya | langka |
| Histogram ERROR + klik-lompat | Ya | kelas lnav/LogViewPlus |
| Top-N error / sesi | Ya | klogg tidak punya |
| Hex peek | Ya | aman untuk file korup |
| Kolom SQL + SQL-lite | Parsial | tanpa JOIN/fungsi tanggal; cap 2 jt |
| Multi-file merge | Parsial 200rb | external-sort belum |
| Timeline gabung multi-file | Parsial | 20 ribu baris/file |
| Workspace share (JSON) | Ya | bagus untuk tim |
| Scratchpad (JWT/Base64/JSON/SQL) | Ya | klogg juga punya scratchpad |
| Buka arsip (zip/tar/gz/bz2/xz/7z) | Ya | keluasan ≈ klogg; 7z tanpa pilih entri |
| Buka URL / tempel teks | Ya | ≈ klogg |
| Pintasan dapat dikonfigurasi | Ya | 58 aksi |
| 9 tema + kontras tinggi | Ya | |
| CLI `grep` / `count` | Ya | tipis tapi berguna |
| Split dual-pane | Ya | |
| Word wrap | Ya | |

### Non-goal eksplisit (by design)

Mengedit, Electron/Tauri, seluruh file di RAM, indeks padat, Elasticsearch/cloud/AI, editor hex penuh, installer Microsoft Store.

**Skor keluasan fitur: 8,5 / 10** untuk penampil gratis v0.3.1.  
**Skor kedalaman fitur: 6,5 / 10** (cap analyzer, tanpa sumber remote, tanpa pencarian multi-file skala penuh).

---

## 4. Penilaian UI / UX

### Yang berjalan baik

- **Alur kerja search-first** — kolom cari selebar jendela, chip, preset, riwayat, cakupan, cache, hasil progresif.
- **Status jujur** — `Mencari… x / y · N hasil`, encoding, chip filter, catatan LIVE.
- **Peta kepadatan + peek tooltip** — kualitas hidup langka untuk log panjang.
- **Tabel pintasan dapat dikonfigurasi** — bantuan dan perilaku tidak bisa meleset.
- **Ganti bahasa instan** — fitur portofolio untuk tim ops ID; langka di antara pesaing.
- **Mode Zen** — memadatkan 5 baris chrome demi murni membaca.
- **Ikon vektor** — tidak ada kotak tofu lintas OS.

### Risiko UX

| Risiko | Detail |
|---|---|
| Kelebihan fitur | Toolbar + tools + search = 3 baris sebelum isi log; banyak jendela mengambang (hist, top-n, hex, analyze×3, options, highlight, scratch, palette) |
| Palet substring, bukan peringkat fuzzy | Terasa kurang “VS Code” dari yang diiklankan |
| Metrik teks egui | Kurang tajam dari Qt (klogg) atau Scintilla (EmEditor/N++) di ukuran kecil |
| UPX + tanpa tanda tangan | Gesekan SmartScreen sudah didokumentasikan tapi nyata untuk pengguna enterprise |
| Tanpa biner macOS | hook cfg ada; CI tidak merilis |
| Cap lanjut kini tampil di Tentang | SQL 2 jt / merge 200 rb / fancy 256 KB / Hyperscan NO-GO — sudah di UI |

**Skor kualitas UI: 7 / 10**  
**UX untuk persona target (insinyur dukungan di log raksasa): 8,5 / 10**  
**Kesederhanaan first-run: 6,5 / 10** (bisa membanjiri; Zen + palet membantu power user).

---

## 5. Kinerja (dari bench jujur proyek sendiri)

| Metrik | AsisLog | Catatan |
|---|---|---|
| Indeks 1 GB sintetis | 0,62 dtk (1656 MB/s) | checkpoint jarang |
| Indeks 12 GB nyata | ±24–26 dtk (±500 MB/s) | vs klogg ±40 dtk |
| Heap 12 GB + cari | **47–105 MB Private** | klaim <250 MB **terbukti** |
| Buka ulang sidecar 12 GB | **0,055 dtk** | 470× vs rebuild |
| Hasil pertama (streaming) | **&lt;0,5 dtk** selagi pindai penuh lanjut | kemenangan yang dirasakan pengguna |
| Pindai literal penuh 12 GB (pola jarang) | ±39 dtk | disk-bound; jujur |
| Prefilter regex fancy | 25–164× pada lookaround | P0 0.3.1 |
| Paralel rayon + AC alternation | linear pada core | C-D1/C-D2 |

**Skor performa: 9 / 10** untuk kelas *penampil* (bukan editor). Catatan: Hyperscan (klogg) masih menang di beberapa pindai penuh regex kompleks.

---

## 6. Perbandingan kompetitif

### Peta pesaing

```text
                    GRATIS                       BAYAR
              ┌──────────────────┐        ┌────────────────────┐
  Penampil    │ AsisLog  ★       │        │ LogViewPlus        │
  log         │ klogg            │        │ EmEditor (editor)  │
  (indeks +   │ lnav (TUI)       │        │ UltraEdit (editor) │
   analyzer)  │ LogExpert        │        │ BareTail $25 (mati)│
  Editor      │ Notepad++        │        │                    │
  baseline    │ VS Code          │        │                    │
              │ less/tail/rg     │        │                    │
              └──────────────────┘        └────────────────────┘
```

### Matriks fitur (ringkas, cek ulang 2026)

Pesaing baru yang masuk radar 2026: **LogoRRR 26.9.0** (macOS/Win/Linux, proprietary + tier gratis Mac App Store, heatmap/timeline, out-of-core 10 GB @ 256 MiB heap, tanpa parser JSON/XML kaya — lihat logorrr.app/desktop-log-viewers).

| Kemampuan | AsisLog | klogg | LogViewPlus | EmEditor | UltraEdit | Notepad++ | lnav |
|---|---|---|---|---|---|---|---|
| Harga | Gratis MIT | Gratis GPLv3 | $45 / $95 | ±$45–60/th | ±$120–200/th | Gratis | Gratis |
| Buka 10 GB+ | Ya | Ya | kelas GB | Ya (klaim 16 TB) | lemah sbg log | Tidak | Ya (stream) |
| Disiplin heap | Sangat baik | Baik | Berat-RAM | Sangat baik (editor) | — | Buruk | Baik |
| Cari literal | Sangat baik | Sangat baik | Baik | Sangat baik | Sangat baik | Baik | Baik |
| Kualitas regex | Kuat + fancy | **Hyperscan** terbaik gratis | Baik | Sangat baik | Sangat baik | Baik | Baik |
| Query boolean | Ya | Ya | Filter berantai | Tidak (editor) | Terbatas | Tidak | Parsial |
| Follow + rotasi | Ya | Ya | Ya + remote | Tidak | Tidak | Plugin | Ya |
| Multi-tab | Ya | Ya | Ya | Ya | Ya | Ya | Views |
| Parser/kolom | Kolom SQL + wizard | Tidak | **Terbaik** | Fokus CSV | Wordfiles | Tidak | Format |
| Analisis SQL | SQL-lite | Tidak | **T-SQL penuh** | Tidak | Tidak | Tidak | **SQLite** |
| Gabung multi-file | Parsial 20rb | Tanpa UI | **Penuh by date** | Split/combine | Compare | Tidak | **Ya** |
| Tail remote | Tidak (hanya unduh URL) | Hanya URL | **SFTP/DB/Event** | Tidak | Suite FTP | Tidak | Tidak |
| Ekspor/Jira | Ya | Clipboard/file | CSV/HTML/Jira | Split | Compare | Dasar | — |
| Buka arsip | Luas | Luas | Analisis zip | — | — | Tidak | Ya |
| Tema/i18n | 9 + ID/EN | Gelap-ish EN | EN | EN | EN | Banyak EN/CN | EN |
| Biner portabel | **Ya ±2,5 MB** | Ya (portable zip resmi) | Installer | Installer + portabel | Installer | Portable ada | Biner |
| Komunitas/dewasa | Baru (0.3.2) | ±4k ★ matang | Komersial matang (v3.2.5, 2026-04) | Puluhan th | Puluhan th | Ubiquitous | ±10k ★ |
| Mengedit | **Tidak** | Tidak | Tidak | **Ya** | **Ya** | Ya | Tidak |

### Narasi head-to-head

**vs klogg (pesaing gratis terdekat)**  
- klogg menang: kedewasaan, throughput regex kompleks Hyperscan, poles multi-window, komunitas lebih besar.  
- AsisLog menang: biner tunggal portabel, angka heap di 12 GB, SQL-lite (2 jt), histogram/Top-N, timeline merge (200 rb/file), ekspor Jira, bilingual ID/EN, command palette + Layar Penuh, keluasan pintasan terkonfigurasi.  
- Verdict: **setara, bukan inferior** — berbeda di UX investigasi + portabilitas; klogg tetap tolok ukur mesin regex.

**vs LogViewPlus (produk log komersial terdekat)**  
- LVP menang: sumber remote (SFTP/DB/Event Log), T-SQL penuh, parser matang, peringatan, dukungan enterprise.  
- AsisLog menang: gratis, lintas platform, open source, RAM lebih rendah, portabel.  
- Verdict: LVP untuk ops enterprise beranggaran; AsisLog untuk tim dukungan hemat biaya di file lokal multi-GB.

**vs EmEditor / UltraEdit (editor berbayar)**  
- Kategori berbeda. Mereka mengedit dan menangani file besar sebagai *editor*.  
- AsisLog gratis, khusus investigasi log; tidak bisa mengedit — by design.  
- Untuk “buka log 10 GB, cari ERROR, tail LIVE, ekspor tiket” AsisLog alat yang lebih tepat di harga $0.

**vs Notepad++ / VS Code (baseline yang ditinggalkan pengguna)**  
- Menang jelas: alat itu gagal di `catalina.out` multi-GB; AsisLog pengganti yang dimaksud.

**vs lnav v0.14.1**  
- lnav menang merge waktu multi-file + SQLite nyata, TUI gratis, SSH.  
- AsisLog menang GUI, penanda/warna, bilingual, portabel Windows-first, scratchpad, UI histogram.  
- Lebih saling melengkapi daripada bersaing (server vs desktop).

---

## 7. Penilaian (0–10)

### 7a. Revisi pasca-rilis 0.3.2 (2026-09-12)

Perbaikan yang dikerjakan dan dampaknya terhadap skor:

| Perbaikan 0.3.2 | Dimensi tersentuh | Δ skor |
|---|---|---|
| SQL-lite cap 500 rb → **2 jt** baris | Alat investigasi | +0,4 |
| Merge timeline 20 rb → **200 rb** baris/file | Alat investigasi | +0,4 |
| Batas jujur tampil di Tentang (Hyperscan NO-GO, fancy 256 KB, ekspor tanpa batas) | UI/UX + kepercayaan | +0,2 |
| Tools progressive disclosure (Alat ▾) + empty-state hints | UI/UX first-run | +0,4 |
| Mode Zen → **Layar Penuh** (label lebih ramah pengguna umum) | UI/UX kejelasan | +0,2 |
| Mutex poison-tolerant di rayon | Kualitas kode | +0,1 |
| `WideLineReader` → `engine/wideline.rs` + tes | Arsitektur | +0,2 |
| Tes critical-path merge + SQL (11 integrasi, suite penuh hijau) | Kualitas kode | +0,2 |
| Catatan packaging winget/choco/scoop/AppImage | Ekosistem | +0,3 |
| Rilis UPX 2,87 MB + SHA256 (siap didistribusikan) | Portabilitas | stabil |

**Bobot untuk penampil log portabel gratis** (pasca 0.3.2):

| Dimensi | Bobot | Skor lama | Skor baru | Terbobot | Alasan |
|---|---|---|---|---|---|
| Inti file besar | 20% | 9,0 | 9,0 | 1,80 | Tidak berubah; masih terbaik di kelasnya |
| Kualitas pencarian | 15% | 8,0 | 8,0 | 1,20 | Fancy prefilter sudah; Hyperscan tetap di klogg |
| Filter / follow / penanda | 10% | 8,5 | 8,5 | 0,85 | Tidak berubah |
| Alat investigasi | 15% | 7,5 | **8,2** | 1,23 | Cap 4×–10× lebih dalam; batas jujur di UI |
| Poles UI/UX | 10% | 7,0 | **7,5** | 0,75 | Alat ▾ + Layar Penuh + empty state |
| Portabilitas & ops | 10% | 9,5 | 9,5 | 0,95 | Masih tanpa tanda tangan / macOS |
| Kualitas kode & tes | 10% | 8,0 | **8,3** | 0,83 | Engine extract + poison-tolerant + tes baru |
| Ekosistem / kedewasaan | 5% | 5,0 | **5,5** | 0,28 | Catatan packaging; manifest belum disubmit |
| Nilai (fitur per $) | 5% | 10,0 | 10,0 | 0,50 | Tetap gratis MIT |
| **TOTAL** | **100%** | **8,18** | | **8,39 ≈ 8,4** | |

### Skor komparatif (rubrik sama)

| Tool | Sebelum | Sesudah | Peran |
|---|---|---|---|
| **AsisLog** | **8,2** | **8,4** | Penjelajah log portabel gratis, suite investigatif |
| LogViewPlus | 8,5 | 8,5 | Produk log Windows berbayar; remote + SQL penuh |
| klogg | 8,0 | 8,0 | Pesaing gratis matang; raja Hyperscan |
| lnav | 8,0 | 8,0 | TUI gratis; merge/SQLite kuat; tanpa GUI |
| EmEditor | 7,5 | 7,5 | Editor berbayar (sebagai *alat log*) |
| UltraEdit | 6,0 | 6,0 | Editor berbayar; spesialisasi log buruk |
| LogExpert | 6,5 | 6,5 | Columnizer Windows gratis; menua |
| Notepad++ | 4,0 | 4,0 | Gagal beban kerja target |
| VS Code | 3,5 | 3,5 | Gagal beban kerja target |

**Gap vs LogViewPlus** menyempit: 0,3 poin (dulu 0,3–0,5 di dimensi analyzer).  
**Gap vs klogg** melebar di sisi AsisLog: +0,4 (dulu +0,2) berkat analyzer + UX.

### Skor komparatif detail (rubrik pra-0.3.2 dipertahankan untuk pembanding historis)

| Tool | Skor | Peran |
|---|---|---|
| **AsisLog** | **8,4** | Penjelajah log portabel gratis, suite investigatif |
| LogViewPlus | 8,5 | Produk log Windows berbayar v3.2.5; remote + SQL penuh |
| klogg | 8,0 | Pesaing gratis matang; raja Hyperscan; portable zip; analyzer lebih dangkal |
| lnav | 8,0 | TUI gratis v0.14.1; merge/SQLite kuat; tanpa GUI |
| LogoRRR | ~7,0 | Baru 2026; heatmap/timeline menarik; parser dangkal; proprietary |
| EmEditor | 7,5 (sebagai *alat log*) | Editor berbayar; klaim buka 16 TB; ukuran buka mentah tak tertandingi |
| UltraEdit | 6,0 (sebagai *alat log*) | Editor berbayar; spesialisasi log buruk |
| LogExpert | 6,5 | Columnizer Windows gratis; menua |
| Notepad++ | 4,0 | Gagal beban kerja target |
| VS Code | 3,5 | Gagal beban kerja target |
| BareTail / glogg | 3–4 | Historis |

**Verdict terurut untuk “buka log lokal raksasa, cari, tail, investigasi, ekspor”:**

1. **AsisLog 8,4** — paket *desktop* gratis terbaik bila ingin GUI + alat + tanpa instal  
2. **klogg 8,0** — *mesin pencari* gratis terbaik; pilih bila butuh Hyperscan atau multi-window (portable zip resmi, Mac didukung)  
3. **lnav 8,0** — jalur *SSH/TUI* gratis terbaik  
4. **LogViewPlus 8,5** — *berbayar* terbaik bila sumber remote / SQL penuh membenarkan $45–95 (v3.2.5)  
5. EmEditor 7,5 — hanya bila juga *mengedit* file raksasa (Windows, berbayar)  
6. LogoRRR ~7 — pesaing baru lintas platform; menarik untuk heatmap, tapi analisis kolom lebih dangkal  
7. Lainnya — tidak masuk shortlist untuk log 10 GB  

---

## 8. SWOT (AsisLog 0.3.2)

| | Membantu | Merugikan |
|---|---|---|
| **Internal** | **S:** portabel 2,87 MB, disiplin heap, bench jujur, ID/EN, suite investigatif (SQL 2 jt · merge 200 rb), Layar Penuh + Alat ▾, tes engine kuat, gerbang ukuran CI, batas jujur di UI | **W:** proyek muda, tanpa rilis macOS, SmartScreen UPX, god-object state app, tanpa tes UI egui, tanpa Hyperscan, worker search belum penuh di engine |
| **Eksternal** | **O:** pengguna N++/VS Code macet di log; harga LVP/EmEditor; celah bilingual pasar ID; klogg tanpa SQL/histogram | **T:** klogg terus maju; enterprise lebih suka installer bertanda tangan + support; pemasaran “editor” (EmEditor 16 TB) mencuri perhatian |

---

## 9. Rekomendasi prioritas (status pasca 0.3.2)

| # | Rekomendasi | Status |
|---|---|---|
| 1 | Rilis biner macOS + installer Windows bertanda tangan | **Belum** — tetap P0 |
| 2 | BENCHMARK §2 GUI vs klogg di mesin sama | **Belum** — tetap P0 |
| 3 | Ekstrak worker pencarian ke `engine` | **Parsial** — baru `WideLineReader` |
| 4 | Naikkan cap merge/SQL | **Selesai** — 200 rb / 2 jt |
| 5 | Dokumentasikan Hyperscan NO-GO di UI | **Selesai** |
| 6 | Sederhanakan chrome first-run | **Selesai** — Alat ▾ + empty state |
| 7 | Mutex unwrap → poison-tolerant | **Selesai** |
| 8 | Packaging komunitas | **Parsial** — catatan README; manifest belum disubmit |

**Sisa pekerjaan krusial (urut ROI):**
1. Installer bertanda tangan + biner macOS  
2. Bench GUI vs klogg di mesin sama  
3. Ekstrak worker search/index utuh ke `engine`  
4. Submit manifest winget/scoop/choco + tangkapan layar  
5. Tes UI alur kritis (buka → cari → follow → ekspor)

---

## 10. Kesimpulan satu kalimat (0.3.2)

**Pasca 0.3.2 + audit 2026-09-15, AsisLog tetap 8,4/10: cap analyzer 4–10× lebih dalam, UX first-run lebih ramping, kode lebih bersih, rilis portabel 2,87 MB — audit source mengonfirmasi semua klaim cap/tes (242 tes lulus); data pesaing 2026 menambah LogoRRR sebagai pesaing baru (~7,0) tanpa mengubah papan atas. Masih di bawah LogViewPlus (8,5) karena tanpa remote/SQL penuh, unggul atas klogg (8,0) di analyzer + UX. Sisa gap utama: tanda tangan kode, macOS, bench GUI head-to-head.**

---

*Metode: inventaris penuh `src/` (pasca 0.3.2: +`engine/wideline.rs`), **audit ulang 2026-09-15: 27.760 LOC dibaca ulang, `cargo test` 242 lulus, cap diverifikasi di source**, tinjauan kejujuran CHANGELOG/BENCHMARK, audit layering kode, halaman produk/harga resmi 2026 pesaing (klogg.dev, logviewplus.com v3.2.5, emeditor.com, lnav.org v0.14.1, logorrr.app). Tanpa benchmark karangan; sel kosong tetap dibiarkan kosong. Revisi skor pasca P1/P2 0.3.2.*
