# PE Configurator

Apple cihazlarını **DFU** ya da **kurtarma (recovery)** modunda tek tıkla geri
yükleyen (restore) masaüstü uygulaması.

- **Diller:** Rust + [Slint](https://slint.dev) (native masaüstü arayüz)
- **Platform:** Linux (Wayland / X11)
- **Desteklenen cihazlar:** iPhone, iPad, iPod, Apple TV ve **Apple Silicon Mac**

libimobiledevice ve restorekit projelerinden ilham almıştır.

## Ne yapar?

1. **Algılar:** USB'ye bağlı Apple cihazını ve modunu (DFU / kurtarma) anında görür.
2. **Tanır:** Cihazın modelini otomatik belirler (ör. Mac mini M1 → `Macmini9,1`).
3. **Bulur:** ipsw.me üzerinden cihaza uygun **imzalı** sürümleri listeler.
4. **İndirir:** Seçilen IPSW yoksa indirir (kaldığı yerden devam + SHA-256 doğrulama).
5. **Kurar:** `idevicerestore` ile restore eder; ilerlemeyi canlı gösterir.

İki restore modu vardır:

| Buton | Etki |
| --- | --- |
| **Flaşı Sil ve Kur** | Tüm veriyi siler, sıfırdan kurar (`-e`) |
| **Veriyi Koru** | Güncelleme / revive (veriyi silmez) |

## Gereksinimler

- Linux (Wayland veya X11 masaüstü)
- Rust (rustup ile kurulabilir)
- Sistem kütüphaneleri: `libfontconfig1-dev`, `libfreetype-dev` (derleme için)
- `idevicerestore` ve `irecovery` (aşağıdaki betikle kurulur)
- DFU cihazlarına USB erişimi için udev kuralı

## Kurulum

### 1) Arayüzü derleyin

```sh
sudo apt-get install -y build-essential pkg-config libfontconfig1-dev libfreetype-dev
cargo build --release
```

### 2) Apple araç zincirini kurun

```sh
scripts/install-toolchain.sh --deps   # derleme bağımlılıkları (sudo ister)
scripts/install-toolchain.sh          # idevicerestore + irecovery derlenir
```

Araçlar `~/.pear/toolchain` altına kurulur (root gerekmez).

### 3) USB erişim kuralı (udev)

```sh
sudo install -m 644 scripts/99-pear-apple.rules /etc/udev/rules.d/99-pear-apple.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Kullanıcı `plugdev` grubunda olmalıdır.

## Çalıştırma

```sh
./target/release/pear-configurator
```

Pencere açılmazsa (GPU'suz ortam) yazılım renderer'ı zorlayın:

```sh
SLINT_BACKEND=winit-software ./target/release/pear-configurator
```

Ek komutlar:

```sh
./target/release/pear-configurator --selftest   # cihaz algılama + model + firmware testi (GUI'siz)
./target/release/pear-configurator --download   # IPSW'yi önceden indir + doğrula
```

## Kullanım

1. Cihazı USB ile bağlayıp **DFU** ya da **kurtarma** moduna alın.
2. Program cihazı, modunu ve modelini otomatik gösterir.
3. **Sürüm (IPSW)** listesinden bir sürüm seçin (imzalılar önerilir).
4. **Flaşı Sil ve Kur** (veya **Veriyi Koru**) düğmesine basın.
5. Gerekirse IPSW indirilir, ardından restore ilerlemesi izlenir.

## Dosya konumları

| Ne | Yer |
| --- | --- |
| Araç zinciri | `~/.pear/toolchain` |
| İndirilen IPSW | `~/.pear/ipsw` |
| Önbellek / veritabanı | `~/.pear/cache`, `~/.pear/data` |
| Günlükler | `~/.pear/logs` |

`PEAR_HOME` ve `PEAR_DOWNLOAD_DIR` ortam değişkenleriyle değiştirilebilir.

## Testler

```sh
cargo test
```

## Uyarılar

- **Flaşı Sil ve Kur geri alınamaz**, cihazdaki tüm veri silinir.
- Apple Silicon Mac restore'u güncel `idevicerestore` gerektirir (bu yüzden
  araçlar kaynaktan derlenir).
- `irecovery` bulunamazsa cihaz yine de USB üzerinden (CPID/BDID) tanımlanır.

## Lisans

**GNU General Public License v2.0 (GPLv2)** — bkz. [LICENSE](LICENSE).
