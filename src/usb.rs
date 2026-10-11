//! Linux sysfs uzerinden USB cihaz taramasi ve mod belirleme.
//!
//! libusb/libusbmuxd kurulu olmasa bile calisir.  Apple cihazlari vendor ID
//! (0x05ac) ile bulunur; mod bilgisi USB `product` tanimlayicisindan, yonga
//! bilgisi ise DFU/kurtarma modundaki `serial` tanimlayicisindan cikarilir.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::config;

const USB_DEVICES_DIR: &str = "/sys/bus/usb/devices";

/// Bilinen Apple DFU / Recovery product ID'leri (urun stringi yoksa yedek).
const DFU_PIDS: [u16; 2] = [0x1227, 0x1222];
const RECOVERY_PIDS: [u16; 4] = [0x1280, 0x1281, 0x1282, 0x1283];
const RESTORE_PIDS: [u16; 3] = [0x1223, 0x1224, 0x1225];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UsbDevice {
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: String,
    pub manufacturer: String,
    pub serial: String,
    pub busnum: Option<u32>,
    pub devnum: Option<u32>,
    pub sysfs_path: String,
    pub mode: String,
    pub cpid: Option<u32>,
    pub bdid: Option<u32>,
    pub ecid: Option<u64>,
    pub srtg: String,
}

impl UsbDevice {
    pub fn mode_label(&self) -> &'static str {
        match self.mode.as_str() {
            "dfu" => "DFU Modu",
            "recovery" => "Kurtarma Modu",
            "restore" => "Restore Modu",
            "wtf" => "WTF Modu",
            "normal" => "Normal Mod",
            _ => "Bilinmeyen Mod",
        }
    }

    pub fn is_restorable(&self) -> bool {
        matches!(self.mode.as_str(), "dfu" | "recovery" | "restore")
    }

    pub fn vid_pid(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor_id, self.product_id)
    }
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_hex(path: &Path) -> Option<u32> {
    let text = read_text(path);
    u32::from_str_radix(text.trim(), 16).ok()
}

fn read_dec(path: &Path) -> Option<u32> {
    read_text(path).trim().parse::<u32>().ok()
}

/// Urun stringi ve PID'e gore cihaz modunu belirler.
pub fn classify_mode(product: &str, product_id: u16) -> &'static str {
    let text = product.to_lowercase();
    if text.contains("dfu") {
        return "dfu";
    }
    if text.contains("recovery") {
        return "recovery";
    }
    if text.contains("restore") {
        return "restore";
    }
    if text.contains("wtf") {
        return "wtf";
    }
    if DFU_PIDS.contains(&product_id) {
        return "dfu";
    }
    if RECOVERY_PIDS.contains(&product_id) {
        return "recovery";
    }
    if RESTORE_PIDS.contains(&product_id) {
        return "restore";
    }
    if text.contains("apple mobile device") {
        return "normal";
    }
    "unknown"
}

/// DFU/kurtarma seri numarasindan yonga bilgisini ayristirir.
///
/// Ornek: `SDOM:01 CPID:8103 BDID:22 ECID:0019492036C0801E SRTG:[iBoot-5540.]`
pub fn parse_serial_info(serial: &str) -> (Option<u32>, Option<u32>, Option<u64>, String) {
    let mut cpid = None;
    let mut bdid = None;
    let mut ecid = None;
    let mut srtg = String::new();

    for token in serial.split_whitespace() {
        if let Some((key, value)) = token.split_once(':') {
            match key {
                "CPID" => cpid = u32::from_str_radix(value, 16).ok(),
                "BDID" => bdid = u32::from_str_radix(value, 16).ok(),
                "ECID" => ecid = u64::from_str_radix(value, 16).ok(),
                "SRTG" => srtg = value.trim_matches(['[', ']']).to_string(),
                _ => {}
            }
        }
    }
    (cpid, bdid, ecid, srtg)
}

/// Bagli tum Apple USB cihazlarini dondurur.
pub fn scan() -> Vec<UsbDevice> {
    let base = Path::new(USB_DEVICES_DIR);
    let mut devices = Vec::new();

    let entries = match fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return devices,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.contains(':') {
            continue; // arayuz girdisi
        }
        let path = entry.path();
        let vendor_id = match read_hex(&path.join("idVendor")) {
            Some(v) => v,
            None => continue,
        };
        let product_id = match read_hex(&path.join("idProduct")) {
            Some(p) => p,
            None => continue,
        };
        if vendor_id != config::APPLE_VENDOR_ID as u32 {
            continue;
        }
        let product_id = product_id as u16;

        let product = read_text(&path.join("product"));
        let manufacturer = read_text(&path.join("manufacturer"));
        let serial = read_text(&path.join("serial"));
        let mode = classify_mode(&product, product_id);
        let (cpid, bdid, ecid, srtg) = parse_serial_info(&serial);

        devices.push(UsbDevice {
            vendor_id: vendor_id as u16,
            product_id,
            product: if product.is_empty() {
                format!("Apple device {:04x}", product_id)
            } else {
                product
            },
            manufacturer: if manufacturer.is_empty() {
                "Apple Inc.".to_string()
            } else {
                manufacturer
            },
            serial,
            busnum: read_dec(&path.join("busnum")),
            devnum: read_dec(&path.join("devnum")),
            sysfs_path: path.to_string_lossy().to_string(),
            mode: mode.to_string(),
            cpid,
            bdid,
            ecid,
            srtg,
        });
    }

    devices.sort_by(|a, b| a.sysfs_path.cmp(&b.sysfs_path));
    devices
}

/// Yalnizca DFU/Recovery/Restore modundaki cihazlari dondurur.
#[allow(dead_code)]
pub fn scan_restorable() -> Vec<UsbDevice> {
    scan().into_iter().filter(|d| d.is_restorable()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_from_product_string() {
        assert_eq!(classify_mode("Apple Mobile Device (DFU Mode)", 0x1227), "dfu");
        assert_eq!(classify_mode("Apple Mobile Device (Recovery Mode)", 0x1281), "recovery");
        assert_eq!(classify_mode("Apple Mobile Device (Restore Mode)", 0x1223), "restore");
        assert_eq!(classify_mode("Apple Mobile Device", 0x12a8), "normal");
        assert_eq!(classify_mode("Other", 0x0001), "unknown");
    }

    #[test]
    fn mode_from_pid_fallback() {
        assert_eq!(classify_mode("", 0x1227), "dfu");
        assert_eq!(classify_mode("", 0x1281), "recovery");
    }

    #[test]
    fn serial_parse_real_device() {
        let serial = "SDOM:01 CPID:8103 CPRV:11 CPFM:03 SCEP:01 BDID:22 ECID:0019492036C0801E IBFL:3C SRTG:[iBoot-5540.0.0.400.2]";
        let (cpid, bdid, ecid, srtg) = parse_serial_info(serial);
        assert_eq!(cpid, Some(0x8103));
        assert_eq!(bdid, Some(0x22));
        assert_eq!(ecid, Some(0x0019_4920_36C0_801E));
        assert_eq!(srtg, "iBoot-5540.0.0.400.2");
    }

    #[test]
    fn serial_parse_empty() {
        let (cpid, bdid, ecid, _) = parse_serial_info("");
        assert_eq!(cpid, None);
        assert_eq!(bdid, None);
        assert_eq!(ecid, None);
    }
}
