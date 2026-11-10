# Eklentiyi mağazalarda yayınlama

Paketler `tools\paketle.ps1` ile üretilir ve `store\` klasörüne yazılır:

| Dosya | Nereye yüklenir |
|---|---|
| `download-manager-chrome.zip` | Chrome Web Store (Chrome, Brave, Opera, Vivaldi buradan kurar) ve Microsoft Edge Add-ons |
| `download-manager-firefox.zip` | addons.mozilla.org (Firefox) |

Mağaza sürümünü kuran tarayıcı, uygulamaya ilk bağlandığında Download Manager penceresinde bir kez
"Tarayıcı eklentisi izni" sorusu çıkar; "İzin ver" denince eşleşme kaydedilir. Mağazalar eklentiye kendi
kimliğini verdiği için bu adım gereklidir.

## 1. Chrome Web Store

1. https://chrome.google.com/webstore/devconsole adresinde geliştirici hesabı açın (bir kerelik 5 USD kayıt ücreti).
2. "New item" → `download-manager-chrome.zip` dosyasını yükleyin.
3. Aşağıdaki metinleri, en az bir ekran görüntüsünü (1280x800) ve gizlilik politikası adresini girin.
4. "Privacy practices" bölümünde her izin için aşağıdaki gerekçeleri yazın, "Submit for review" deyin.

İnceleme genellikle birkaç gün sürer; `<all_urls>` ve `cookies` izinleri yüzünden daha uzun sürebilir.

**YouTube:** Chrome Web Store kuralları YouTube'dan video indiren eklentileri kabul etmez. Bu yüzden Chrome/Edge
paketinde YouTube'daki "Bu videoyu indir" düğmesi kapalıdır. Firefox paketinde ve elle yüklenen sürümde açıktır.

## 2. Microsoft Edge Add-ons

https://partner.microsoft.com/dashboard/microsoftedge adresinde ücretsiz geliştirici kaydı → aynı
`download-manager-chrome.zip` dosyası → aynı metinler.

## 3. Firefox

https://addons.mozilla.org/developers adresinde ücretsiz hesap → `download-manager-firefox.zip`.
Kaynak kodu sıkıştırılmamış olduğu için ek kaynak yüklemesi gerekmez. Firefox paketi bu projede henüz bir
Firefox kurulumunda denenmedi; yayınlamadan önce `about:debugging` → "Geçici eklenti yükle" ile deneyin.

## Mağaza metinleri

**Ad:** Download Manager Entegrasyonu

**Kısa açıklama (132 karakter sınırı):**
Tarayıcı indirmelerini Download Manager masaüstü uygulamasına aktarır; sayfalardaki video ve ses akışlarını listeler.

**Ayrıntılı açıklama:**

Bu eklenti, Windows için Download Manager uygulamasının tarayıcı tarafıdır. Tek başına indirme yapmaz;
uygulamanın kurulu ve açık olması gerekir.

Uygulamayı indirin: https://github.com/alibu44/download-manager/releases/latest

Ne yapar?
- Tarayıcıda başlayan indirmeleri uygulamaya aktarır. Uygulama önce dosya adını, klasörü ve boyutu gösterip
  onayınızı ister, sonra dosyayı 32 bağlantıya kadar parçalayarak indirir.
- Sayfaların yüklediği video/ses dosyalarını ve HLS/DASH akışlarını eklenti simgesinde listeler.
- Videoların üzerinde "Bu videoyu indir" düğmesi gösterir.
- Bağlantıya sağ tıklayıp "Download Manager ile indir" seçeneği sunar.
- Uygulama kapalıyken hiçbir şeye karışmaz; indirmeler tarayıcıda normal şekilde yapılır.

Eklenti yalnızca bilgisayarınızdaki uygulamayla (127.0.0.1) konuşur; hiçbir veriyi internete göndermez.

**Kategori:** Productivity / Üretkenlik — **Dil:** Türkçe

## İzin gerekçeleri (mağaza formu için)

| İzin | Gerekçe |
|---|---|
| `downloads` | Başlayan indirmeyi fark edip iptal etmek ve masaüstü uygulamasına devretmek. |
| `cookies` | Giriş gerektiren indirmelerin uygulamada da çalışması için ilgili sitenin çerezlerini yalnızca yerel uygulamaya iletmek. |
| `webRequest` | Sayfanın yüklediği video/ses dosyalarını ve akış listelerini (m3u8/mpd) yanıt başlıklarından tanımak. İstekler değiştirilmez. |
| `tabs` | Yakalanan medyayı sekmeye göre listelemek ve sekme adresini "kaynak sayfa" olarak uygulamaya iletmek. |
| `storage` | "İndirmeleri yakala" ayarını ve sekme başına medya listesini saklamak. |
| `contextMenus` | Sağ tık menüsüne "Download Manager ile indir" eklemek. |
| Tüm siteler (`<all_urls>`) | İndirme ve medya herhangi bir sitede olabilir; video düğmesi sayfaya eklenir. |

**Uzak kod:** Yok. Tüm kod pakettedir.

## Gizlilik politikası

Mağazalar herkese açık bir adres ister. `GIZLILIK.md` dosyasını herkese açık bir sayfada yayınlayın
(örneğin depoyu herkese açık yapıp dosyanın GitHub adresini verin).
