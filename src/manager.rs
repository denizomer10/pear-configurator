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
use crate::{config, download, ipsw, irecovery, mobile, queue, restore, usb};

const DETECT_INTERVAL: Duration = Duration::from_millis(1500);
const MAX_LOG_LINES: usize = 500;

#[derive(Default, Clone)]
pub struct JobSnapshot {
    #[allow(dead_code)]
    pub kind: String,
    pub target: String,
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
    pub devices: Vec<(String, String)>,
    pub selected_key: String,
    pub busy: bool,
    pub queue: Vec<queue::QueueItem>,
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

#[derive(Clone)]
struct DetectedDevice {
    key: String,
    device: UsbDevice,
    irecv: Option<irecovery::IRecoveryInfo>,
    entry: Option<DeviceEntry>,
    mobile: Option<mobile::MobileInfo>,
}
impl DetectedDevice {
    fn ecid(&self) -> Option<u64> {
        self.irecv
            .as_ref()
            .and_then(|i| i.ecid)
            .or_else(|| self.mobile.as_ref().and_then(|i| i.ecid))
            .or(self.device.ecid)
    }
    fn label(&self) -> String {
        format!(
            "{} · {} · {}",
            self.entry
                .as_ref()
                .map(|e| e.name.as_str())
                .unwrap_or(&self.device.product),
            self.device.mode_label(),
            self.ecid()
                .map(|e| format!("ECID {e:x}"))
                .unwrap_or_else(|| self.device.sysfs_path.clone())
        )
    }
}

struct Inner {
    devices: Vec<DetectedDevice>,
    selected_key: String,
    queue: Vec<queue::QueueItem>,
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
    batch_running: AtomicBool,
    stop: AtomicBool,
}

impl Manager {
    pub fn new() -> Arc<Self> {
        Arc::new(Manager {
            usb_access: Mutex::new(()),
            inner: Mutex::new(Inner {
                devices: Vec::new(),
                selected_key: String::new(),
                queue: Vec::new(),
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
            batch_running: AtomicBool::new(false),
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

        let mut usb_devices = usb::scan();
        usb_devices.sort_by_key(|d| {
            if d.is_restorable() {
                0
            } else if d.mode == "normal" {
                1
            } else {
                2
            }
        });
        let recovery_count = usb_devices.iter().filter(|d| d.is_restorable()).count();
        let mut devices = Vec::new();
        for device in usb_devices {
            // Never issue an unaddressed query when more than one restore device exists.
            let irecv = if device.is_restorable() && (device.ecid.is_some() || recovery_count == 1)
            {
                irecovery::query(device.ecid)
            } else {
                None
            };
            let normal_info = if device.mode == "normal" {
                mobile::query(&device.serial)
            } else {
                None
            };
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
                self.resolve_entry(&Some(device.clone()), &irecv)
            };
            let ecid = irecv
                .as_ref()
                .and_then(|i| i.ecid)
                .or_else(|| normal_info.as_ref().and_then(|i| i.ecid))
                .or(device.ecid);
            let key = ecid.map(|e| format!("ecid:{e:x}")).unwrap_or_else(|| {
                if device.serial.is_empty() {
                    format!("port:{}", device.sysfs_path)
                } else {
                    format!("serial:{}", device.serial)
                }
            });
            devices.push(DetectedDevice {
                key,
                device,
                irecv,
                entry,
                mobile: normal_info,
            });
        }
        let identifiers: Vec<_> = devices
            .iter()
            .filter_map(|d| d.entry.as_ref().map(|e| e.identifier.clone()))
            .collect();
        if let Ok(mut inner) = self.inner.lock() {
            Self::replace_devices(&mut inner, devices);
        }
        drop(_usb);
        // Publish detection before doing network requests.
        for identifier in identifiers {
            self.ensure_firmware(&identifier);
        }
    }

    fn replace_devices(inner: &mut Inner, devices: Vec<DetectedDevice>) {
        if inner.selected_key.is_empty() {
            if let Some(first) = devices.first() {
                inner.selected_key = first.key.clone();
            }
        } else if !devices.iter().any(|d| d.key == inner.selected_key) {
            // A successful later query may enrich a serial identity with its ECID.
            // Follow only the same non-empty serial, never a replacement on the same port.
            let serial = inner
                .device
                .as_ref()
                .map(|d| d.serial.as_str())
                .or_else(|| inner.selected_key.strip_prefix("serial:"));
            if let Some(serial) = serial.filter(|s| !s.is_empty()) {
                if let Some(same) = devices.iter().find(|d| d.device.serial == serial) {
                    inner.selected_key = same.key.clone();
                }
            }
        }
        inner.devices = devices;
        Self::apply_selection(inner);
    }

    fn apply_selection(inner: &mut Inner) {
        let selected = inner
            .devices
            .iter()
            .find(|d| d.key == inner.selected_key)
            .cloned();
        inner.device = selected.as_ref().map(|d| d.device.clone());
        inner.irecv = selected.as_ref().and_then(|d| d.irecv.clone());
        inner.entry = selected.as_ref().and_then(|d| d.entry.clone());
        inner.mobile = selected.and_then(|d| d.mobile);
    }

    pub fn select_device(&self, key: &str) -> bool {
        if self.job_running.load(Ordering::SeqCst) {
            return false;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if !inner.devices.iter().any(|d| d.key == key) {
            return false;
        }
        inner.selected_key = key.into();
        Self::apply_selection(&mut inner);
        true
    }

    fn resolve_entry(
        &self,
        device: &Option<UsbDevice>,
        irecv: &Option<irecovery::IRecoveryInfo>,
    ) -> Option<DeviceEntry> {
        let fallback = irecv
            .as_ref()
            .filter(|i| i.identifier.contains(','))
            .map(|i| DeviceEntry {
                identifier: i.identifier.clone(),
                name: i.identifier.clone(),
                boardconfig: i.model.clone(),
            });
        let guard = self.db.read().ok()?;
        let Some(db) = guard.as_ref() else {
            return fallback;
        };
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
        let identifier = irecv
            .as_ref()
            .map(|i| i.identifier.as_str())
            .filter(|id| !id.is_empty());
        db.resolve(cpid, bdid, model, identifier).or(fallback)
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
        let (device, irecv, entry, normal_info, job, devices, selected_key, queued) = {
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
                inner
                    .devices
                    .iter()
                    .map(|d| (d.key.clone(), d.label()))
                    .collect(),
                inner.selected_key.clone(),
                inner.queue.clone(),
            )
        };

        let mut snap = Snapshot {
            devices,
            selected_key,
            queue: queued,
            busy: self.job_running.load(Ordering::SeqCst),
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
        let mut list = self
            .firmware_cache
            .lock()
            .ok()
            .and_then(|c| c.get(&identifier).cloned())
            .unwrap_or_default();
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

    pub fn prepare_restore(
        &self,
        key: &str,
        fw: &Firmware,
        erase: bool,
    ) -> Result<queue::RestoreRequest, String> {
        let inner = self.inner.lock().map_err(|_| "Device state unavailable")?;
        let device = inner
            .devices
            .iter()
            .find(|d| d.key == key)
            .ok_or("Selected device disconnected")?;
        if !device.device.is_restorable() {
            return Err("Put this device into DFU or Recovery Mode first".into());
        }
        let ecid = device
            .ecid()
            .filter(|e| *e != 0)
            .ok_or("Device ECID unavailable; restore cannot be safely targeted")?;
        if device.entry.as_ref().map(|e| e.identifier.as_str()) != Some(fw.identifier.as_str()) {
            return Err("Firmware does not match the selected device".into());
        }
        Ok(queue::RestoreRequest {
            key: device.key.clone(),
            name: device.entry.as_ref().unwrap().name.clone(),
            ecid,
            identifier: fw.identifier.clone(),
            version: fw.version.clone(),
            buildid: fw.buildid.clone(),
            erase,
        })
    }

    pub fn add_to_queue(&self, request: queue::RestoreRequest) -> Result<(), String> {
        if self.job_running.load(Ordering::SeqCst) {
            return Err("Wait for the current operation".into());
        }
        let mut inner = self.inner.lock().map_err(|_| "Queue unavailable")?;
        if inner
            .queue
            .iter()
            .any(|item| item.request.ecid == request.ecid)
        {
            return Err(
                "This device is already queued. Remove its entry to change firmware or mode."
                    .into(),
            );
        }
        inner.queue.push(queue::QueueItem {
            request,
            status: "Queued".into(),
        });
        Ok(())
    }

    pub fn remove_from_queue(&self, key: &str) {
        if self.job_running.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.queue.retain(|i| i.request.key != key);
        }
    }
    pub fn clear_queue(&self) {
        if self.job_running.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.queue.clear();
        }
    }
    pub fn pending_requests(&self) -> Vec<queue::RestoreRequest> {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .queue
                    .iter()
                    .filter(|i| i.status == "Queued")
                    .map(|i| i.request.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn report_error(&self, error: &str) {
        self.log_line(error);
    }

    pub fn start_requests(self: &Arc<Self>, requests: Vec<queue::RestoreRequest>) -> bool {
        let unique: std::collections::HashSet<_> = requests.iter().map(|r| r.ecid).collect();
        if requests.is_empty()
            || unique.len() != requests.len()
            || requests.iter().any(|r| r.ecid == 0)
            || !self.begin_job("restore")
        {
            return false;
        }
        self.batch_running.store(true, Ordering::SeqCst);
        let me = Arc::clone(self);
        thread::spawn(move || {
            let mut failed = 0;
            queue::execute(
                &requests,
                &me.job_cancel,
                |request| {
                    me.update_job(|job| {
                        job.status = "running".into();
                        job.target = format!("{} · ECID {:x}", request.name, request.ecid);
                        job.progress = 0.0;
                        job.message =
                            format!("Restoring {} · ECID {:x}", request.name, request.ecid);
                    });
                    me.log_line(&format!("Target: {}", request.summary()));
                    me.run_restore(
                        request.erase,
                        "",
                        &request.identifier,
                        &request.version,
                        &request.buildid,
                        false,
                        request.ecid,
                    );
                    let job = me.inner.lock().unwrap().job.clone();
                    if job.status == "done" {
                        Ok(())
                    } else {
                        failed += 1;
                        Err(job.message)
                    }
                },
                |request, status| {
                    if let Ok(mut inner) = me.inner.lock() {
                        if let Some(item) = inner.queue.iter_mut().find(|i| {
                            i.request.key == request.key
                                && i.request.version == request.version
                                && i.request.buildid == request.buildid
                                && i.request.erase == request.erase
                        }) {
                            item.status = status.into();
                        }
                    }
                    me.log_line(&format!("ECID {:x}: {}", request.ecid, status));
                },
            );
            me.batch_running.store(false, Ordering::SeqCst);
            if me.job_cancel.load(Ordering::Relaxed) {
                me.finish_job(
                    "cancelled",
                    "Queue cancelled; remaining devices were skipped",
                    None,
                );
            } else if failed > 0 {
                me.finish_job(
                    "error",
                    &format!("Queue finished: {failed} device(s) failed. See queue results."),
                    None,
                );
            } else {
                me.finish_job(
                    "done",
                    &format!("Restore completed for {} device(s)", requests.len()),
                    Some(1.0),
                );
            }
        });
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
        if !self.batch_running.load(Ordering::SeqCst) {
            self.job_running.store(false, Ordering::SeqCst);
        }
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
        ecid: u64,
    ) {
        if !usb::scan()
            .iter()
            .any(|d| d.is_restorable() && d.ecid == Some(ecid))
        {
            self.finish_job(
                "error",
                "Target device disconnected or no longer in DFU/Recovery Mode",
                None,
            );
            return;
        }
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
        // Revalidate after downloads. Never fall back to the first connected device.
        if !usb::scan()
            .iter()
            .any(|d| d.is_restorable() && d.ecid == Some(ecid))
        {
            self.finish_job("error", "Target device disconnected before restore", None);
            return;
        }

        let options = restore::RestoreOptions {
            ipsw_path: path.to_string_lossy().to_string(),
            erase,
            ecid: Some(ecid),
            ignore_errors,
            variant: String::new(),
            cache_path: Some(config::cache_dir()),
            logfile: Some(config::log_dir().join(format!("restore-{ecid:x}.log"))),
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

#[cfg(test)]
mod tests {
    use super::*;
    fn detected(ecid: u64, mode: &str, id: &str) -> DetectedDevice {
        DetectedDevice {
            key: format!("ecid:{ecid:x}"),
            device: UsbDevice {
                vendor_id: 0x05ac,
                product_id: 0x1227,
                product: "Test device".into(),
                manufacturer: "Apple".into(),
                serial: format!("ECID:{ecid:x}"),
                busnum: Some(1),
                devnum: Some(1),
                sysfs_path: format!("/test/{ecid}"),
                mode: mode.into(),
                cpid: None,
                bdid: None,
                ecid: Some(ecid),
                srtg: String::new(),
            },
            irecv: None,
            mobile: None,
            entry: Some(DeviceEntry {
                identifier: id.into(),
                name: id.into(),
                boardconfig: "Test".into(),
            }),
        }
    }
    fn firmware(id: &str) -> Firmware {
        Firmware {
            identifier: id.into(),
            version: "test".into(),
            buildid: "test".into(),
            url: String::new(),
            sha256: String::new(),
            sha1: String::new(),
            signed: true,
            releasedate: String::new(),
            filesize: 1,
            filesize_human: "1B".into(),
        }
    }
    #[test]
    fn explicit_selection_and_disconnect_never_select_another_device() {
        let me = Manager::new();
        {
            let mut inner = me.inner.lock().unwrap();
            inner.devices = vec![
                detected(11, "dfu", "iPhone15,2"),
                detected(22, "recovery", "iPad13,1"),
            ];
        }
        assert!(me.select_device("ecid:b"));
        assert_eq!(me.snapshot().ecid, Some(11));
        {
            let mut inner = me.inner.lock().unwrap();
            inner.devices.retain(|d| d.ecid() != Some(11));
            Manager::apply_selection(&mut inner);
        }
        let snapshot = me.snapshot();
        assert!(!snapshot.connected);
        assert_eq!(snapshot.selected_key, "ecid:b");
        assert_eq!(snapshot.devices.len(), 1);
        assert!(me.select_device("ecid:16"));
        assert_eq!(me.snapshot().ecid, Some(22));
    }
    #[test]
    fn follows_identity_enrichment_but_not_same_port_replacements() {
        let me = Manager::new();
        let mut unqueried = detected(11, "normal", "iPhone15,2");
        unqueried.key = "serial:phone-A".into();
        unqueried.device.serial = "phone-A".into();
        let mut queried = unqueried.clone();
        queried.key = "ecid:b".into();
        {
            let mut inner = me.inner.lock().unwrap();
            Manager::replace_devices(&mut inner, vec![unqueried]);
            Manager::replace_devices(&mut inner, vec![queried]);
        }
        assert_eq!(me.snapshot().selected_key, "ecid:b");
        let mut replacement = detected(22, "normal", "iPhone15,2");
        replacement.device.sysfs_path = "/test/11".into();
        {
            let mut inner = me.inner.lock().unwrap();
            Manager::replace_devices(&mut inner, vec![replacement]);
        }
        assert!(!me.snapshot().connected);
        assert_eq!(me.snapshot().selected_key, "ecid:b");
    }

    #[test]
    fn recovery_product_identity_works_before_database_loads() {
        let me = Manager::new();
        let info = irecovery::parse_query_output("PRODUCT: iPhone17,5\nMODEL: v59ap\n");
        let entry = me.resolve_entry(&None, &Some(info)).unwrap();
        assert_eq!(entry.identifier, "iPhone17,5");
        assert_eq!(entry.boardconfig, "v59ap");
    }

    #[test]
    fn plan_rejects_wrong_firmware_normal_mode_and_missing_ecid() {
        let me = Manager::new();
        {
            let mut inner = me.inner.lock().unwrap();
            inner.devices = vec![
                detected(11, "dfu", "iPhone15,2"),
                detected(22, "normal", "iPad13,1"),
                detected(0, "dfu", "iPhone15,2"),
            ];
        }
        assert!(me
            .prepare_restore("ecid:b", &firmware("iPad13,1"), true)
            .is_err());
        assert!(me
            .prepare_restore("ecid:16", &firmware("iPad13,1"), true)
            .is_err());
        assert!(me
            .prepare_restore("ecid:0", &firmware("iPhone15,2"), true)
            .is_err());
        let request = me
            .prepare_restore("ecid:b", &firmware("iPhone15,2"), true)
            .unwrap();
        assert_eq!(request.ecid, 11);
        me.add_to_queue(request.clone()).unwrap();
        assert!(me.add_to_queue(request.clone()).is_err());
        // An already confirmed plan remains bound to its original device.
        assert!(me.select_device("ecid:16"));
        assert_eq!(request.ecid, 11);
        assert_eq!(me.pending_requests()[0].ecid, 11);
        me.remove_from_queue("ecid:b");
        assert!(me.pending_requests().is_empty());
    }
    #[test]
    fn busy_state_survives_each_batch_item_and_blocks_queue_edits() {
        let me = Manager::new();
        let request = queue::RestoreRequest {
            key: "ecid:b".into(),
            name: "Test".into(),
            ecid: 11,
            identifier: "iPhone15,2".into(),
            version: "test".into(),
            buildid: "test".into(),
            erase: true,
        };
        me.add_to_queue(request.clone()).unwrap();
        assert!(me.begin_job("restore"));
        me.batch_running.store(true, Ordering::SeqCst);
        me.finish_job("done", "First complete", None);
        assert!(me.snapshot().busy);
        assert!(!me.select_device("ecid:b"));
        assert!(me.add_to_queue(request).is_err());
        me.clear_queue();
        assert_eq!(me.pending_requests().len(), 1);
        me.batch_running.store(false, Ordering::SeqCst);
        me.finish_job("done", "Queue complete", None);
        assert!(!me.snapshot().busy);
    }
}
