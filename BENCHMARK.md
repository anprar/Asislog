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

Kontrak performa (dari `asislog-agent-prompt.md`):

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
