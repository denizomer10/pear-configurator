//! ipsw.me API istemcisi (signed firmware listeleri).
//!
//! Apple Silicon Mac'ler dahil tum cihazlar icin `/v4/device/{id}` uclu
//! kullanilir; yanit icindeki `firmwares` dizisi okunur.

use serde::{Deserialize, Serialize};

use crate::config;
use crate::net;

#[derive(Debug, Clone, Serialize)]
pub struct Firmware {
    pub identifier: String,
    pub version: String,
    pub buildid: String,
    pub url: String,
    pub sha256: String,
    pub sha1: String,
    pub signed: bool,
    pub releasedate: String,
    pub filesize: u64,
    pub filesize_human: String,
}

#[derive(Debug, Deserialize)]
struct RawFirmware {
    identifier: String,
    version: String,
    buildid: String,
    url: String,
    #[serde(default, rename = "sha256sum")]
    sha256sum: String,
    #[serde(default, rename = "sha1sum")]
    sha1sum: String,
    #[serde(default)]
    signed: bool,
    #[serde(default)]
    releasedate: String,
    #[serde(default)]
    filesize: u64,
}

#[derive(Debug, Deserialize)]
struct DeviceResponse {
    #[serde(default)]
    firmwares: Vec<RawFirmware>,
}

/// Bir cihaz tanimlayicisi icin tum firmware surumlerini dondurur.
pub fn get_firmwares(identifier: &str, timeout_secs: u64) -> Vec<Firmware> {
    let url = format!("{}/device/{}", config::IPSW_API_BASE, identifier);
    let body = match net::get_string(&url, timeout_secs) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let parsed: DeviceResponse = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    parsed
        .firmwares
        .into_iter()
        .map(|raw| Firmware {
            identifier: raw.identifier,
            version: raw.version,
            buildid: raw.buildid,
            url: raw.url,
            sha256: raw.sha256sum,
            sha1: raw.sha1sum,
            signed: raw.signed,
            releasedate: raw.releasedate,
            filesize: raw.filesize,
            filesize_human: human_size(raw.filesize),
        })
        .collect()
}

/// En guncel imzali (signed) firmware'i dondurur.
#[allow(dead_code)]
pub fn latest_signed(identifier: &str, timeout_secs: u64) -> Option<Firmware> {
    let mut firmwares = get_firmwares(identifier, timeout_secs);
    firmwares.retain(|f| f.signed);
    firmwares.into_iter().next()
}

pub fn human_size(num: u64) -> String {
    if num == 0 {
        return "-".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = num as f64;
    let mut idx = 0;
    while size >= 1024.0 && idx < units.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} B", num)
    } else {
        format!("{:.1} {}", size, units[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "-");
        assert_eq!(human_size(512), "512 B");
        assert!(human_size(5 * 1024 * 1024 * 1024).ends_with("GB"));
    }
}
