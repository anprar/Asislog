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
| Open first paint (dingin, cache OS dibersihkan) | stopwatch s.d. baris pertama tampil |  |  |  |
| Search literal `ERROR` s.d. batch pertama | status `Mencari…` / stopwatch |  |  |  |
| Search regex `Exception\|timeout` full scan | stopwatch |  |  |  |
| RAM puncak saat search ribuan hasil | Task Manager / `UMNS` |  |  |  |
| Scroll tahan PgDn 10 dtk | FPS/stutter kualitatif |  |  |  |

Kontrak performa (dari `asislog-agent-prompt.md`):

- Open first paint < 1 detik (SSD lokal, ukuran file berapa pun)
- RAM < 250 MB untuk log 10 GB + ribuan hasil
- Scroll tanpa pindai ulang per frame; hasil mengalir per batch + cancel

## 3. Status klaim kecepatan (diisi maintainer)

- [ ] Literal search vs klogg di file ≥1 GB: ____ (tanggal, mesin: ____)
- [ ] Regex kompleks vs klogg (Hyperscan): ____ — ekspektasi jujur: klogg
      menang 2–4x sampai AsisLog memakai vectorscan/prefilter Aho-Corasick.
- [ ] Open 10 GB + RAM: ____

Sampai tabel terisi, klaim resmi proyek: **"setara untuk literal,
belum tentu untuk regex kompleks"**.
