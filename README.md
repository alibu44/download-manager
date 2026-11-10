# Download Manager

Rust ile yazılmış, IDM tarzı indirme yöneticisi: dinamik parçalı HTTP indirme, tarayıcı eklentisiyle
indirme ve video yakalama, yt-dlp ile video indirme.

## Özellikler

- **Dinamik parçalama:** 32 bağlantıya kadar. İşi biten bağlantı, kalan en büyük parçanın yarısını devralır;
  bağlantı sayısını sınırlayan sunucularda fazla bağlantılar kendiliğinden çekilir.
- **Kaldığı yerden devam:** parça durumu iki saniyede bir diske yazılır; `If-Range` ve boyut denetimiyle
  değişmiş bir dosyanın eskisiyle birleştirilmesi engellenir.
- **Adres yenileme:** süresi dolan bağlantı yeniden yakalanınca yarım kalan indirme kaldığı yerden sürer.
- **İndirme onayı:** tarayıcıdan yakalanan indirme başlamadan önce ad, klasör ve boyut gösterilir.
- **Kuyruk ve hız sınırı.**
- **Tarayıcı eklentisi (Chrome / Edge):** indirmeleri uygulamaya aktarır, sayfaların yüklediği video/ses
  dosyalarını ve HLS/DASH akışlarını listeler, videoların üzerinde "Bu videoyu indir" düğmesi gösterir.
- **Güvenlik:** yerel API yalnızca `127.0.0.1` üzerinde dinler ve yalnızca uygulamanın kendi penceresinden
  ve bu eklentiden gelen isteklere izin verir; indirilen dosyalara Windows "internetten indirildi" işareti
  (`Zone.Identifier`) yazılır; oturum çerezleri indirme tamamlanınca diskten silinir.

## Derleme

```
cargo build --release
```

Çalışma zamanında exe'nin yanında `yt-dlp.exe` ve `ffmpeg.exe` bulunmalıdır (video indirme için).
Kurulum dosyası [Inno Setup](https://jrsoftware.org/isinfo.php) ile üretilir: `dist\` klasörüne
`DownloadManager.exe`, `yt-dlp.exe` ve `ffmpeg.exe` konur, ardından `ISCC installer\setup.iss` çalıştırılır.

Kaynak kodu Linux'ta da derlenecek şekilde yazılmıştır (GTK/WebKitGTK gerekir) ancak yalnızca Windows'ta
denenmiştir.

## Eklenti kurulumu

`chrome://extensions` (ya da `edge://extensions`) → Geliştirici modu → Paketlenmemiş öğe yükle →
`extension` klasörü. Ayrıntılar: `installer/KURULUM.txt`.

## Yapı

| Yol | İçerik |
|---|---|
| `src/engine.rs` | Parçalı indirme motoru, kuyruk, hız sınırı, kalıcı durum |
| `src/ytdl.rs` | yt-dlp ile video biçimleri ve indirme |
| `src/server.rs` | Yerel HTTP API (`127.0.0.1:52931`) |
| `src/main.rs` | Pencere (WebView2), sistem tepsisi |
| `ui/` | Uygulama arayüzü |
| `extension/` | Tarayıcı eklentisi (Manifest V3) |
| `e2e/` | Eklenti uçtan uca testleri (puppeteer) |
