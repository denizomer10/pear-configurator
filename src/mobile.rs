//! Normal-mode iPhone/iPad access through libimobiledevice CLI tools.
//! Recovery is a software request; DFU still requires device-specific buttons.
use crate::config;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone)]
pub struct MobileInfo {
    pub identifier: String,
    pub board: String,
    pub ecid: Option<u64>,
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    let env = format!("PEAR_{}", name.to_ascii_uppercase());
    if let Some(path) = std::env::var_os(env)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(path);
    }
    let local = config::bin_dir().join(name);
    if local.is_file() {
        return Some(local);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

/// sysfs omits the hyphen in the newer 24-digit USB serial/UDID format.
pub fn usb_udid(serial: &str) -> Option<String> {
    let compact = serial.replace('-', "");
    if !compact.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match compact.len() {
        24 => Some(format!("{}-{}", &compact[..8], &compact[8..])),
        40 => Some(compact),
        _ => None,
    }
}

pub fn parse_info(text: &str) -> MobileInfo {
    let mut info = MobileInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "ProductType" => info.identifier = value.into(),
            "HardwareModel" => info.board = value.into(),
            "UniqueChipID" => {
                info.ecid = if let Some(hex) = value.strip_prefix("0x") {
                    u64::from_str_radix(hex, 16).ok()
                } else {
                    value.parse().ok()
                }
            }
            _ => {}
        }
    }
    info
}

pub fn query(serial: &str) -> Option<MobileInfo> {
    let udid = usb_udid(serial)?;
    let text = run_tool("ideviceinfo", &["-s", "-u", &udid], Duration::from_secs(5)).ok()?;
    let info = parse_info(&text);
    if info.identifier.starts_with("iPhone")
        || info.identifier.starts_with("iPad")
        || info.identifier.starts_with("iPod")
    {
        Some(info)
    } else {
        None
    }
}

pub fn enter_recovery(serial: &str) -> Result<String, String> {
    let udid =
        usb_udid(serial).ok_or("Cannot determine this device's USB UDID. Reconnect the device.")?;
    run_tool("ideviceenterrecovery", &[&udid], Duration::from_secs(15))
}

fn run_tool(name: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let tool = find_tool(name).ok_or_else(|| {
        format!("{name} is missing. Install the Apple toolchain or libimobiledevice-utils.")
    })?;
    let mut cmd = Command::new(tool);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut paths = vec![config::lib_dir()];
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(paths) = std::env::join_paths(paths) {
        cmd.env("LD_LIBRARY_PATH", paths);
    }
    capture(cmd, timeout)
}

fn capture(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start mobile device tool: {e}"))?;
    let readers: Vec<_> = [
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
    ]
    .into_iter()
    .map(|mut stream| {
        thread::spawn(move || {
            let mut output = String::new();
            let _ = stream.read_to_string(&mut output);
            output
        })
    })
    .collect();
    let started = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(None) => break Err(
                "Device command timed out. Unlock the device, check Trust and usbmuxd, then retry."
                    .to_string(),
            ),
            Err(e) => break Err(format!("Cannot wait for device tool: {e}")),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let output = readers
        .into_iter()
        .map(|r| r.join().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let status = result?;
    if status.success() {
        Ok(output.trim().into())
    } else {
        Err(format!("Device command failed. Unlock the device, accept Trust if requested, and check usbmuxd.\n{}", output.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn udid_formats_and_invalid_serials() {
        assert_eq!(
            usb_udid("0000811000123456789ABCDE").as_deref(),
            Some("00008110-00123456789ABCDE")
        );
        assert_eq!(
            usb_udid("00008110-00123456789ABCDE").as_deref(),
            Some("00008110-00123456789ABCDE")
        );
        assert_eq!(usb_udid(&"a".repeat(40)).unwrap(), "a".repeat(40));
        for invalid in ["", "CPID:8103 ECID:1234", "Apple Keyboard", "../../device"] {
            assert!(usb_udid(invalid).is_none());
        }
    }
    #[test]
    fn normal_mode_chip_id_is_decimal() {
        let info =
            parse_info("ProductType: iPhone15,2\nHardwareModel: D73AP\nUniqueChipID: 123456789\n");
        assert_eq!(info.identifier, "iPhone15,2");
        assert_eq!(info.board, "D73AP");
        assert_eq!(info.ecid, Some(123456789));
        assert_eq!(parse_info("UniqueChipID: 0xabcd").ecid, Some(0xabcd));
    }
    #[test]
    fn captures_failure_diagnostics_and_bounds_silent_commands() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "echo 'lockdown unavailable' >&2; exit 1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        assert!(capture(cmd, Duration::from_secs(1))
            .unwrap_err()
            .contains("lockdown unavailable"));
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "exec sleep 10"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start = Instant::now();
        assert!(capture(cmd, Duration::from_millis(100))
            .unwrap_err()
            .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
