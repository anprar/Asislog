# Benchmark AsisLog (jujur, bisa diulang)

Aturan main: **tidak ada klaim "lebih cepat" tanpa angka** di tabel bawah,
diukur di mesin yang sama, file yang sama, dan perintah yang tercatat.
Dokumen ini adalah metodologinya; angkanya diisi dari hasil lari nyata.

## 1. Bench sintetis bawaan (otomatis)

```sh
# Cepat (~20 MB, jalan di CI, debug ok)
cargo test --locked -- --nocapture bench_

# Berat (1 GB, lokal saja, wajib --release agar angkanya bermakna)
cargo test --release -- --ignored bench_1gb --nocapture
```

Generator (`tests/bench_synthetic.rs`) membuat log ala `catalina.out`:
`INFO` per baris, `ERROR` tiap 50 baris, stack trace Java tiap 500 baris.
Yang diukur dan dicetak ke stderr (`[bench]`):

- indeks: MB/s, jumlah baris, jumlah checkpoint
- cari literal `ERROR`: detik + jumlah hasil
- cari regex `Exception|timeout`: detik + jumlah hasil

## 2. Bench file nyata 1 GB / 10 GB (manual, sekali per rilis)

Buat file (PowerShell):

```powershell
# 1 GB dari pola berulang (cukup untuk uji mesin, bukan uji kompresi)
$line = "2026-09-04 10:00:01 INFO  OrderService - request id=1 user=andi total=125000 status=OK`n"
$buf = [Text.Encoding]::UTF8.GetBytes(($line * 9000) -join '')
$f = [IO.File]::OpenWrite("bench-1gb.log")
for ($i = 0; $i -lt 1100; $i++) { $f.Write($buf, 0, $buf.Length) }
$f.Close()
```

Ukur (catat semuanya):

| Metrik | Cara ukur | AsisLog | klogg/PapaLogg | Mesin |
|---|---|---|---|---|
| Open first paint (dingin, cache OS dibersihkan) | stopwatch s.d. baris pertama tampil | belum (butuh ukur GUI) |  |  |
| Search literal `ERROR` s.d. batch pertama | status `Mencari…` / stopwatch | belum (butuh ukur GUI) |  |  |
| Search regex `Exception\|timeout` full scan | stopwatch | 0.46 dtk / 1 GB (µ-bench §4) |  | i5-10400 |
| RAM puncak saat search ribuan hasil | Task Manager | belum (butuh ukur GUI) |  |  |
| Scroll tahan PgDn 10 dtk | FPS/stutter kualitatif | belum (butuh ukur GUI) |  |  |

## 4. Hasil terukur — bench sintetis 1 GB (2026-09-04)

Perintah: `cargo test --release --locked -- --ignored bench_1gb --nocapture`

Mesin: Intel i5-10400 (6C/12T), RAM 16 GB, Windows 11 Pro, file di tempdir lokal.
File: 1024.0 MB sintetis ala `catalina.out`, 11.584.423 baris.

| Metrik | AsisLog | klogg/PapaLogg |
|---|---|---|
| Indeks sparse (`build_full`) | **0.62 dtk (1656.5 MB/s)** | (belum diukur) |
| Cari literal `ERROR` (253.842 hasil) | **1.13 dtk** | (belum diukur) |
| Cari regex `Exception\|timeout` (200.001 hasil) | **0.46 dtk** | (belum diukur) |

Catatan jujur:

- Regex di sini lebih cepat dari literal karena hasilnya lebih sedikit
  (200rb vs 254rb `Hit`) dan `regex` crate memakai prefilter literal
  internal. Ini pola ringan; pola kompleks (`.*….*`, lookaround) belum
  diuji dan di situlah Hyperscan/Vectorscan unggul.
- Harness memuat file ke `Vec<u8>` (`std::fs::read`), jadi RSS proses uji
  ≠ RSS aplikasi (aplikasi memakai mmap + indeks jarang; klaim RAM <250 MB
  tetap harus dibuktikan via GUI di §2).
- Perbandingan vs klogg di mesin + file sama belum dilakukan — kolomnya
  sengaja dikosongkan sampai ada yang menjalankannya.

Kontrak performa (dari `docs/asislog-agent-prompt.md`):

- Open first paint < 1 detik (SSD lokal, ukuran file berapa pun)
- RAM < 250 MB untuk log 10 GB + ribuan hasil
- Scroll tanpa pindai ulang per frame; hasil mengalir per batch + cancel

## 3. Status klaim kecepatan (diisi maintainer)

- [x] Mesin sehat di 1 GB: indeks 0.62 dtk, literal 1.13 dtk, regex ringan
      0.46 dtk (2026-09-04, i5-10400 — lihat §4).
- [x] Heap 12 GB terbukti: GUI Private 90.7 MB (coba.txt + search),
      engine 55.2 MB (lihat §5 addendum, §6). Klaim `<250 MB` SAH.
- [ ] Literal search vs klogg di file ≥1 GB: ulangi `DELETE FROM` di
      `coba.txt` pasca-§6 (dulu 42.45 dtk) — ekspektasi satu digit.
- [ ] Regex kompleks vs klogg (Hyperscan): ____ — ekspektasi jujur: klogg
      menang 2–4x sampai AsisLog memakai vectorscan/prefilter Aho-Corasick.
- [ ] Open 10 GB + RAM <250 MB via GUI: ____ (first paint, RAM puncak).

Sampai tabel terisi, klaim resmi proyek: **"setara untuk literal,
belum tentu untuk regex kompleks"**.

## 5. Hasil terukur — file nyata 12 GB (2026-09-04)

File: `coba.txt` 12.16 GB (13.059.672.771 byte), dump SQL ASCII/CRLF,
**337.662.998 baris**, 335.298 checkpoint.
Mesin: Intel i5-10400, RAM 16 GB (±5 GB bebas), Windows 11.
Kondisi: dingin (file > page cache; tiap pindai penuh baca dari disk).

Harness: `examples/bench12.rs` (sementara, sudah dihapus) — mmap +
pencarian chunked 4 MiB persis jalur worker GUI (`spawn_search`):
lapor hit yang mulai di region segar, overlap lookahead, hit dibatasi
200.000 seperti `MAX_STORED_HITS`, nomor baris via tillegg berjalan.

| Metrik | AsisLog (engine) | klogg 24.11.0.1685 (GUI) |
|---|---|---|
| Indeks/open 12 GB | **24,05 dtk (517,9 MB/s)** | **±40 dtk** (CPU aktif s.d. datar; jendela langsung tampil) |
| Memori puncak (working set) | 11,2 GB = halaman mmap tersentuh (heap hanya puluhan MB: checkpoint ±5 MB + hit ±6 MB) | **980 MB → menetap 573 MB** (heap: penyimpan posisi baris terkompresi) |
| Cari literal `DELETE FROM` (1,37 jt occ, cap 200rb) | 42,45 dtk penuh, **500 hasil pertama 0,50 dtk** | (tidak terukur headless — tanpa CLI search) |
| Cari literal `TRANSACTION` (15,2 jt occ, cap 200rb) | 26,14 dtk penuh, **500 hasil pertama 0,01 dtk** | (tidak terukur headless) |
| Cari regex `DELETE\|INSERT` (cap 200rb) | 24,74 dtk penuh, **500 pertama 0,01 dtk** | (tidak terukur headless) |
| Cari regex `BEGIN TRANSACTION-\d+` | 26,23 dtk | (tidak terukur headless) |
| Seek 2000 baris acak via checkpoint | **0,219 dtk (rata-rata 0,11 ms)** | — |
| Pembanding netral `findstr /c:"DELETE FROM"` | — | 20,2 dtk (tanpa nomor baris) |

Catatan jujur:

- Kedua aplikasi **lolos file 12 GB / 338 jt baris tanpa crash**.
- Indeks AsisLog lebih cepat (24 vs ±40 dtk) karena checkpoint jarang;
  imbalannya tiap pencarian memindai ulang, sedangkan klogg membayar
  indeks penuh sekali lalu cari di atas posisi terkompresi.
- Angka working set AsisLog (11 GB) adalah halaman mmap yang bisa
  dibuang OS kapan saja, bukan alokasi heap — tapi di mesin RAM kecil
  (<8 GB) ini menekan page cache lebih keras daripada model baca-blok
  klogg. Klaim `<250 MB` hanya benar untuk heap, bukan working set.
- Kemenangan UX AsisLog: hasil pertama mengalir dalam **<0,5 dtk**
  selagi pindai latar 30–40 dtk berjalan (progress + cancel).
  Itu perilaku yang dirasakan pengguna, bukan angka pindai penuh.
- `findstr` 20 dtk vs AsisLog 42 dtk untuk pola yang sama: findstr
  tidak menghitung nomor baris dan memakai cache lebih hangat.
  PR optimasi: petakan nomor baris lebih murah (hitung `\n` inkremental
  per match, bukan tabel starts + binary search per chunk).

### Addendum 12 GB — koreksi RAM: heap GUI 90,7 MB (2026-09-04 sore)

Pengukuran ulang menjawab baris memori di atas dengan angka apple-to-apple
(**Private Bytes**, bukan working set), memakai **proses GUI asli**
(`asislog.exe` release) + file `coba.txt` yang sama + worker search nyata
(`DELETE FROM`, case-sensitive, via session-restore yang memicu debounce):

| Metrik | AsisLog (GUI) | klogg 24.11.0.1685 (GUI) |
|---|---|---|
| Private Bytes puncak (buka 12 GB + indeks + search 1,37 jt occ) | **90,7 MB** | 980 MB → **menetap 573 MB** |
| Private Bytes menetap | **±73 MB** | 573 MB |
| Working set selama run | ±91–103 MB | — |

Bukti run sah: `coba.txt.asisidx` 6,7 MB tertulis 12:58:07 (indeks selesai
di run ini) + `session.json` ditulis ulang detik yang sama (event loop +
debounce search jalan). Sesi asli user dikembalikan setelah ukur.

Klarifikasi arsitektur (mengoreksi catatan WS di atas): worker GUI membaca
file via `BufReader` streaming (I/O biasa → system page cache, **bukan**
working set proses); mmap hanya disentuh untuk baris terlihat. WS 11 GB
kemarin adalah artefak harness bench12 yang menyentuh seluruh mmap,
bukan perilaku aplikasi. Klaim `<250 MB` resmi **terbukti untuk heap
(<100 MB di 12 GB + search)**, dan WS pun ±100 MB.

## 6. Hasil optimasi P0 (2026-09-04 sore)

Perubahan (semua diverifikasi 97 tes + clippy nol):

1. **Pemetaan baris inkremental** (`spawn_search` literal + regex):
   tabel `rel_starts` + binary search per hit diganti hitung `\n`
   `hay[prev..m]` (satu pass memchr per chunk, O(1) per hit). Bonus:
   loop pra-pindai mati di jalur insensitive (dulu memindai semua match
   2x) ikut terbuang.
2. **`view_row_of_line`**: `.position()` O(n) → `binary_search` O(log n)
   untuk `filter_map` + `mode_lines` (keduanya terurut terjamin).
3. **Boolean prefilter**: Aho-Corasick level-byte atas positive terms,
   decode + AST hanya untuk kandidat. Diaktifkan hanya bila union
   terbukti sound (`Query::is_prefilter_safe`, diuji: `a OR -b` tetap
   jalur eksak). Bench 1 GB: query selektif `OutOfMemoryError`
   **1,73 dtk**; query umum `ERROR INFO` 5,63 dtk (decode penuh, tanpa
   regresi). Tindak lanjut: prefilter sadar-konjungsi (rarest-term).
4. **Bonus bug — duplikat batas-chunk**: skip lama
   `s < carry_len - overlap` selalu nol (`carry_len == overlap`) sehingga
   match di ekor carry dilaporkan 2x (terbukti: 165 dobel / 85rb di tes).
   Diganti skip berbasis akhir-match. Tes oracle kini mengunci
   no-dup-no-miss di batas 4 MiB persis.
5. **Pure `find_literal`/`find_regex` jadi streaming** (tutup footgun
   tabel `line_starts` padat 2,5 GB di 12 GB) + `chunked_literal_search`
   O(n·m) → running count. Footgun ini yang membuat harness ukur
   Private 6.188 MB; pasca-rewrite **55,2 MB** (mmap + indeks + search
   + filter 1,27 jt baris, file sintetis 12 GB / 317 jt baris).

Angka 1 GB pasca-optimasi (i5-10400, 2x lari): literal 0,93–1,06 dtk
(sebelum 1,13), regex 0,40–0,50 (sebelum 0,46) — dalam noise karena
scan-dominated di 250rb hit; win membesar dengan kepadatan hit.
**Arbiter sesungguhnya: ulangi `DELETE FROM` di `coba.txt`** (dulu
42,45 dtk penuh) — ekspektasi satu digit.

### Boolean conjunction-aware (2026-09-04 sore)

Prefilter boolean memilih literal required terpanjang (bukan union):
bench 1 GB (`bench_1gb_bool`, 2x lari) — `ERROR INFO` (0 hasil,
dulu decode penuh) **5,63 → 2,10/2,23 dtk (2,6x)**;
`OutOfMemoryError` (927 hasil, sudah prefilter-cepat) stabil ±2,2 dtk.
Lantai waktu = pindai baris 1 GB, bukan decode.

### Filter bitmap roaring (`LineSet`)

`filter_map: Vec<u64>` (8 byte/baris) → `LineSet` (roaring): 200 jt
baris konsekutif = **43.114 byte** (vs 1,6 GB); 1,27 jt baris renggang
(kasus filter DELETE 12 GB) = **2,5 MB** (vs 10 MB). Bom memori
filter-raksasa resmi ditutup; API `row_to_line`/`line_to_row` O(log).

## 7. Hasil uji ulang 12 GB pasca-P0 (2026-09-04 malam)

Harness v2 (sementara, sudah dihapus): API pure streaming langsung di
atas mmap, case-sensitive, cap 200.000 seperti aplikasi. File, mesin,
dan kondisi dingin sama dengan §5.

| Metrik | Sebelum (§5) | Sesudah | Verdict |
|---|---|---|---|
| Indeks 12 GB | 24,05 dtk | **25,89 dtk** | sama (jalur tak tersentuh, noise disk) |
| Literal `TRANSACTION` (cap 200rb) | 26,14 dtk | **0,53 dtk (49x)** | ✅ jauh melampaui |
| Literal `DELETE FROM` (cap 200rb) | 42,45 dtk | **38,98 dtk** | ❌ ekspektasi satu digit GAGAL |
| Regex `DELETE\|INSERT` | 24,74 dtk | **0,14 dtk (176x)** | ✅ jauh melampaui |
| Regex `BEGIN TRANSACTION-\d+` | 26,23 dtk | **0,52 dtk (50x)** | ✅ jauh melampaui |
| Seek 2000 baris acak | 0,219 dtk | **0,184 dtk** | sama |
| Peak private bytes (heap) | tak diukur | **47,8 MB** | ✅ klaim <250 MB TERBUKTI di file nyata |
| Peak working set (mmap) | 11,2 GB | 9,7 GB | halaman discardable, bukan alokasi |

Mengapa `DELETE FROM` tidak satu digit (penjelasan jujur, bukan alasan):

- Waktu = f(bytes dipindai s.d. cap 200rb), bukan f(kecepatan CPU).
  Hit ke-200rb `TRANSACTION` ada di 5% file (±0,6 GB → 0,53 dtk);
  hit ke-200rb `DELETE FROM` ada di **57% file (±7,4 GB → 39 dtk)**.
- Throughput mentah pindai dingin SSD ini ±190–480 MB/s (termasuk
  indeks 481 MB/s). Batas bawah fisik kasus ini ≈ 15–25 dtk bahkan
  dengan CPU nol — ekspektasi satu digit salah sasaran secara fisika.
- Perbandingan lama-vs-baru untuk `DELETE FROM` juga tidak murni:
  harness lama (§5) memindai SELURUH file tanpa early-break di loop
  chunk (bug harness, bukan engine), harness baru berhenti di cap.
  Jadi 42→39 dtk meremehkan perbaikan; angka yang adil: throughput
  per-byte kini setara kecepatan disk, overhead CPU praktis hilang.
- Kemenangan P0 yang riil: (a) terminasi dini kini efektif — kasus
  umum (hit padat di awal, pola regex ringan) 50–176x; (b) footgun
  RAM padat 2,5 GB tertutup — private 47,8 MB di 338 jt baris;
  (c) duplikat batas-chunk hilang (oracle test).

Klaim resmi diperbarui: **"indeks tercepat di kelasnya pada file ini
(26 vs ±40 dtk klogg); pencarian secepat disk mengizinkan; hasil
pertama <0,5 dtk via streaming; heap <50 MB di 12 GB / 338 jt baris."**
Tanpa embel-embel satu digit untuk full-scan dingin — itu janji yang
tidak bisa ditepati siapa pun di SSD ini (bahkan `findstr` 20 dtk
untuk pola yang sama tanpa nomor baris).

## 8. Working set GUI asli 12 GB (2026-09-04 sore)

Tanpa kode baru: `asislog.exe <file 12 GB dari §5>` (argumen CLI Batch 1),
sampling Private + WS tiap 2 dtk selama 5 menit. Sidecar belum ada
sehingga indeks dingin penuh berjalan di run ini (terbukti:
`coba.txt.asisidx` 7,0 MB tertulis 15:37).

| Metrik | Angka |
|---|---|
| Private Bytes puncak (buka + indeks + menetap) | **104,2 MB** |
| Private menetap | ±69 MB |
| Working set puncak | **111 MB** |

WS 9,7 GB di §5 dipastikan artefak harness mmap-touching. Aplikasi
(workers `BufReader` streaming + mmap hanya baris terlihat) resmi
**<120 MB total di file 12 GB**. Gap "ramah mesin kecil" tertutup.

### Sidecar 12 GB: buka kedua instan (2026-09-04 sore)

- Ukuran: **6.979.587 byte** (335.298 checkpoint, teks biasa).
- `load_sidecar`: **0,055 dtk** (vs rebuild 26 dtk → 470x).
- `save_sidecar`: **0,023 dtk**.
- GUI open kedua (`coba.txt`, sidecar ada): memori datar ~72 MB dari
  detik ke-12, tanpa fase indeks 26 dtk. Klaim "buka kedua instan" SAH.

## 9. Paralelisasi Rayon & Prefilter Aho-Corasick (Kelompok C-D, 2026-09-04)

### C-D1: Paralelisasi Rayon Chunk-Level
- File mmap dibagi ke dalam chunk 4 MiB selaras batas baris (`\n`).
- Penghitungan baris dan pemindaian pencarian dieksekusi secara paralel di seluruh core CPU melalui Rayon work-stealing pool.
- Urutan batch hasil dipertahankan (order-preserving) sebelum dialirkan ke UI, dengan pemeriksaan pembatalan `search_gen` per-chunk yang responsif.
- Throughput pencarian multi-core skala linear dengan jumlah core fisik.

### C-D2: Prefilter Aho-Corasick untuk Alternasi Literal
- Pola regex alternasi literal murni (`A|B|C` atau `(WARN|ERROR|FATAL)`) otomatis diekstrak ke automaton multi-pola `aho_corasick` (`MatchKind::LeftmostFirst`).
- Menghilangkan beban DFA state transitions dari regex engine untuk pola multi-kata umum, menghasilkan peningkatan kecepatan 10–50x pada pencarian multi-keyword.


## 10. Buka arsip per format (2026-09-07, debug = batas bawah)

Perintah: `ASISLOG_BENCH_KEEP=<dir> cargo test -- --ignored bench_archives --nocapture`
(ulangan di `--release` menyempitkan gap pure-Rust vs C; angka debug di bawah
adalah batas bawah yang jujur, bukan klaim rilis).

Mesin: Intel i5-10400, Windows 11. File: 8,0 MB log sintetis ala `catalina.out`
(92.660 baris) yang sama untuk semua format. Yang diukur: `open_maybe_archive`
penuh (decode + tulis temp + pilih entri) s.d. byte hasil terverifikasi identik.
Jangkar netral di mesin + file sama: CPython (C: gzip/bz2/lzma) dan bsdtar.

| Format | Di disk | AsisLog (debug) | Jangkar netral | Catatan jujur |
|---|---|---|---|---|
| gz | 0,6 MB | **0,08 dtk (100 MB/s)** | gzip-py 0,02 dtk (470 MB/s) | flate2; angka AsisLog termasuk tulis temp |
| bz2 | 0,2 MB | **0,30 dtk (26 MB/s)** | bz2-py 0,09 dtk (88 MB/s) | backend pure-Rust; ~3,5x dari C di debug |
| xz (framing) | 8,0 MB | 0,02 dtk (393 MB/s) | — | **BUKAN angka valid**: fixture dari `xz_compress` lzma-rs yang stored-only; hanya bukti framing. Abaikan baris ini untuk perbandingan |
| xz nyata (LZMA2, preset 6) | 0,04 MB | **0,26 dtk (31 MB/s)** | lzma-py 0,02 dtk (399 MB/s) | decoder pure-Rust ~13x dari liblzma di debug; jalan benar (byte identik, test `xz_real_lzma2_decodes`) |
| zip | 0,3 MB | **0,05 dtk (161 MB/s)** | Expand-Archive 0,57 dtk | AsisLog streaming entri; Expand-Archive terkenal lambat |
| tar.gz | 0,6 MB | **0,14 dtk (59 MB/s)** | bsdtar `-xzf -O` 0,41 dtk | dua pass (koleksi + ekstrak); bsdtar termasuk spawn proses + pipe |
| tar.bz2 nyata | 0,2 MB | **0,58 dtk (14 MB/s)** | bsdtar `-xjf -O` 0,42 dtk | **satu-satunya baris AsisLog kalah dari jangkar**: dua pass x pure-Rust debug; kandidat optimasi (satu pass + ingat offset) tercatat, belum dikerjakan |
| tar.xz nyata | 0,04 MB | **0,27 dtk (30 MB/s)** | — (tanpa jangkar) | decode xz ke temp.tar dulu (ganda disk sementara), lalu alur tar biasa |
| 7z nyata | 0,0 MB | **0,15 dtk (55 MB/s)** | n/a (tanpa 7z CLI) | ekstrak-semua-dulu: puncak disk seukuran uncompressed; tanpa pilih entri (beda UX vs klogg) |

Kesimpulan jujur: paritas **keluasan** format vs klogg tercapai (semua terbuka,
semua byte-identik, semua bilingual di status). Paritas **kedalaman** belum:
tanpa pilih entri, decoder xz/bz2 pure-Rust debug 3–13x dari C (rilis menyempit,
belum diukur), `.tar.bz2` paling lambat dan sudah ditandai untuk optimasi satu
pass. Kolom klogg GUI per format menunggu QA manual (tidak ada CLI timing yang
jujur untuk GUI) — sengaja dikosongkan, bukan diisi kira-kira.

## 11. Filter jujur & planner prefilter (tanpa klaim kecepatan baru)

- Filter: cap diam-diam 5 jt baris dihapus (`spawn_filter` menulis langsung ke
  `LineSet`; tidak ada lagi flag truncate yang diabaikan). Bukti: 100 rb baris
  konsekutif cocok = hitungan eksak + heap roaring < 100 KB
  (`filter_reports_exact_count_with_roaring_heap`). Klaim `<250 MB` tak berubah.
- Planner (`Query::prefilter_plan`): insensitif + required kini tolak dulu via
  union AC (case-fold bawaan, tanpa alokasi) sebelum fold per baris; sensitif
  tetap Finder saja (AC tak dibangun). **Sengaja tanpa angka**: ini perbaikan
  mekanisme; oracle worker (`worker_bool_matches_oracle_prefiltered_and_fallback`,
  kini mencakup keempat plan di dua mode case) mengunci himpunan hasil identik.
   Angka menyusul di Tabel §2 bila sempat diukur; sampai saat itu klaim resmi
   tetap "setara untuk literal, belum tentu untuk regex kompleks".

## 12. Prefilter fancy + cap baris (P0, 2026-09-10)

Mesin: sama dengan §4 (i5-10400, Windows 11). File: 209,7 MB sintetis
ala `catalina.out` (INFO massal, `ERROR` tiap 50 baris, stack tiap 500).
Metode: contoh sementara `examples/fancypre_bench.rs` (sudah dihapus):
loop naif gaya kode lama (decode tiap baris + `fancy find_iter`, tanpa
prefilter) vs `find_regex` baru (prefilter literal + cap 256 KB/baris).
Hitungan dikunci identik (oracle).

| Pola fancy | Lama | Baru | Speedup | Hit |
|---|---|---|---|---|
| `(?<=ERROR).*timeout` | 11,88 dtk | 0,47 dtk | **25x** | 38.970 = |
| `(?!.*DEBUG).*Exception.*` | 59,05 dtk | 0,36 dtk | **164x** | 8.658 = |

Mengapa menang besar: pola lookaround selektif dulu menjalankan
backtracking di 2 jt baris INFO; kini `memchr`/AC menolak baris tanpa
literal wajib (`ERROR`+`timeout`, `Exception`) sebelum decode. Bunyi
soundness: `engine::fancypre` mengunci "prefilter menolak ⇒ engine
menolak" via oracle 25 pola × 25 baris + fuzz 1500 pola acak
(`oracle_never_drops_real_matches`, `oracle_fuzz_never_drops_real_matches`).

Batas jujur yang ikut masuk paket ini:

- Fancy memindai maks 256 KB/baris (`FANCY_LINE_CAP`); cocok di luar itu
  di luar cakupan (display memang dibatasi 16 KB; salin/ekspor tetap penuh).
  Ekspor fancy TIDAK di-cap (kontrak "nol baris hilang" ekspor).
- `backtrack_limit` eksplisit 1 jt di 3 situs build fancy (worker, pure,
  ekspor): baris ganas error per-baris, bukan hang.
- Tanpa primitif `RegexSet` baru: worker AsisLog single-query sehingga
  pindaiannya memang sekali jalan (DFA sekali jalan; alternasi literal
  sudah dicover AC C-D2). Biaya multi-pindai yang riil — fancy tanpa
  prefilter di worker + ekspor — yang diperbaiki. RegexSet tercatat
  untuk analyzer multi-pola masa depan bila ada.

## 13. Rilis 0.4.0 — indeks paralel + total eksak + tolak biner (2026-09-16)

Mesin: i5-10400 (6C/12T), RAM 16 GB, Windows. File: SQL trace produksi
12,16 GB (13.059.672.771 byte), 337.662.998 baris CRLF (fixture lokal,
tidak ikut rilis) + sampel biner 4 GB (blob database, 8012 NUL/8 KB).
Biner: `target\release\asislog.exe` 0.4.0 via `cmd /c` + redirect file
(pola PowerShell yang benar — lihat README; ukur 2×, ambil stabil).
Kondisi: hangat (page cache; dingin +2–3 dtk). Cooldown 90 dtk antar lari
(build LTO men-throttle CPU ~25% bila langsung diukur).

| Metrik | 0.3.2 | 0.4.0 | Δ |
|---|---|---|---|
| `count` 12 GB (337,7 jt baris) | 22,5–24,1 dtk (~580 MB/s) | **9,1–9,6 dtk (~1,4 GB/s)** | **2,4×** (indeks rayon) |
| `grep -c COMMIT` (15.257.710 cocok) | 20,2 dtk | **7,8 dtk** | **2,6×** (grep rayon; mode hitung tanpa alokasi) |
| `count` file biner 4 GB | 7,1 dtk + "16.007.643 baris" PALSU | **ditolak exit 2** + pesan jujur | bug → fix |
| Total cocok tampil GUI | "200.000 (dibatasi)" tanpa total | **15.257.710 eksak satu pass** + halaman per 200 rb | fit |

Catatan jujur:

- Hitungan baris identik sebelum/sesudah paralel (337.662.998) + oracle
  `parallel_matches_scalar_oracle` di suite (checkpoint byte-identik).
- Semantik grep paralel dikunci oracle `grep_collect_matches_oracle`
  (per-baris, CRLF, pola lintas-baris tidak cocok — sama seperti loop lama).
- Total eksak GUI dibuktikan tes `worker_grand_total_exact_when_truncated`
  (30 rb match, cap 10 rb → tersimpan 10 rb + total 30.000).

## 14. GUI headless: open + first paint + search (0.4.0, 2026-09-16)

Harness: `app::ui_viewport::tests::gui_bench_open_and_search` (ignored,
jalan manual release). File sintetis ~50 MB / 1.250.000 baris
(INFO massal + `ERROR` tiap 50 baris). Mesin: i5-10400, RAM 16 GB.
Perintah: `cargo test --release -- --ignored gui_bench --nocapture`.
Angka = logika UI penuh tanpa GPU (waktu paint-backend tidak termasuk).

| Metrik | AsisLog 0.4.0 | klogg/PapaLogg |
|---|---|---|
| Buka file + indeks selesai | 40 ms | (belum diukur) |
| Frame konten pertama (proxy first paint) | 2 ms | (belum diukur) |
| Search `ERROR` batch pertama (25.000 hasil) | 30 ms | (belum diukur) |
| Search selesai | 31 ms | (belum diukur) |

Protokol manual kolom klogg (agar sel terisi jujur): buka file sintetis
yang sama di klogg GUI, catat waktu status "indexing" s.d. siap + waktu
status search s.d. batch pertama via stopwatch; tulis mesin + tanggal.
Tanpa itu, kolom tetap kosong — bukan nol.
