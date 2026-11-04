//! High-level orchestration: device detection and job (download/restore) management.
//!
//! The UI (Slint) only reads this module: it calls [`Manager::snapshot`] to
//! reflect state on screen and starts jobs through the buttons.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use crate::database::{DeviceDatabase, DeviceEntry};
use crate::ipsw::Firmware;
use crate::usb::UsbDevice;
use crate::{config, download, ipsw, irecovery, mobile, restore, usb};

const DETECT_INTERVAL: Duration = Duration::from_millis(1500);
const MAX_LOG_LINES: usize = 500;

#[derive(Default, Clone)]
pub struct JobSnapshot {
    #[allow(dead_code)]
    pub kind: String,
    pub status: String,
    pub progress: f64,
    pub message: String,
    pub detail: String,
    pub log: Vec<String>,
    #[allow(dead_code)]
    pub started_at: u64,
}

#[derive(Default, Clone)]
pub struct ToolsSnapshot {
    pub idevicerestore: bool,
    pub irecovery: bool,
    pub ready: bool,
    #[allow(dead_code)]
    pub download_dir: String,
}

#[derive(Default, Clone)]
pub struct Snapshot {
    pub connected: bool,
    pub can_enter_recovery: bool,
    pub recovery_hint: String,
    pub mode: String,
    pub mode_label: String,
    pub is_restorable: bool,
    pub product: String,
    pub vid_pid: String,
    #[allow(dead_code)]
    pub serial: String,
    pub ecid: Option<u64>,
    pub cpid: Option<u32>,
    pub bdid: Option<u32>,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub boardconfig: String,
    pub identified: bool,
    pub job: JobSnapshot,
    pub tools: ToolsSnapshot,
}

struct Inner {
    device: Option<UsbDevice>,
    irecv: Option<irecovery::IRecoveryInfo>,
    entry: Option<DeviceEntry>,
    mobile: Option<mobile::MobileInfo>,
    job: JobSnapshot,
}

pub struct Manager {
    inner: Mutex<Inner>,
    usb_access: Mutex<()>,
    db: RwLock<Option<DeviceDatabase>>,
    firmware_cache: Mutex<HashMap<String, Vec<Firmware>>>,
    job_cancel: AtomicBool,
    job_running: AtomicBool,
    stop: AtomicBool,
}

impl Manager {
    pub fn new() -> Arc<Self> {
        Arc::new(Manager {
            usb_access: Mutex::new(()),
            inner: Mutex::new(Inner {
                device: None,
                irecv: None,
                entry: None,
                mobile: None,
                job: JobSnapshot::default(),
            }),
            db: RwLock::new(None),
            firmware_cache: Mutex::new(HashMap::new()),
            job_cancel: AtomicBool::new(false),
            job_running: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        })
    }

    pub fn start(self: &Arc<Self>) {
        config::ensure_dirs();

        let me = Arc::clone(self);
        thread::spawn(move || {
            let db = DeviceDatabase::load(false);
            if let Ok(mut guard) = me.db.write() {
                *guard = Some(db);
            }
        });

        let me = Arc::clone(self);
        thread::spawn(move || me.detect_loop());
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.job_cancel.store(true, Ordering::Relaxed);
    }

    // -- detection -------------------------------------------------------
    fn detect_loop(self: Arc<Self>) {
        while !self.stop.load(Ordering::Relaxed) {
            self.poll_device();
            thread::sleep(DETECT_INTERVAL);
        }
    }

    fn poll_device(&self) {
        // libirecovery probes must never claim USB while idevicerestore owns it.
        let Ok(_usb) = self.usb_access.try_lock() else {
            return;
        };
        if self.job_running.load(Ordering::SeqCst) {
            return;
        }

        let devices = usb::scan();
        let device = devices.into_iter().min_by_key(|d| {
            if d.is_restorable() {
                0
            } else if d.mode == "normal" {
                1
            } else {
                2
            }
        });

        let irecv = match &device {
            Some(d) if d.is_restorable() => irecovery::query(d.ecid),
            _ => None,
        };

        let normal_info = device
            .as_ref()
            .filter(|d| d.mode == "normal")
            .and_then(|d| mobile::query(&d.serial));
        let entry = if let Some(info) = &normal_info {
            self.db
                .read()
                .ok()
                .and_then(|db| {
                    db.as_ref()
                        .and_then(|db| db.lookup(&info.identifier).cloned())
                })
                .or_else(|| {
                    Some(DeviceEntry {
                        identifier: info.identifier.clone(),
                        name: info.identifier.clone(),
                        boardconfig: info.board.clone(),
                    })
                })
        } else {
            self.resolve_entry(&device, &irecv)
        };
        if let Some(entry) = &entry {
            self.ensure_firmware(&entry.identifier);
        }

        if let Ok(mut inner) = self.inner.lock() {
            inner.device = device;
            inner.irecv = irecv;
            inner.entry = entry;
            inner.mobile = normal_info;
        }
    }

    fn resolve_entry(
        &self,
        device: &Option<UsbDevice>,
        irecv: &Option<irecovery::IRecoveryInfo>,
    ) -> Option<DeviceEntry> {
        let guard = self.db.read().ok()?;
        let db = guard.as_ref()?;
        let cpid = irecv
            .as_ref()
            .and_then(|i| i.cpid)
            .or_else(|| device.as_ref().and_then(|d| d.cpid));
        let bdid = irecv
            .as_ref()
            .and_then(|i| i.bdid)
            .or_else(|| device.as_ref().and_then(|d| d.bdid));
        let model = irecv
            .as_ref()
            .map(|i| i.model.as_str())
            .filter(|m| !m.is_empty());
        db.resolve(cpid, bdid, model, None)
    }

    fn ensure_firmware(&self, identifier: &str) {
        let already = self
            .firmware_cache
            .lock()
            .map(|c| c.contains_key(identifier))
            .unwrap_or(true);
        if already {
            return;
        }
        let firmwares = ipsw::get_firmwares(identifier, 30);
        if let Ok(mut cache) = self.firmware_cache.lock() {
            cache.insert(identifier.to_string(), firmwares);
        }
    }

    // -- reading ---------------------------------------------------------
    pub fn snapshot(&self) -> Snapshot {
        let (device, irecv, entry, normal_info, job) = {
            let inner = match self.inner.lock() {
                Ok(i) => i,
                Err(_) => return Snapshot::default(),
            };
            (
                inner.device.clone(),
                inner.irecv.clone(),
                inner.entry.clone(),
                inner.mobile.clone(),
                inner.job.clone(),
            )
        };

        let mut snap = Snapshot {
            job,
            tools: self.tools_snapshot(),
            ..Snapshot::default()
        };

        let device = match device {
            Some(d) => d,
            None => return snap,
        };

        snap.connected = true;
        let normal_mobile = device.mode == "normal" && mobile::usb_udid(&device.serial).is_some();
        snap.can_enter_recovery = normal_mobile
            && mobile::find_tool("ideviceenterrecovery").is_some()
            && !self.job_running.load(Ordering::SeqCst);
        snap.recovery_hint = if normal_mobile {
            if mobile::find_tool("ideviceenterrecovery").is_none() {
                "Recovery tool missing. Install libimobiledevice-utils or rebuild the Apple toolchain."
            } else {
                "Unlock the device and accept Trust if asked. Recovery does not erase data. DFU requires device buttons."
            }
        } else { "" }.into();
        snap.mode = device.mode.clone();
        snap.mode_label = device.mode_label().to_string();
        snap.is_restorable = device.is_restorable();
        snap.product = device.product.clone();
        snap.vid_pid = device.vid_pid();
        snap.serial = device.serial.clone();
        snap.ecid = irecv
            .as_ref()
            .and_then(|i| i.ecid)
            .or_else(|| normal_info.as_ref().and_then(|i| i.ecid))
            .or(device.ecid);
        snap.cpid = irecv.as_ref().and_then(|i| i.cpid).or(device.cpid);
        snap.bdid = irecv.as_ref().and_then(|i| i.bdid).or(device.bdid);
        snap.boardconfig = entry
            .as_ref()
            .map(|e| e.boardconfig.clone())
            .unwrap_or_default();
        snap.identified = entry.is_some();
        if let Some(entry) = entry {
            snap.identifier = Some(entry.identifier);
            snap.name = Some(entry.name);
        }
        snap
    }

    pub fn tools_snapshot(&self) -> ToolsSnapshot {
        ToolsSnapshot {
            idevicerestore: restore::is_available(),
            irecovery: irecovery::is_available(),
            ready: restore::is_available(),
            download_dir: config::download_dir().to_string_lossy().to_string(),
        }
    }

    /// Firmware list for the current device (signed and newer versions first).
    pub fn firmware_list(&self) -> Vec<Firmware> {
        let identifier = match self.inner.lock() {
            Ok(inner) => match inner.entry.as_ref() {
                Some(entry) => entry.identifier.clone(),
                None => return Vec::new(),
            },
            Err(_) => return Vec::new(),
        };
        let mut list = self.firmwares_for(&identifier);
        list.sort_by(|a, b| (b.signed, b.version.clone()).cmp(&(a.signed, a.version.clone())));
        list
    }

    fn firmwares_for(&self, identifier: &str) -> Vec<Firmware> {
        if let Ok(cache) = self.firmware_cache.lock() {
            if let Some(existing) = cache.get(identifier) {
                return existing.clone();
            }
        }
        let fetched = ipsw::get_firmwares(identifier, 30);
        if let Ok(mut cache) = self.firmware_cache.lock() {
            cache.insert(identifier.to_string(), fetched.clone());
        }
        fetched
    }

    fn firmware_for(&self, identifier: &str, version: &str, buildid: &str) -> Option<Firmware> {
        self.firmwares_for(identifier).into_iter().find(|f| {
            (version.is_empty() || f.version == version)
                && (buildid.is_empty() || f.buildid == buildid)
        })
    }

    fn ipsw_path_for(&self, fw: &Firmware) -> std::path::PathBuf {
        config::download_dir().join(format!(
            "{}_{}_{}.ipsw",
            fw.identifier, fw.version, fw.buildid
        ))
    }

    // -- jobs ------------------------------------------------------------
    fn begin_job(self: &Arc<Self>, kind: &str) -> bool {
        if self
            .job_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        self.job_cancel.store(false, Ordering::Relaxed);
        if let Ok(mut inner) = self.inner.lock() {
            inner.job = JobSnapshot {
                kind: kind.to_string(),
                status: "running".to_string(),
                started_at: restore::now_epoch(),
                ..JobSnapshot::default()
            };
        }
        true
    }

    pub fn start_restore(
        self: &Arc<Self>,
        erase: bool,
        ipsw_path: &str,
        identifier: &str,
        version: &str,
        buildid: &str,
        ignore_errors: bool,
    ) -> bool {
        if identifier.is_empty() || !self.begin_job("restore") {
            return false;
        }
        let me = Arc::clone(self);
        let (id, ver, bid, path) = (
            identifier.to_string(),
            version.to_string(),
            buildid.to_string(),
            ipsw_path.to_string(),
        );
        thread::spawn(move || me.run_restore(erase, &path, &id, &ver, &bid, ignore_errors));
        true
    }

    /// Request Recovery only for the selected normal-mode mobile device.
    /// Never runs automatically during detection and never starts an erase.
    pub fn start_enter_recovery(self: &Arc<Self>) -> bool {
        if !self.snapshot().can_enter_recovery || !self.begin_job("recovery") {
            return false;
        }
        let me = Arc::clone(self);
        thread::spawn(move || {
            let Ok(_usb) = me.usb_access.lock() else {
                me.finish_job("error", "USB access lock failed", None);
                return;
            };
            let selected = me.inner.lock().ok().and_then(|inner| inner.device.clone());
            let Some(selected) = selected.filter(|d| d.mode == "normal") else {
                me.finish_job(
                    "error",
                    "The selected device is no longer in Normal Mode",
                    None,
                );
                return;
            };
            let expected_ecid = me.snapshot().ecid;
            // Do not reboot a different device if the selection went stale.
            if !usb::scan().iter().any(|d| {
                d.sysfs_path == selected.sysfs_path
                    && d.serial == selected.serial
                    && d.mode == "normal"
            }) {
                me.finish_job("error", "Device disconnected. Reconnect and retry.", None);
                return;
            }
            if me.job_cancel.load(Ordering::Relaxed) {
                me.finish_job("cancelled", "Recovery request cancelled", None);
                return;
            }
            me.update_job(|job| job.message = "Requesting Recovery Mode…".into());
            match mobile::enter_recovery(&selected.serial) {
                Err(error) => {
                    me.log_line(&error);
                    me.finish_job(
                        "error",
                        "Could not enter Recovery Mode. See Activity.",
                        None,
                    );
                    return;
                }
                Ok(output) => {
                    me.log_line(&output);
                }
            }
            me.update_job(|job| job.message = "Waiting for Recovery Mode…".into());
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                if me.job_cancel.load(Ordering::Relaxed) {
                    me.finish_job(
                        "cancelled",
                        "Stopped waiting; the recovery request was already sent",
                        None,
                    );
                    return;
                }
                // Prefer ECID: switching USB speed can change the bus and sysfs path.
                if let Some(device) = usb::scan().into_iter().find(|d| {
                    d.mode == "recovery"
                        && match (expected_ecid, d.ecid) {
                            (Some(expected), Some(actual)) => expected == actual,
                            _ => d.sysfs_path == selected.sysfs_path,
                        }
                }) {
                    let info = irecovery::query(device.ecid);
                    let entry = me.resolve_entry(&Some(device.clone()), &info);
                    if let Ok(mut inner) = me.inner.lock() {
                        inner.device = Some(device);
                        inner.irecv = info;
                        inner.entry = entry;
                        inner.mobile = None;
                    }
                    me.log_line("Recovery Mode detected. Choose firmware to restore when ready.");
                    me.finish_job("done", "Device is in Recovery Mode", Some(1.0));
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            me.finish_job("error", "Recovery requested, but the mode change was not detected. Check the USB connection.", None);
        });
        true
    }

    pub fn cancel_job(&self) {
        self.job_cancel.store(true, Ordering::Relaxed);
    }

    /// Downloads the latest signed firmware for the current device (skips if
    /// already present). Used for headless pre-download and verification.
    pub fn download_latest(self: &Arc<Self>) -> Result<std::path::PathBuf, String> {
        let fw = self
            .firmware_list()
            .into_iter()
            .next()
            .ok_or_else(|| "Firmware not found".to_string())?;
        let dest = self.ipsw_path_for(&fw);
        if dest.exists() && download::verify_sha256(&dest, &fw.sha256) {
            return Ok(dest);
        }
        self.begin_job("download");
        let verified = self.download_firmware(&fw, &dest)?;
        self.finish_job(
            if verified { "done" } else { "error" },
            if verified {
                "Download complete"
            } else {
                "SHA-256 verification failed"
            },
            Some(1.0),
        );
        if verified {
            Ok(dest)
        } else {
            Err("SHA-256 verification failed".to_string())
        }
    }

    pub fn clear_log(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.job.log.clear();
        }
    }

    fn finish_job(&self, status: &str, message: &str, progress: Option<f64>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.job.status = status.to_string();
            inner.job.message = message.to_string();
            if let Some(p) = progress {
                inner.job.progress = p;
            }
        }
        self.job_running.store(false, Ordering::SeqCst);
    }

    fn update_job<F: FnOnce(&mut JobSnapshot)>(&self, f: F) {
        if let Ok(mut inner) = self.inner.lock() {
            f(&mut inner.job);
        }
    }

    fn log_line(&self, line: &str) {
        self.update_job(|job| {
            job.log.push(line.to_string());
            if job.log.len() > MAX_LOG_LINES {
                let excess = job.log.len() - MAX_LOG_LINES;
                job.log.drain(0..excess);
            }
        });
    }

    /// Downloads a firmware; returns `Ok(true)` if the SHA-256 verifies.
    fn download_firmware(
        self: &Arc<Self>,
        fw: &Firmware,
        dest: &std::path::Path,
    ) -> Result<bool, String> {
        let last_milestone = AtomicU64::new(u64::MAX);
        self.update_job(|job| {
            job.message = format!("Downloading {}", fw.version);
            job.detail = fw.filesize_human.clone();
            job.progress = 0.0;
        });
        let total_hint = fw.filesize;
        let progress = |done: u64, total: u64| {
            let denom = if total > 0 { total } else { total_hint };
            let pct = if denom > 0 {
                done as f64 / denom as f64
            } else {
                0.0
            };
            let milestone = (pct * 1000.0) as u64;
            if last_milestone.swap(milestone, Ordering::Relaxed) != milestone {
                self.update_job(|job| {
                    job.progress = pct.min(1.0);
                    job.message = format!("Downloading {}", fw.version);
                    job.detail = format!(
                        "{} / {}",
                        ipsw::human_size(done),
                        if denom > 0 {
                            ipsw::human_size(denom)
                        } else {
                            "?".to_string()
                        }
                    );
                });
            }
        };
        let note = |msg: &str| {
            self.log_line(msg);
        };
        let result = download::download(
            &fw.url,
            dest,
            &fw.sha256,
            true,
            &progress,
            &note,
            &self.job_cancel,
        )?;
        self.log_line(&format!("Downloaded: {}", result.path.display()));
        Ok(result.verified)
    }

    fn run_restore(
        self: &Arc<Self>,
        erase: bool,
        ipsw_path: &str,
        identifier: &str,
        version: &str,
        buildid: &str,
        ignore_errors: bool,
    ) {
        let fw = self.firmware_for(identifier, version, buildid);
        let path = if !ipsw_path.is_empty() && std::path::Path::new(ipsw_path).exists() {
            std::path::PathBuf::from(ipsw_path)
        } else if let Some(fw) = &fw {
            self.ipsw_path_for(fw)
        } else {
            self.finish_job("error", "IPSW could not be determined", None);
            return;
        };

        if !path.exists() {
            let fw = match &fw {
                Some(fw) => fw,
                None => {
                    self.finish_job("error", "No firmware available to download", None);
                    return;
                }
            };
            match self.download_firmware(fw, &path) {
                Ok(true) => {}
                Ok(false) => {
                    self.finish_job("error", "Download could not be verified", None);
                    return;
                }
                Err(e) if e == download::CANCELLED => {
                    self.finish_job("cancelled", "Download cancelled", None);
                    return;
                }
                Err(e) => {
                    self.finish_job("error", &format!("Download error: {}", e), None);
                    return;
                }
            }
        }

        let _usb = match self.usb_access.lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.finish_job("error", "USB access lock failed", None);
                return;
            }
        };
        if self.job_cancel.load(Ordering::Relaxed) {
            self.finish_job("cancelled", "Restore cancelled", None);
            return;
        }
        let ecid = {
            let inner = match self.inner.lock() {
                Ok(i) => i,
                Err(_) => return,
            };
            inner
                .irecv
                .as_ref()
                .and_then(|i| i.ecid)
                .or_else(|| inner.device.as_ref().and_then(|d| d.ecid))
        };

        let options = restore::RestoreOptions {
            ipsw_path: path.to_string_lossy().to_string(),
            erase,
            ecid,
            ignore_errors,
            variant: String::new(),
            cache_path: Some(config::cache_dir()),
            logfile: Some(config::log_dir().join("restore.log")),
        };

        self.update_job(|job| {
            job.message = "Starting restore...".to_string();
            job.progress = 0.0;
        });
        self.log_line(&format!("IPSW: {}", path.display()));
        self.log_line(if erase {
            "Mode: ERASE (wipe)"
        } else {
            "Mode: KEEP DATA (update)"
        });

        let last_milestone = AtomicU64::new(u64::MAX);
        let on_event = |event: restore::RestoreEvent| match event {
            restore::RestoreEvent::Command(cmd) => self.log_line(&format!("$ {}", cmd)),
            restore::RestoreEvent::Progress {
                step_name,
                overall,
                text,
                ..
            } => {
                let milestone = (overall * 1000.0) as u64;
                if last_milestone.swap(milestone, Ordering::Relaxed) != milestone {
                    self.update_job(|job| {
                        job.progress = overall;
                        job.message = step_name;
                        job.detail = text;
                    });
                }
            }
            restore::RestoreEvent::Detail { text, .. } => {
                self.update_job(|job| job.detail = text);
            }
            restore::RestoreEvent::Log(text) => {
                self.update_job(|job| job.detail = text.clone());
                self.log_line(&text);
            }
        };

        match restore::run(&options, &on_event, &self.job_cancel) {
            Ok(0) => self.finish_job("done", "Restore completed successfully", Some(1.0)),
            Ok(code) => self.finish_job(
                "error",
                &format!(
                    "Restore failed (exit code {}). See activity for USB / device errors.",
                    code
                ),
                None,
            ),
            Err(e) if e == restore::CANCELLED => {
                self.finish_job("cancelled", "Restore cancelled", None)
            }
            Err(e) => self.finish_job("error", &format!("Restore error: {}", e), None),
        }
    }
}
