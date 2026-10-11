//! PE Configurator - giris noktasi (Slint masaustu arayuzu).

mod config;
mod database;
mod download;
mod ipsw;
mod irecovery;
mod manager;
mod net;
mod restore;
mod usb;

slint::include_modules!();

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::ipsw::Firmware;
use crate::manager::{Manager, Snapshot};

fn main() -> Result<(), slint::PlatformError> {
    // Veri hattini (algilama + kimlik + firmware) GUI olmadan dogrulamak icin.
    if std::env::args().any(|a| a == "--selftest") {
        selftest();
        return Ok(());
    }

    // IPSW'yi GUI olmadan indirip dogrulamak icin (on indirme).
    if std::env::args().any(|a| a == "--download") {
        cmd_download();
        return Ok(());
    }

    let manager = Manager::new();
    manager.start();

    let ui = AppWindow::new()?;

    // --- Butonlar: onay diyalogu ve islem baslatma ---
    {
        let ui_weak = ui.as_weak();
        ui.on_erase_clicked(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_confirm_visible(true);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_confirm_cancel(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_confirm_visible(false);
            }
        });
    }
    {
        let manager = manager.clone();
        let ui_weak = ui.as_weak();
        ui.on_confirm_accept(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_confirm_visible(false);
            }
            start_restore(&manager, &ui_weak, true);
        });
    }
    {
        let manager = manager.clone();
        let ui_weak = ui.as_weak();
        ui.on_update_clicked(move || {
            start_restore(&manager, &ui_weak, false);
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

    // --- Arayuzu duzenli olarak guncelle ---
    let timer = Timer::default();
    {
        let manager = manager.clone();
        let ui_weak = ui.as_weak();
        let mut last_sig: (String, usize) = (String::new(), usize::MAX);
        let mut last_log_len: usize = usize::MAX;
        timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let ui = match ui_weak.upgrade() {
                Some(ui) => ui,
                None => return,
            };
            let snap = manager.snapshot();
            apply_snapshot(&ui, &snap);

            // Firmware listesi (cihaz tanimlandiginda)
            let identifier = snap.identifier.clone().unwrap_or_default();
            let list = if snap.identified {
                manager.firmware_list()
            } else {
                Vec::new()
            };
            let sig = (identifier, list.len());
            if sig != last_sig {
                let labels: Vec<SharedString> =
                    list.iter().map(|f| SharedString::from(display_firmware(f))).collect();
                ui.set_firmware_model(ModelRc::from(Rc::new(VecModel::from(labels))));
                ui.set_firmware_index(0);
                last_sig = sig;
            }

            // Butonlar yalnizca firmware listesi hazir oldugunda etkin olsun.
            let running = snap.job.status == "running";
            let base = snap.connected
                && snap.is_restorable
                && snap.identified
                && !running
                && snap.tools.ready;
            ui.set_can_restore(base && !list.is_empty());

            // Gunluk (en yeni ustte)
            if snap.job.log.len() != last_log_len {
                last_log_len = snap.job.log.len();
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

fn start_restore(manager: &Arc<Manager>, ui_weak: &slint::Weak<AppWindow>, erase: bool) {
    let ui = match ui_weak.upgrade() {
        Some(ui) => ui,
        None => return,
    };
    let list = manager.firmware_list();
    let index = ui.get_firmware_index().max(0) as usize;
    if let Some(fw) = list.get(index) {
        manager.start_restore(erase, "", &fw.identifier, &fw.version, &fw.buildid, false);
    }
}

fn cmd_download() {
    println!("PE Configurator --download");
    let manager = Manager::new();
    manager.start();

    // Cihaz + firmware hazir olana kadar bekle.
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
        eprintln!("HATA: cihaz tanimlanamadi ya da firmware listesi alinamadi");
        return;
    }

    // Ilerleme yazici.
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
        Ok(path) => println!("INDIRME TAMAM: {}", path.display()),
        Err(err) => println!("INDIRME HATA: {}", err),
    }
}

fn selftest() {
    println!("PE Configurator --selftest");
    let manager = Manager::new();
    manager.start();
    let mut last = String::new();
    for _ in 0..18 {
        std::thread::sleep(Duration::from_millis(1000));
        let snap = manager.snapshot();
        let line = format!(
            "cihaz={} mod={} tanimli={} model={} cpid={:?} bdid={:?} ecid={:?}",
            snap.connected,
            snap.mode_label,
            snap.identified,
            snap.identifier.clone().unwrap_or_default(),
            snap.cpid,
            snap.bdid,
            snap.ecid
        );
        if line != last {
            println!("{}", line);
            last = line;
        }
        if snap.identified {
            let list = manager.firmware_list();
            if !list.is_empty() {
                println!(
                    "firmware: {} adet; en guncel: {} ({}) imzali={} boyut={}",
                    list.len(),
                    list[0].version,
                    list[0].buildid,
                    list[0].signed,
                    list[0].filesize_human
                );
                println!("OK: veri hatti calisiyor (algilama + kimlik + firmware)");
                return;
            }
        }
    }
    println!("BITTI: cihaz bulunamadi ya da firmware alinamadi");
}

fn apply_snapshot(ui: &AppWindow, snap: &Snapshot) {
    ui.set_device_connected(snap.connected);
    ui.set_mode_label(snap.mode_label.clone().into());
    ui.set_mode_color(mode_color(&snap.mode));
    ui.set_tools_ready(snap.tools.ready);
    ui.set_tools_summary(
        format!(
            "idevicerestore: {} · irecovery: {}",
            if snap.tools.idevicerestore { "var" } else { "yok" },
            if snap.tools.irecovery { "var" } else { "yok" },
        )
        .into(),
    );

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
        specs.push(
            format!(
                "CPID/BDID {} / {}",
                hex_opt(snap.cpid),
                hex_opt(snap.bdid)
            )
            .into(),
        );
        specs.push(format!("USB      {}", snap.vid_pid).into());
        ui.set_specs(ModelRc::from(Rc::new(VecModel::from(specs))));
    }

    let running = snap.job.status == "running";
    ui.set_job_running(running);
    ui.set_progress(snap.job.progress as f32);
    ui.set_progress_pct(format!("{}%", (snap.job.progress * 100.0).round()).into());
    ui.set_progress_label(
        if snap.job.message.is_empty() {
            "Hazır".to_string()
        } else {
            snap.job.message.clone()
        }
        .into(),
    );
    ui.set_progress_detail(snap.job.detail.clone().into());
}

fn display_firmware(fw: &Firmware) -> String {
    format!(
        "{} ({}) · {} · {}",
        fw.version,
        fw.buildid,
        if fw.signed { "imzalı" } else { "imzasız" },
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
