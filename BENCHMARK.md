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
- [ ] Literal search vs klogg di file ≥1 GB: ____ (mesin sama, file sama).
- [ ] Regex kompleks vs klogg (Hyperscan): ____ — ekspektasi jujur: klogg
      menang 2–4x sampai AsisLog memakai vectorscan/prefilter Aho-Corasick.
- [ ] Open 10 GB + RAM <250 MB via GUI: ____ (first paint, RAM puncak).

Sampai tabel terisi, klaim resmi proyek: **"setara untuk literal,
belum tentu untuk regex kompleks"**.
