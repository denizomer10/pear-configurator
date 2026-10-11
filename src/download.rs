//! IPSW indirme motoru.
//!
//! - Akis halinde indirir ve ilerleme geri cagirimi tetikler.
//! - Yarim kalan indirmeleri `Range` basligi ile devam ettirir.
//! - Gecici hatalarda (baglanti kopmasi / zaman asimi) otomatik yeniden dener
//!   ve kaldigi yerden devam eder; islem sonunda SHA-256 ozetini dogrular.
//!
//! Onemli: ureq'in `Request::timeout` degeri istegin **tumune** uygulanan bir
//! son tarihtir; 20+ GB'lik bir IPSW icin bu uygun degildir. Bunun yerine
//! ajan seviyesinde yalnizca baglanti ve okuma (stall) zaman asimlari verilir.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::config;

const CHUNK_SIZE: usize = 1024 * 512; // 512 KB
const MAX_ATTEMPTS: u32 = 100;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(90);

/// Indirme iptal edildiginde donen hata metni.
pub const CANCELLED: &str = "__cancelled__";

pub struct DownloadResult {
    pub path: PathBuf,
    #[allow(dead_code)]
    pub bytes_total: u64,
    pub verified: bool,
}

enum AttemptError {
    Cancelled,
    Fatal(String),
    Transient(String),
}

fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(READ_TIMEOUT)
            .user_agent(config::USER_AGENT)
            .build()
    })
}

fn part_path(destination: &Path) -> PathBuf {
    let mut os = destination.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

/// Bir dosyanin SHA-256 ozetini dogrular.
pub fn verify_sha256(path: &Path, expected: &str) -> bool {
    if expected.is_empty() {
        return true;
    }
    match sha256_file(path) {
        Some(actual) => actual.eq_ignore_ascii_case(expected),
        None => false,
    }
}

fn sha256_file(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn classify(error: ureq::Error) -> AttemptError {
    match error {
        ureq::Error::Status(code, _) => {
            // 4xx (408/429 haric) kalici; 5xx ve digerleri gecici.
            if (400..500).contains(&code) && code != 408 && code != 429 {
                AttemptError::Fatal(format!("HTTP {}", code))
            } else {
                AttemptError::Transient(format!("HTTP {}", code))
            }
        }
        ureq::Error::Transport(t) => AttemptError::Transient(t.to_string()),
    }
}

/// Tek bir indirme denemesi; basarida `part` dosyasini `destination`'a tasir.
fn download_once(
    url: &str,
    destination: &Path,
    resume: bool,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), AttemptError> {
    if let Some(parent) = destination.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let part = part_path(destination);

    let mut existing: u64 = if resume {
        fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
    } else {
        let _ = fs::remove_file(&part);
        0
    };

    let mut request = agent().get(url);
    if existing > 0 {
        request = request.set("Range", &format!("bytes={}-", existing));
    }

    let resp = match request.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(416, _)) if existing > 0 => {
            // Dosya zaten tam; dogrulama karar verecek.
            let _ = fs::rename(&part, destination);
            return Ok(());
        }
        Err(e) => return Err(classify(e)),
    };

    let status = resp.status();
    if existing > 0 && status != 206 {
        existing = 0;
        let _ = fs::remove_file(&part);
    }

    let content_length: Option<u64> = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let total = content_length.map(|c| c + existing).unwrap_or(0);

    let mut reader = resp.into_reader();
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(existing > 0)
        .truncate(existing == 0)
        .open(&part)
        .map_err(|e| AttemptError::Fatal(format!("Dosya acilamadi: {}", e)))?;

    let mut done = existing;
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AttemptError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => return Err(AttemptError::Transient(format!("Okuma hatasi: {}", e))),
        };
        if n == 0 {
            break;
        }
        if let Err(e) = file.write_all(&buf[..n]) {
            return Err(AttemptError::Fatal(format!("Yazma hatasi: {}", e)));
        }
        done += n as u64;
        progress(done, total);
    }
    drop(file);

    fs::rename(&part, destination).map_err(|e| AttemptError::Fatal(format!("Yeniden adlandirma hatasi: {}", e)))?;
    Ok(())
}

/// Dosyayi indirir; ilerleme `progress(downloaded, total)`, durum mesajlari
/// `note(text)` ile bildirilir. Gecici hatalarda kaldigi yerden devam eder.
pub fn download(
    url: &str,
    destination: &Path,
    expected_sha256: &str,
    resume: bool,
    progress: &dyn Fn(u64, u64),
    note: &dyn Fn(&str),
    cancel: &AtomicBool,
) -> Result<DownloadResult, String> {
    let mut use_resume = resume;
    let mut attempts: u32 = 0;

    loop {
        attempts += 1;
        match download_once(url, destination, use_resume, progress, cancel) {
            Ok(()) => {
                if verify_sha256(destination, expected_sha256) {
                    let bytes_total = fs::metadata(destination).map(|m| m.len()).unwrap_or(0);
                    return Ok(DownloadResult {
                        path: destination.to_path_buf(),
                        bytes_total,
                        verified: true,
                    });
                }
                // Bozuk/eksik: bastan indir.
                let _ = fs::remove_file(destination);
                use_resume = false;
                if attempts >= MAX_ATTEMPTS {
                    return Err("SHA-256 dogrulanamadi".to_string());
                }
                note("Dogrulama basarisiz, bastan indiriliyor...");
            }
            Err(AttemptError::Cancelled) => return Err(CANCELLED.to_string()),
            Err(AttemptError::Fatal(msg)) => return Err(msg),
            Err(AttemptError::Transient(msg)) => {
                if attempts >= MAX_ATTEMPTS {
                    return Err(format!("{} ({} deneme sonrasi)", msg, attempts));
                }
                note(&format!(
                    "Baglanti sorunu ({}), kaldigi yerden devam ediliyor... ({}/{})",
                    msg, attempts, MAX_ATTEMPTS
                ));
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn sha256_verify_matches() {
        let dir = std::env::temp_dir().join("pear-test-sha");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("data.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"hello").unwrap();
        drop(f);
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(verify_sha256(&path, expected));
        assert!(!verify_sha256(&path, "deadbeef"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_encode_basic() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x10]), "00ff10");
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/tmp/foo.ipsw")),
            PathBuf::from("/tmp/foo.ipsw.part")
        );
    }
}
