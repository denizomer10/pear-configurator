//! Uygulama genelinde kullanilan yollar ve sabitler.

use std::fs;
use std::path::PathBuf;

pub const USER_AGENT: &str = "pear-configurator/0.1";

pub const IPSW_API_BASE: &str = "https://api.ipsw.me/v4";

/// Cihaz veritabani onbellek yasi (saniye).
pub const DEVICE_DB_MAX_AGE: u64 = 7 * 24 * 60 * 60;

/// Apple USB vendor ID.
pub const APPLE_VENDOR_ID: u16 = 0x05ac;

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn base_dir() -> PathBuf {
    std::env::var_os("PEAR_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".pear"))
}

pub fn toolchain_dir() -> PathBuf {
    base_dir().join("toolchain")
}

pub fn bin_dir() -> PathBuf {
    toolchain_dir().join("bin")
}

pub fn lib_dir() -> PathBuf {
    toolchain_dir().join("lib")
}

pub fn download_dir() -> PathBuf {
    std::env::var_os("PEAR_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| base_dir().join("ipsw"))
}

pub fn cache_dir() -> PathBuf {
    base_dir().join("cache")
}

pub fn data_dir() -> PathBuf {
    base_dir().join("data")
}

pub fn log_dir() -> PathBuf {
    base_dir().join("logs")
}

pub fn device_db_cache() -> PathBuf {
    data_dir().join("devices.json")
}

/// Gerekli calisma dizinlerini olusturur.
pub fn ensure_dirs() {
    for dir in [
        base_dir(),
        toolchain_dir(),
        bin_dir(),
        lib_dir(),
        download_dir(),
        cache_dir(),
        data_dir(),
        log_dir(),
    ] {
        let _ = fs::create_dir_all(dir);
    }
}
