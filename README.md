# AsisLog

AsisLog adalah penampil log/teks portabel untuk file sangat besar (target 10 GB+).
Hanya **melihat** — tidak mengedit file asli.

Pengguna tipikal: dukungan ERP / Java / Tomcat / Linux yang biasa macet membuka
`catalina.out` di Notepad++.

## Cara menjalankan (pengguna)

Unduh `asislog.exe` (Windows) atau `asislog` (Linux) dari halaman Rilis.
Tidak perlu menginstal Rust, Python, atau Node.

```text
Windows: klik ganda asislog.exe, lalu Buka file log…
Linux:   ./asislog
```

Aplikasi dibuka **maksimal** (fullscreen). Tema awal mengikuti tema
sistem operasi (gelap/terang); bisa diubah manual lewat menu Tema.

Seret file `.log` / `.txt` / `.out` / `.err` ke jendela, atau tekan `Ctrl+O`.

## Cara membangun (pengembang)

```sh
cargo build --release
# Windows: target/release/asislog.exe
# Linux:   target/release/asislog
```

Pengguna tidak perlu menginstal Rust untuk **menjalankan** biner.

## Rilis portable (maintainer)

Biner rilis tanpa jendela console, sudah dioptimasi ukuran (`opt-level=z`,
LTO, strip ≈ 7 MB), berikon + metadata versi (via `build.rs` +
`assets/asislog.ico`), lalu dikompresi UPX `--lzma` (≈ 2,55 MB; budget
CI 6 MB) dan di-ZIP.
CI (`release.yml`, picu tag `v*`) mengerjakan semuanya otomatis: tes →
build Windows+Linux → smoke test `--version` → UPX → ZIP → SHA-256 →
GitHub Release.

Lokal (Windows):

```powershell
cargo build --release
.\target\release\asislog.exe --version   # smoke test
winget install -e --id UPX.UPX          # sekali saja
upx --lzma --best .\target\release\asislog.exe
Compress-Archive .\target\release\asislog.exe asislog-windows.zip -Force
```

Verifikasi keaslian unduhan & Mitigasi Antivirus (Windows Defender SmartScreen / WDSI):

> [!NOTE]
> Biner rilis portabel dikompresi menggunakan UPX dan dirilis tanpa sertifikat penandatangan kode (code-signing certificate EV) berbayar. Karena itu, Windows Defender SmartScreen mungkin menampilkan peringatan heuristik *"Windows protected your PC"* atau *"Unknown Publisher"*. Ini adalah false-positive umum pada utilitas open-source baru.

1. **Verifikasi Hash SHA-256**:
   Cocokkan hash biner dengan file `SHA256SUMS.txt` resmi pada rilis GitHub:
   ```powershell
   # Windows (PowerShell / Command Prompt)
   CertUtil -hashfile asislog.exe SHA256
   ```
   ```sh
   # Linux
   sha256sum -c SHA256SUMS.txt
   ```
2. **Menjalankan jika dicegat SmartScreen**:
   - Pada dialog SmartScreen, klik **"More info"** / **"Info selengkapnya"**, lalu klik **"Run anyway"** / **"Tetap jalankan"**.
   - Atau lewat PowerShell: `Unblock-File .\asislog.exe`.
3. **Pelaporan False-Positive (WDSI)**:
   Pengembang dan pengguna dapat melaporkan deteksi keliru secara resmi ke portal [Microsoft Security Intelligence (WDSI) File Submission](https://www.microsoft.com/en-us/wdsi/filesubmission) dengan memilih kategori *"Incorrectly detected as malware / false positive"*.

CLI & Subcommand:
```text
asislog [FILE]...                  buka file log langsung sebagai tab
asislog grep [-n] [-c] <POLA> <F>  cari pola dalam file log secara instan (exit code 0 jika cocok, 1 jika nihil)
asislog count <FILE>               hitung total baris dan ukuran file instan menggunakan sparse indexer
asislog --version                  tampilkan versi lalu keluar
asislog --help                     tampilkan bantuan ringkas
```

## Penggunaan singkat

- **Buka**: `Ctrl+O`, atau seret file ke jendela. Beberapa file dibuka sebagai tab.
  Menu **Riwayat v** berisi file terakhir + **favorit** (tombol F per baris).
  `Ctrl+Tab` / `Ctrl+Shift+Tab` pindah tab (berlaku juga saat mengetik
  di kolom teks, seperti peramban).
  Arsip `.zip`/`.tar.gz`/`.tgz`/`.tar`/`.gz`/`.bz2`/`.tbz2`/`.xz`/`.txz`/`.7z`
  dibuka otomatis
  (entri teks terbesar diekstrak ke temp; dibersihkan saat tab ditutup).
  Menu **URL/teks v**: **Buka URL…** (unduh http(s) ke temp) dan
  **Tempel teks…** (Ctrl+V di dialog, buka sebagai file temp).
- **Cari** (pusat alur kerja): kolom pencarian selebar jendela.
  Ketik teks, exception, atau request ID (debounce 150 ms).
  - Satu kata: jalur literal cepat (SIMD). Info hasil menyebut modenya
    (`literal` / `regex` / `boolean`).
  - Boolean (mode literal, tanpa centang regex): `err timeout -debug`,
    `"order service"`, `a OR b`, `(a OR b) NOT c`, `a|b`.
    Chip di bawah kolom menampilkan token AND; × pada chip membuang token.
    Regex (`.*`) selalu berarti satu pola utuh.
  - History global: saat kolom fokus, maksimal 6 pola lama yang cocok
    (fuzzy) tampil untuk sekali klik. Tersimpan di config (30 terakhir).
  - Cache: mengulang pola sama pada file yang tak berubah = instan
    (status `Hasil dari cache`), otomatis gugur bila file tumbuh/dirotasi
    atau encoding diganti.
  Centang **Aa** untuk peka huruf besar/kecil, **.\*** untuk regex;
  regex salah tampil sebagai galat status.
  Selama memindai tampil progres `Mencari… x / y · N hasil`;
  bila nihil tampil `Tidak ada kecocokan untuk "…"`.
  Tombol **Jadikan filter** aktif hanya bila query tidak kosong.
- **Prev/Next**: **‹ Sebelumnya** / **Berikutnya ›** (`Shift+F3`/`F3`,
  atau `N`/`n` di luar kolom ketik) menelusuri daftar hasil tanpa memindai
  ulang. **Batalkan pencarian** menghentikan pindaian yang berjalan.
- **Cakupan**: tombol **Cakupan…** membatasi pencarian ke rentang baris
  (hemat untuk file 10 GB; bongkah di luar dilewati, cocok tepi difilter).
- **Mode tampil**: **Tampil: Semua/Hasil/Penanda** — viewport hanya
  menampilkan baris hasil pencarian atau penanda (nomor gutter tetap asli).
- **Preset pencarian**: menu **Preset ▾** berisi preset bawaan
  (Java: ERROR/FATAL/Exception/timeout/…, SQL: Checkpoint/Transaksi/
  Rollback/Commit/INSERT) dan simpanan sendiri via
  **Simpan pencarian saat ini…** (tersimpan di config global).
- **History**: `Alt+Left` / `Alt+Right` kembali/maju antar lokasi
  investigasi (maks 200); tombol ← → di bilah navigasi.
- **Penanda persisten**: klik nomor baris / `Ctrl+B` menandai,
  `F2` ubah label + warna, `Ctrl+Shift+B` buka panel (bisa disaring),
  `Alt+↑`/`Alt+↓` pindah antar penanda. Tersimpan di
  `<file>.asislog.json` (beserta ukuran, waktu, sidik head file);
  bila file sumber berubah muncul peringatan, bukan data hilang.
- **Panel hasil**: collapsed (header saja) saat belum mencari agar
  viewport log lega; terbuka otomatis saat ada hasil, bisa diciutkan
  (▲/▼), diubah tingginya dengan menyeret, atau dibersihkan (×).
- **Filter**: isi kolom Filter lalu **Terapkan**.
  Token dipisah spasi (AND); awalan `-` berarti kecualikan.
  Contoh: `ERROR`, `ERROR -DEBUG`, `OrderService`.
  Token `kunci=nilai` mencocokkan field baris JSON (mis. `level=ERROR`,
  `-level=DEBUG`, tak peka huruf).
  Tombol **?** menampilkan bantuan; saat aktif muncul chip tegas
  `Filter aktif: … · n/m baris` + tombol **Hapus**.
  Nomor baris di gutter tetap nomor asli.
- **Ikuti (follow/tail)**: tombol **Ikuti akhir file** / **LIVE**
  (atau `Ctrl+Shift+F`) di baris navigasi.
  AsisLog memantau pertumbuhan file tiap ~500 ms; hanya ekor baru yang diindeks.
  Identitas = ukuran + sidik head, sehingga tulis-ulang berukuran sama
  terbaca sebagai rotasi (buka ulang), bukan append.
  Bila file dipotong/dirotasi, dibuka ulang dari awal dengan peringatan status.
  Gulir ke atas menjeda LIVE; tombol Akhir / LIVE mengunci lagi.
- **Ke baris**: tombol **Ke baris…** (`Ctrl+G`). Terima nomor baris (`38166903`),
  persen (`50%`), `akhir`/`awal`, atau cap waktu (`2026-08-24 15:48:43`)
  bila baris berawalan waktu monotonik.
- **Salin**: menu **Salin v** — baris ini, 50 baris, +nomor
  (`baris: isi`), simpan 200 baris ke file, salin sebagai `path:baris`.
  Batas papan klip 16 MB; bila lebih, gunakan ekspor.
- **Menu Salin blok v**: salin/ekspor blok **SQL**, **transaksi**
  (`BEGIN TRANSACTION` s.d. `COMMIT`/`ROLLBACK`/`go`), dan **checkpoint**
  (`--START` s.d. `--FINISH CHECKPOINT`) ke papan klip / file.
  Ekspor menulis baris polos (tanpa nomor) agar bisa dijalankan.
- **Sorotan kustom**: tombol **Sorotan…** mengelola **set bernama**
  per produk (mis. ERP-Java, SQL): pilih/buat/hapus set, tambah aturan
  (teks/regex, 9 warna, match atau baris penuh), **Ekspor/Impor set…**
  via file JSON untuk dishare. Hanya dievaluasi pada baris terlihat.
  **Label cepat**: dengan query aktif, tekan `1`–`9` untuk menjadikan
  query aturan sorotan warna itu (tekan lagi untuk hapus).
  Tersimpan di config global; set aktif ikut tersimpan di workspace.
- **Rentang waktu**: tombol **Rentang waktu…** menampilkan hanya baris
  dalam rentang cap waktu (binary-search bila monotonik).
- **Penanda**: klik `*` gutter / `Ctrl+B` untuk menambah/menghapus.
- **Salin**: tombol **Salin baris ini** / **Salin 50 baris**.
  Batas papan klip 16 MB; bila lebih, gunakan **Ekspor…**.
- **Ekspor**: **Ekspor hasil…** menyimpan hasil (+ N baris konteks, bawaan 10)
  ke file baru secara streaming; **Tiket Markdown (Jira)** menyimpan hasil +
  konteks sebagai Markdown siap paste (nama file, query, nomor baris + hash).
  File sumber tidak pernah diubah.
- **Mode JSON**: baris objek JSON otomatis diringkas inline
  (`LEVEL pesan k=v …`) di viewport dan panel hasil
  (teks asli tetap dipakai untuk salin/ekspor).
- **Workspace**: menu **Workspace v** — **Simpan workspace…** menulis 1 file
  JSON (daftar log + filter/rentang tab aktif + salinan set sorotan),
  cocok dishare via git; **Buka workspace…** membuka semuanya sekaligus.
- **Encoding**: kotaknya saja (tanpa label). Otomatis = deteksi BOM +
  detector chardetng (port Mozilla, pure Rust) atas sampel 64 KiB;
  tebakan single-byte dipetakan ke tampilan Windows-1252.
  Override manual: UTF-8 / Windows-1252 / UTF-16 LE/BE.
- **Sesi**: tab + posisi + pencarian + filter + follow + encoding
  tersimpan otomatis (debounce 10 dtk) ke `session.json` di direktori
  config, ditulis lagi saat keluar, dan dipulihkan saat start
  (file yang hilang dilewati dengan catatan).
- **Tema**: menu Tema di bilah atas — **Sistem (otomatis)** (bawaan,
  mengikuti OS) / **Gelap** / **Terang** / **Terang kontras** /
  **Kontras tinggi** / **Senja biru** / **Solarized gelap** /
  **Solarized terang** / **Monokai**. Pilihan tersimpan di config.
  Warna sorotan ERROR/WARN/INFO dan baris terpilih menyesuaikan tema;
  toggle aktif (Aa, .*, LIVE) memakai gaya terpilih yang tegas.
  Semua ikon tombol digambar vektor (tanpa font simbol) sehingga tidak
  ada kotak tofu di Windows/Linux mana pun.
- **Bahasa / Language**: menu **Bahasa** di bilah atas beralih
  Indonesia ⇄ English seketika, tersimpan di `config.json` (portabel,
  tanpa instal ulang). Palet (`Ctrl+Shift+P`) → *Ganti bahasa /
  Switch language* bisa dipakai dari mode Zen. CLI:
  `asislog --lang en --help` atau `ASISLOG_LANG=en`.
  Format ekspor tiket Markdown stabil dua bahasa agar tetap bisa di-grep.
- **Language**: the **Bahasa** menu in the top bar switches
  Indonesian ⇄ English instantly, persisted to portable `config.json`.
  Palette (`Ctrl+Shift+P`) → *Ganti bahasa / Switch language* also works
  from Zen mode. CLI: `asislog --lang en --help` or `ASISLOG_LANG=en`.
  First run (GUI) without saved config follows the OS locale (`LANG`/`LC_*`;
  English for unknown locales, Indonesian when undetectable as before —
  Windows display language needs OS APIs, so it stays Indonesian default).
  Panel hasil dan penanda bisa diubah ukurannya dengan menyeret pembatasnya;
  baris log yang panjang digulir mendatar agar tidak terpotong.
- **Mode Zen (`F11`)**: Memadatkan seluruh bilah toolbar dan header 5-baris menjadi 1-baris ramping (22px) untuk memaksimalkan ruang baca log. Menekan `Ctrl+F` di mode Zen memunculkan jendela pencarian HUD melayang (floating search bar).
- **Command Palette (`Ctrl+Shift+P`)**: Akses cepat ke seluruh fitur aplikasi melalui dialog pencarian fuzzy keyboard-first (buka file, simpan workspace, ganti tema, toggle Zen, ekspor, goto baris, scratchpad).
- **Histogram Waktu ERROR**: Panel interaktif di bawah viewport yang memetakan lonjakan frekuensi ERROR per menit. Klik pada bar histogram mana pun untuk melompatkan viewport langsung ke rentang waktu tersebut.
- **Investigasi Top-N**: 1-klik untuk mengagregasi 10 error atau ID sesi/thread paling sering muncul dengan memori terkendali (< 20 MB). Tombol *Saring* instan mengubah error terpilih menjadi filter pencarian.
- **Hex Peek**: Mode inspeksi biner aman untuk file log korup atau file non-teks, menampilkan alamat offset, 16 hex byte, dan karakter ASCII yang aman.
- **Kolom SQL**: Pengurai otomatis struktur log query (Waktu, Sesi/Thread, Aksi SQL, Perintah Query) menjadi kolom tabel yang rapi dan mudah diinspeksi.
- **Peek Preview Peta**: Arahkan kursor (hover) pada strip peta kepadatan di tepi kanan layar untuk melihat tooltip cuplikan baris asli secara instan.
- **Zoom**: `Ctrl+=` / `Ctrl+-` / `Ctrl+0` (tersimpan di config).
- **Scratchpad**: tombol **Catatan** — tab catatan + transform
  (JSON rapi, Base64 decode, JWT decode, SQL rapi); isi tersimpan di config.
- **Korelasi**: **Ke semua tab** menerapkan filter aktif ke semua tab;
  checkbox **Semua tab** di dialog Ke baris melompatkan semua tab
  ke baris/persen/cap waktu yang sama.
- **Hasil**: baris hasil menampilkan `#n · baris N · [cap waktu] · cuplikan`
  agar konteks bisa dipindai. Klik melompat ke baris log + sorotan dengan
  posisi ~40% dari atas viewport, **Tampilkan ±20 baris** menggeser ke konteks.
- **Strip peta** di kanan viewport: merah = bucket mengandung ERROR,
  kuning = WARN (512 bucket, dipindai latar), teal = hasil pencarian,
  biru = penanda, hijau = hasil aktif, putih = posisi kini,
  arsir merah = kepadatan ERROR per menit (histogram waktu).
  Arahkan kursor untuk legenda + jumlah; klik strip melompat ke posisi.
  Di sebelahnya track scrollbar kustom setinggi viewport dengan thumb
  proporsional (klik/drag untuk gulir).
- **Viewport log**: gutter dinamis rata kanan + separator, hover dan
  baris terpilih jelas, sorotan token per baris terlihat
  (ERROR/FATAL/Exception merah, WARN kuning, timestamp kebiruan,
  stack trace `at …` redup), gulir mendatar untuk baris panjang,
  strip peta kepadatan di kanan (kuning = hasil, biru = penanda,
  hijau = aktif, putih = posisi; klik untuk melompat), dan indikator
  `baris X/Y · Z%` di kanan bawah viewport.
- **Status**: grup ringkas — keadaan (Siap/Mengindeks/LIVE), ukuran file,
  jumlah baris, encoding, hasil cari, filter, posisi. Contoh saat mengindeks:
  `◌ Mengindeks 64% · 24.702.831 baris terdeteksi`.
- **Pintasan** (daftar lengkap juga di tombol **?** / `F1`):
  `Ctrl+O` buka · `Ctrl+F` fokus cari · `F3`/`Shift+F3` next/prev ·
  `n`/`N` next/prev (di luar kolom ketik) · `1`–`9` label warna ·
  `Ctrl+G` ke baris · `Ctrl+E` ekspor hasil · `Ctrl+Tab` pindah tab ·
  `Ctrl+Home`/`Ctrl+End` awal/akhir · `Ctrl+Shift+F` ikuti ·
  `Ctrl+B` penanda · `Ctrl+Shift+B` panel penanda ·
  `F2` label penanda · `Alt+Left/Right` history · `Alt+Atas/Bawah` penanda ·
  `Ctrl+=`/`Ctrl+-`/`Ctrl+0` zoom · `Esc` batal.

## Kinerja dan batas

- Buka melukis layar pertama dari byte 0 selagi indeks latar berjalan.
- Indeks jarang (checkpoint tiap 1024 baris / 64 KiB) + sampingan
  `namafile.log.asisidx` (boleh dihapus; dibangun ulang bila ukuran/waktu berubah).
- Pencarian dipindai per bongkah 4 MiB di utas latar, hasil dialirkan
  200–500 per batch; kueri baru membatalkan pekerjaan lama (`search_gen`).
- Hanya baris terlihat yang didekode (lossy, tak pernah panik).
- Anggaran RAM: jauh di bawah ukuran file (< 250 MB untuk log 10 GB
  dengan ribuan hasil; hasil dibatasi 200.000).

## Struktur kode

- `src/app/` — UI egui (dulu 1 file `app.rs` 5.316 baris, kini 15 modul <800 baris):
  `tab` (state per tab), `jobs_index`/`jobs_search` (pekerja latar),
  `state` (config/sesi/workspace), `actions` (pintasan/goto/salin),
  `ui` + `ui_*` (render per panel/dialog).
- `src/engine/` — mesin tanpa UI: `mmap`, `index` (checkpoint jarang),
  `search`, `filter`, `follow`, `decode`, `archive`, `query`, `jsonlog`,
  `marks`, `scratch`.
- `tests/` — `engine_integration.rs` (CRLF, checkpoint, filter, follow,
  search-gen, sidecar) + `bench_synthetic.rs` (bench 20 MB di CI,
  1 GB lokal via `--ignored`). Lihat `BENCHMARK.md` untuk metodologi
  perbandingan jujur vs klogg.

Lihat `asislog-agent-prompt.md` untuk spesifikasi awal.
