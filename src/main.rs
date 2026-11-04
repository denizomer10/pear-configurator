//! PEAR Configurator - entry point (Slint desktop UI).

mod config;
mod database;
mod download;
mod ipsw;
mod irecovery;
mod manager;
mod mobile;
mod net;
mod queue;
mod restore;
mod udev;
mod usb;

slint::include_modules!();

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::ipsw::Firmware;
use crate::manager::{Manager, Snapshot};

fn main() -> Result<(), slint::PlatformError> {
    if std::env::args().any(|a| a == "--version") {
        println!("PEAR Configurator {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Verify the data pipeline (detection + identity + firmware) without a GUI.
    if std::env::args().any(|a| a == "--selftest") {
        selftest();
        return Ok(());
    }

    // Download and verify the IPSW without a GUI (pre-download).
    if std::env::args().any(|a| a == "--download") {
        cmd_download();
        return Ok(());
    }

    let manager = Manager::new();
    manager.start();

    let ui = AppWindow::new()?;
    let pending: Rc<RefCell<Vec<queue::RestoreRequest>>> = Rc::new(RefCell::new(Vec::new()));

    // --- First-run setup: install the udev rule when it is missing ---
    {
        let ui_weak = ui.as_weak();
        ui.on_setup_install(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_setup_error(false);
                ui.set_setup_done(false);
                ui.set_setup_busy(true);
                ui.set_setup_status("Waiting for administrator permission…".into());
                start_udev_install(&ui);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_setup_quit(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let _ = ui.hide();
            }
            let _ = slint::quit_event_loop();
        });
    }

    if !udev::is_installed() {
        ui.set_setup_visible(true);
        ui.set_setup_message(
            "PEAR needs a one-time USB access rule to talk to Apple devices in DFU or recovery mode. \
             Administrator permission is required to install it."
                .into(),
        );
        ui.set_setup_status("Waiting for administrator permission…".into());
        ui.set_setup_busy(true);
        start_udev_install(&ui);
    }

    // Freeze targets and firmware before opening any confirmation dialog.
    {
        let manager = manager.clone();
        let weak = ui.as_weak();
        let pending = pending.clone();
        ui.on_erase_clicked(move || {
            let Some(ui) = weak.upgrade() else { return; };
            match prepare_request(&manager, &ui, true) {
                Ok(request) if ui.get_queue_mode() => {
                    if let Err(error) = manager.add_to_queue(request) { manager.report_error(&error); }
                    ui.set_queue_visible(true);
                }
                Ok(request) => {
                    ui.set_confirmation_text(format!("The following device will be permanently erased:\n\n{}\n\nThis cannot be undone.",request.summary()).into());
                    *pending.borrow_mut() = vec![request]; ui.set_confirm_visible(true);
                }
                Err(error) => manager.report_error(&error),
            }
        });
    }
    {
        let weak = ui.as_weak();
        let pending = pending.clone();
        ui.on_confirm_cancel(move || {
            pending.borrow_mut().clear();
            if let Some(ui) = weak.upgrade() {
                ui.set_confirm_visible(false);
            }
        });
    }
    {
        let manager = manager.clone();
        let weak = ui.as_weak();
        let pending = pending.clone();
        ui.on_confirm_accept(move || {
            let requests = std::mem::take(&mut *pending.borrow_mut());
            if let Some(ui) = weak.upgrade() {
                ui.set_confirm_visible(false);
                ui.set_queue_visible(false);
            }
            manager.start_requests(requests);
        });
    }
    {
        let manager = manager.clone();
        let weak = ui.as_weak();
        ui.on_update_clicked(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match prepare_request(&manager, &ui, false) {
                Ok(request) if ui.get_queue_mode() => {
                    if let Err(error) = manager.add_to_queue(request) {
                        manager.report_error(&error);
                    }
                    ui.set_queue_visible(true);
                }
                Ok(request) => {
                    manager.start_requests(vec![request]);
                }
                Err(error) => manager.report_error(&error),
            }
        });
    }
    {
        let manager = manager.clone();
        let weak = ui.as_weak();
        ui.on_device_selected(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Some(key) = ui.get_device_keys().row_data(index.max(0) as usize) {
                manager.select_device(key.as_str());
            }
        });
    }
    {
        let manager = manager.clone();
        ui.on_queue_remove(move |key| manager.remove_from_queue(key.as_str()));
    }
    {
        let manager = manager.clone();
        ui.on_queue_clear(move || manager.clear_queue());
    }
    {
        let manager = manager.clone();
        let weak = ui.as_weak();
        let pending = pending.clone();
        ui.on_queue_start(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let requests = manager.pending_requests();
            if requests.is_empty() {
                return;
            }
            let summary = requests
                .iter()
                .map(|r| r.summary())
                .collect::<Vec<_>>()
                .join("\n\n");
            let warning = if requests.iter().any(|r| r.erase) {
                "Entries marked ERASE ALL DATA will be permanently erased. This cannot be undone."
            } else {
                "Update/revive will be attempted. Data preservation is not guaranteed."
            };
            ui.set_confirmation_text(
                format!(
                    "Restore {} queued devices in order:\n\n{}\n\n{}",
                    requests.len(),
                    summary,
                    warning
                )
                .into(),
            );
            *pending.borrow_mut() = requests;
            ui.set_confirm_visible(true);
        });
    }
    {
        let manager = manager.clone();
        ui.on_cancel_clicked(move || manager.cancel_job());
    }
    {
        let manager = manager.clone();
        ui.on_clear_log(move || manager.clear_log());
    }

    {
        let manager = manager.clone();
        ui.on_recovery_clicked(move || {
            manager.start_enter_recovery();
        });
    }

    // --- Refresh the UI periodically ---
    let timer = Timer::default();
    {
        let manager = manager.clone();
        let ui_weak = ui.as_weak();
        let mut last_sig: (String, usize) = (String::new(), usize::MAX);
        let mut last_log: Vec<String> = Vec::new();
        let mut last_devices = Vec::new();
        let mut setup_done_at: Option<std::time::Instant> = None;
        timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let ui = match ui_weak.upgrade() {
                Some(ui) => ui,
                None => return,
            };
            let snap = manager.snapshot();
            apply_snapshot(&ui, &snap);
            if snap.devices != last_devices {
                last_devices = snap.devices.clone();
                let labels: Vec<SharedString> = snap
                    .devices
                    .iter()
                    .map(|(_, label)| label.as_str().into())
                    .collect();
                let keys: Vec<SharedString> = snap
                    .devices
                    .iter()
                    .map(|(key, _)| key.as_str().into())
                    .collect();
                ui.set_device_model(ModelRc::from(Rc::new(VecModel::from(labels))));
                ui.set_device_keys(ModelRc::from(Rc::new(VecModel::from(keys))));
            }
            ui.set_device_index(
                snap.devices
                    .iter()
                    .position(|(key, _)| key == &snap.selected_key)
                    .map(|i| i as i32)
                    .unwrap_or(-1),
            );
            ui.set_selected_device_key(snap.selected_key.clone().into());
            let rows: Vec<QueueRow> = snap
                .queue
                .iter()
                .map(|i| QueueRow {
                    key: i.request.key.clone().into(),
                    label: i.request.summary().into(),
                    status: i.status.clone().into(),
                })
                .collect();
            ui.set_queue_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
            ui.set_queue_count(snap.queue.len() as i32);
            ui.set_queue_pending(snap.queue.iter().any(|i| i.status == "Queued"));

            // Firmware list (once the device is identified)
            let identifier = snap.identifier.clone().unwrap_or_default();
            let list = if snap.identified {
                manager.firmware_list()
            } else {
                Vec::new()
            };
            let sig = (identifier, list.len());
            if sig != last_sig {
                let labels: Vec<SharedString> = list
                    .iter()
                    .map(|f| SharedString::from(display_firmware(f)))
                    .collect();
                ui.set_firmware_model(ModelRc::from(Rc::new(VecModel::from(labels))));
                ui.set_firmware_index(0);
                last_sig = sig;
            }

            // Enable buttons only when the firmware list is ready.
            let running = snap.busy;
            let base = snap.connected
                && snap.is_restorable
                && snap.identified
                && !running
                && snap.tools.ready
                && snap.ecid.is_some_and(|e| e != 0);
            ui.set_can_restore(base && !list.is_empty() && !ui.get_setup_visible());

            // Auto-close the first-run setup window once the rule is installed.
            if ui.get_setup_done() {
                let now = std::time::Instant::now();
                let since = *setup_done_at.get_or_insert(now);
                if now.duration_since(since) >= Duration::from_millis(1200) {
                    ui.set_setup_visible(false);
                    setup_done_at = None;
                }
            } else {
                setup_done_at = None;
            }

            // Log (newest first)
            if snap.job.log != last_log {
                last_log = snap.job.log.clone();
                let lines: Vec<SharedString> = snap
                    .job
                    .log
                    .iter()
                    .rev()
                    .take(300)
                    .map(|l| SharedString::from(l.as_str()))
                    .collect();
                ui.set_log_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
            }
        });
    }

    ui.run()
}

fn start_udev_install(ui: &AppWindow) {
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = udev::install();
        let _ = slint::invoke_from_event_loop(move || {
            let ui = match ui_weak.upgrade() {
                Some(ui) => ui,
                None => return,
            };
            match result {
                Ok(()) => {
                    ui.set_setup_busy(false);
                    ui.set_setup_done(true);
                    ui.set_setup_error(false);
                    ui.set_setup_status("Installed. Ready to use.".into());
                }
                Err(err) => {
                    ui.set_setup_busy(false);
                    ui.set_setup_done(false);
                    ui.set_setup_error(true);
                    ui.set_setup_status(err.into());
                }
            }
        });
    });
}

fn prepare_request(
    manager: &Arc<Manager>,
    ui: &AppWindow,
    erase: bool,
) -> Result<queue::RestoreRequest, String> {
    let list = manager.firmware_list();
    let index = ui.get_firmware_index().max(0) as usize;
    let fw = list.get(index).ok_or("Choose firmware first")?;
    manager.prepare_restore(ui.get_selected_device_key().as_str(), fw, erase)
}

fn cmd_download() {
    println!("PEAR Configurator --download");
    let manager = Manager::new();
    manager.start();

    // Wait until the device is identified and firmwares are available.
    let mut ready = false;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(1000));
        let snap = manager.snapshot();
        if snap.identified && !manager.firmware_list().is_empty() {
            ready = true;
            break;
        }
    }
    if !ready {
        eprintln!("ERROR: device not identified or firmware list unavailable");
        return;
    }

    // Progress printer.
    {
        let manager = manager.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let snap = manager.snapshot();
            if snap.job.status != "running" {
                break;
            }
            println!(
                "{:>3}%  {}  {}",
                (snap.job.progress * 100.0).round(),
                snap.job.message,
                snap.job.detail
            );
        });
    }

    match manager.download_latest() {
        Ok(path) => println!("DOWNLOAD COMPLETE: {}", path.display()),
        Err(err) => println!("DOWNLOAD ERROR: {}", err),
    }
}

fn selftest() {
    println!("PEAR Configurator --selftest");
    let manager = Manager::new();
    manager.start();
    let mut last = String::new();
    for _ in 0..18 {
        std::thread::sleep(Duration::from_millis(1000));
        let snap = manager.snapshot();
        let line = format!(
            "device={} mode={} identified={} model={} cpid={:?} bdid={:?} ecid={:?}",
            snap.connected,
            snap.mode_label,
            snap.identified,
            snap.identifier.clone().unwrap_or_default(),
            snap.cpid,
            snap.bdid,
            snap.ecid
        );
        if line != last {
            println!("Detected devices: {}", snap.devices.len());
            for (_, label) in &snap.devices {
                println!("  {label}");
            }
            println!("{}", line);
            last = line;
        }
        if snap.identified {
            let list = manager.firmware_list();
            if !list.is_empty() {
                println!(
                    "firmware: {} entries; latest: {} ({}) signed={} size={}",
                    list.len(),
                    list[0].version,
                    list[0].buildid,
                    list[0].signed,
                    list[0].filesize_human
                );
                println!("OK: data pipeline works (detection + identity + firmware)");
                return;
            }
        }
    }
    println!("DONE: no device found or firmware list unavailable");
}

fn apply_snapshot(ui: &AppWindow, snap: &Snapshot) {
    ui.set_device_connected(snap.connected);
    ui.set_normal_mobile(snap.mode == "normal" && mobile::usb_udid(&snap.serial).is_some());
    ui.set_can_enter_recovery(snap.can_enter_recovery);
    ui.set_recovery_hint(snap.recovery_hint.clone().into());
    ui.set_mode_label(snap.mode_label.clone().into());
    ui.set_mode_color(mode_color(&snap.mode));
    ui.set_tools_ready(snap.tools.ready);
    ui.set_tools_summary(
        format!(
            "idevicerestore: {} · irecovery: {}",
            if snap.tools.idevicerestore {
                "yes"
            } else {
                "no"
            },
            if snap.tools.irecovery { "yes" } else { "no" },
        )
        .into(),
    );

    if !snap.connected {
        ui.set_device_name("Selected device disconnected".into());
        ui.set_specs(ModelRc::from(Rc::new(VecModel::<SharedString>::default())));
    }
    if snap.connected {
        ui.set_device_name(
            snap.name
                .clone()
                .unwrap_or_else(|| snap.product.clone())
                .into(),
        );

        let mut specs: Vec<SharedString> = Vec::new();
        specs.push(format!("Model    {}", snap.identifier.as_deref().unwrap_or("—")).into());
        specs.push(format!("Board    {}", non_empty(&snap.boardconfig)).into());
        specs.push(
            format!(
                "ECID     {}",
                snap.ecid
                    .map(|v| format!("0x{:x}", v))
                    .unwrap_or_else(|| "—".to_string())
            )
            .into(),
        );
        if snap.cpid.is_some() || snap.bdid.is_some() {
            specs.push(format!("CPID/BDID {} / {}", hex_opt(snap.cpid), hex_opt(snap.bdid)).into());
        }
        specs.push(format!("USB      {}", snap.vid_pid).into());
        ui.set_specs(ModelRc::from(Rc::new(VecModel::from(specs))));
    }

    ui.set_job_running(snap.busy);
    ui.set_progress(snap.job.progress as f32);
    ui.set_progress_pct(format!("{}%", (snap.job.progress * 100.0).round()).into());
    ui.set_progress_label(
        if snap.job.message.is_empty() {
            "Ready".to_string()
        } else {
            snap.job.message.clone()
        }
        .into(),
    );
    ui.set_active_target(snap.job.target.clone().into());
    ui.set_progress_detail(snap.job.detail.clone().into());
}

fn display_firmware(fw: &Firmware) -> String {
    format!(
        "{} ({}) · {} · {}",
        fw.version,
        fw.buildid,
        if fw.signed { "signed" } else { "unsigned" },
        fw.filesize_human
    )
}

fn mode_color(mode: &str) -> slint::Color {
    match mode {
        "dfu" => slint::Color::from_rgb_u8(255, 198, 77),
        "recovery" => slint::Color::from_rgb_u8(53, 212, 154),
        "restore" => slint::Color::from_rgb_u8(155, 109, 255),
        _ => slint::Color::from_rgb_u8(109, 139, 255),
    }
}

fn hex_opt(value: Option<u32>) -> String {
    value
        .map(|v| format!("0x{:x}", v))
        .unwrap_or_else(|| "—".to_string())
}

fn non_empty(value: &str) -> String {
    if value.is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}
