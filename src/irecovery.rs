//! `irecovery` ikili cagri sarmalayicisi.
//!
//! libimobiledevice kurulumuyla gelen `irecovery` araci, DFU/kurtarma modundaki
//! cihazdan CPID, BDID, ECID ve MODEL gibi bilgileri okur.  Bulunamazsa ust
//! katman USB tarafindan cikarilan bilgilerle yetinir.

use std::path::PathBuf;
use std::process::Command;

use crate::config;

#[derive(Debug, Clone, Default)]
pub struct IRecoveryInfo {
    pub cpid: Option<u32>,
    pub bdid: Option<u32>,
    pub ecid: Option<u64>,
    pub model: String,
    pub srtg: String,
}

/// `irecovery` ikilisini bulur (ortam degiskeni > toolchain > PATH).
pub fn find_irecovery() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("PEAR_IRECOVERY") {
        let path = PathBuf::from(env);
        if path.exists() {
            return Some(path);
        }
    }
    let local = config::bin_dir().join("irecovery");
    if local.exists() {
        return Some(local);
    }
    which("irecovery")
}

#[allow(dead_code)]
pub fn is_available() -> bool {
    find_irecovery().is_some()
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `irecovery -q` ciktisini ayristirir.
///
/// Degerler `0x` onekli ya da oneksiz hex olabilir (or. `CPID: 0x8103`).
pub fn parse_query_output(text: &str) -> IRecoveryInfo {
    let mut info = IRecoveryInfo::default();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_lowercase();
            let value = value.trim();
            match key.as_str() {
                "cpid" => info.cpid = parse_hex_u32(value),
                "bdid" => info.bdid = parse_hex_u32(value),
                "ecid" => info.ecid = parse_hex_u64(value),
                "model" => info.model = value.to_string(),
                "srtg" => info.srtg = value.to_string(),
                _ => {}
            }
        }
    }
    info
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    u32::from_str_radix(strip_hex_prefix(value.trim()), 16).ok()
}

fn parse_hex_u64(value: &str) -> Option<u64> {
    u64::from_str_radix(strip_hex_prefix(value.trim()), 16).ok()
}

/// Bagli cihazi sorgular; basarisizlikta `None` dondurur.
pub fn query(ecid: Option<u64>) -> Option<IRecoveryInfo> {
    let tool = find_irecovery()?;
    let mut cmd = Command::new(tool);
    cmd.arg("-q");
    if let Some(ecid) = ecid {
        cmd.arg("-e").arg(format!("0x{:x}", ecid));
    }
    // Toolchain kutuphanelerini bulmak icin LD_LIBRARY_PATH ayarla.
    let lib = config::lib_dir();
    let mut ld = lib.to_string_lossy().to_string();
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        ld.push(':');
        ld.push_str(&existing.to_string_lossy());
    }
    cmd.env("LD_LIBRARY_PATH", ld);

    let output = cmd.output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let info = parse_query_output(&stdout);
    if info.cpid.is_none() && info.ecid.is_none() && info.model.is_empty() {
        return None;
    }
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample() {
        let sample = "CPID: 8103\nBDID: 22\nECID: 00001234567890ab\nMODEL: J274AP\n";
        let info = parse_query_output(sample);
        assert_eq!(info.cpid, Some(0x8103));
        assert_eq!(info.bdid, Some(0x22));
        assert_eq!(info.ecid, Some(0x0012_3456_7890_ab));
        assert_eq!(info.model, "J274AP");
    }

    #[test]
    fn parse_empty() {
        let info = parse_query_output("");
        assert_eq!(info.cpid, None);
    }

    #[test]
    fn parse_with_0x_prefix() {
        let sample = "CPID: 0x8103\nBDID: 0x22\nECID: 0x0019492036c0801e\nSRTG: iBoot-1\n";
        let info = parse_query_output(sample);
        assert_eq!(info.cpid, Some(0x8103));
        assert_eq!(info.bdid, Some(0x22));
        assert_eq!(info.ecid, Some(0x0019_4920_36c0_801e));
    }
}
