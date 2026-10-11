//! Cihaz (board) veritabani.
//!
//! Apple cihazlari DFU modunda kendini CPID (yonga) + BDID (board) ile tanitir.
//! Bu ikiliyi urun tanimlayicisina (ornegin `Macmini9,1`) cevirmek icin
//! ipsw.me `/v4/devices` uclu kullanilir ve yerel olarak onbelleklenir.

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::net;

#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub identifier: String,
    pub name: String,
    pub boardconfig: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct RawDevice {
    identifier: String,
    name: String,
    #[serde(default)]
    boardconfig: String,
    #[serde(default)]
    boards: Vec<RawBoard>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct RawBoard {
    #[serde(default)]
    boardconfig: String,
    #[serde(default)]
    cpid: Option<u32>,
    #[serde(default)]
    bdid: Option<u32>,
}

pub struct DeviceDatabase {
    by_board: HashMap<(u32, u32), DeviceEntry>,
    by_id: HashMap<String, DeviceEntry>,
}

impl DeviceDatabase {
    pub fn from_raw(raw: &[RawDevice]) -> Self {
        let mut by_board = HashMap::new();
        let mut by_id = HashMap::new();

        for dev in raw {
            let base = DeviceEntry {
                identifier: dev.identifier.clone(),
                name: dev.name.clone(),
                boardconfig: dev.boardconfig.clone(),
            };
            by_id.entry(dev.identifier.clone()).or_insert_with(|| base.clone());

            let boards: Vec<&RawBoard> = if dev.boards.is_empty() {
                // boards alani yoksa kok seviye bilgisi kullanilamaz; atla.
                Vec::new()
            } else {
                dev.boards.iter().collect()
            };

            for board in boards {
                if let (Some(cpid), Some(bdid)) = (board.cpid, board.bdid) {
                    let entry = DeviceEntry {
                        identifier: dev.identifier.clone(),
                        name: dev.name.clone(),
                        boardconfig: if board.boardconfig.is_empty() {
                            dev.boardconfig.clone()
                        } else {
                            board.boardconfig.clone()
                        },
                    };
                    by_board.entry((cpid, bdid)).or_insert(entry);
                }
            }
        }

        DeviceDatabase { by_board, by_id }
    }

    pub fn identify(&self, cpid: Option<u32>, bdid: Option<u32>) -> Option<&DeviceEntry> {
        match (cpid, bdid) {
            (Some(cpid), Some(bdid)) => self.by_board.get(&(cpid, bdid)),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn lookup(&self, identifier: &str) -> Option<&DeviceEntry> {
        self.by_id.get(identifier)
    }

    pub fn resolve(
        &self,
        cpid: Option<u32>,
        bdid: Option<u32>,
        model: Option<&str>,
        identifier: Option<&str>,
    ) -> Option<DeviceEntry> {
        if let Some(id) = identifier {
            if let Some(entry) = self.by_id.get(id) {
                return Some(entry.clone());
            }
        }
        if let Some(entry) = self.identify(cpid, bdid) {
            return Some(entry.clone());
        }
        if let Some(model) = model {
            let needle = model.to_lowercase();
            for entry in self.by_board.values() {
                if entry.boardconfig.to_lowercase() == needle {
                    return Some(entry.clone());
                }
            }
        }
        None
    }

    #[allow(dead_code)]
    pub fn all_devices(&self) -> Vec<DeviceEntry> {
        let mut devices: Vec<DeviceEntry> = self.by_id.values().cloned().collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        devices
    }

    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.by_board.len()
    }

    /// Veritabanini onbellekten ya da ipsw.me'den yukler.
    pub fn load(refresh: bool) -> Self {
        let cache_path = config::device_db_cache();
        let mut raw: Option<Vec<RawDevice>> = None;

        if !refresh && cache_fresh(&cache_path) {
            raw = read_cache(&cache_path);
        }

        if raw.is_none() {
            if let Some(fetched) = fetch_remote() {
                write_cache(&cache_path, &fetched);
                raw = Some(fetched);
            } else {
                raw = read_cache(&cache_path);
            }
        }

        DeviceDatabase::from_raw(&raw.unwrap_or_default())
    }
}

fn cache_fresh(path: &std::path::Path) -> bool {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let modified = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age < Duration::from_secs(config::DEVICE_DB_MAX_AGE),
        Err(_) => true,
    }
}

fn read_cache(path: &std::path::Path) -> Option<Vec<RawDevice>> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(path: &std::path::Path, devices: &[RawDevice]) {
    if let Ok(text) = serde_json::to_string(devices) {
        let _ = fs::write(path, text);
    }
}

fn fetch_remote() -> Option<Vec<RawDevice>> {
    let url = format!("{}/devices", config::IPSW_API_BASE);
    let body = net::get_string(&url, 30).ok()?;
    serde_json::from_str(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<RawDevice> {
        serde_json::from_str(
            r#"[
              {"identifier":"Macmini9,1","name":"Mac mini (M1, Late 2020)","boardconfig":"J274AP",
               "boards":[{"boardconfig":"J274AP","platform":"t8103","cpid":33027,"bdid":34}]},
              {"identifier":"iPhone14,2","name":"iPhone 13 Pro","boardconfig":"D63AP",
               "boards":[{"boardconfig":"D63AP","platform":"t8110","cpid":33040,"bdid":12}]}
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn identify_by_board() {
        let db = DeviceDatabase::from_raw(&fixture());
        let entry = db.identify(Some(33027), Some(34)).unwrap();
        assert_eq!(entry.identifier, "Macmini9,1");
    }

    #[test]
    fn identify_missing() {
        let db = DeviceDatabase::from_raw(&fixture());
        assert!(db.identify(Some(1), Some(2)).is_none());
    }

    #[test]
    fn resolve_priority_cpid_over_model() {
        let db = DeviceDatabase::from_raw(&fixture());
        let entry = db
            .resolve(Some(33040), Some(12), Some("J274AP"), None)
            .unwrap();
        assert_eq!(entry.identifier, "iPhone14,2");
    }

    #[test]
    fn resolve_by_model() {
        let db = DeviceDatabase::from_raw(&fixture());
        let entry = db.resolve(None, None, Some("j274ap"), None).unwrap();
        assert_eq!(entry.identifier, "Macmini9,1");
    }
}
