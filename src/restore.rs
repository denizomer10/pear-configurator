//! Restore engine: runs the `idevicerestore` process and parses its progress.
//!
//! With `idevicerestore -P` (plain progress) the tool emits machine-readable
//! lines:
//!
//! ```text
//! progress: <step> <step_progress>
//! <Label>: <percent>
//! ```

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config;

pub const RESTORE_STEPS: [&str; 14] = [
    "Detecting device",
    "Preparing",
    "Verifying firmware",
    "Uploading iBSS",
    "Uploading iBEC",
    "Sending ramdisk",
    "Sending kernelcache",
    "Sending device tree",
    "Sending SEP",
    "Updating baseband",
    "Verifying OS",
    "Writing filesystem",
    "Writing recovery files",
    "Writing to flash",
];
pub const TOTAL_STEPS: usize = RESTORE_STEPS.len();

pub const CANCELLED: &str = "__cancelled__";

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum RestoreEvent {
    Command(String),
    Progress {
        step: usize,
        step_name: String,
        step_progress: f64,
        overall: f64,
        text: String,
    },
    Detail {
        label: String,
        percent: f64,
        text: String,
    },
    Log(String),
}

pub struct RestoreOptions {
    pub ipsw_path: String,
    pub erase: bool,
    pub ecid: Option<u64>,
    pub ignore_errors: bool,
    pub variant: String,
    pub cache_path: Option<PathBuf>,
    pub logfile: Option<PathBuf>,
}

pub fn find_idevicerestore() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("PEAR_IDEVICERESTORE") {
        let path = PathBuf::from(env);
        if path.exists() {
            return Some(path);
        }
    }
    let local = config::bin_dir().join("idevicerestore");
    if local.exists() {
        return Some(local);
    }
    which("idevicerestore")
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

pub fn is_available() -> bool {
    find_idevicerestore().is_some()
}

/// Builds the argv list for `idevicerestore`.
pub fn build_command(tool: &Path, options: &RestoreOptions) -> Vec<String> {
    let mut cmd = vec![
        tool.to_string_lossy().to_string(),
        "-P".to_string(),
        "-y".to_string(),
    ];
    let cache = options.cache_path.clone().unwrap_or_else(config::cache_dir);
    cmd.push("-C".to_string());
    cmd.push(cache.to_string_lossy().to_string());

    let logfile = options
        .logfile
        .clone()
        .unwrap_or_else(|| config::log_dir().join("restore.log"));
    cmd.push("--logfile".to_string());
    cmd.push(logfile.to_string_lossy().to_string());

    if options.erase {
        cmd.push("-e".to_string());
    }
    if let Some(ecid) = options.ecid {
        cmd.push("-i".to_string());
        cmd.push(format!("0x{:x}", ecid));
    }
    if options.ignore_errors {
        cmd.push("--ignore-errors".to_string());
    }
    if !options.variant.is_empty() {
        cmd.push("--variant".to_string());
        cmd.push(options.variant.clone());
    }
    cmd.push(options.ipsw_path.clone());
    cmd
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Turns a single output line into an event (or `None`).
pub fn parse_progress_line(line: &str) -> Option<RestoreEvent> {
    let clean = strip_ansi(line).trim_end().to_string();
    if clean.is_empty() {
        return None;
    }

    if let Some(rest) = clean.strip_prefix("progress:") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 2 {
            if let (Ok(step), Ok(frac)) = (parts[0].parse::<usize>(), parts[1].parse::<f64>()) {
                let step = step.min(TOTAL_STEPS - 1);
                let frac_clamped = frac.clamp(0.0, 1.0);
                let overall = (step as f64 + frac_clamped) / TOTAL_STEPS as f64;
                return Some(RestoreEvent::Progress {
                    step,
                    step_name: RESTORE_STEPS[step].to_string(),
                    step_progress: frac,
                    overall,
                    text: clean,
                });
            }
        }
    }

    // "<Label>: <percent>" form (e.g. "Sending KernelCache:  42.5")
    if let Some((label, value)) = clean.rsplit_once(':') {
        let value = value.trim();
        if !label.is_empty() && value.contains('.') {
            if let Ok(percent) = value.parse::<f64>() {
                return Some(RestoreEvent::Detail {
                    label: label.trim().to_string(),
                    percent,
                    text: clean,
                });
            }
        }
    }

    Some(RestoreEvent::Log(clean))
}

/// Runs the restore; `on_event` is called for every output event.
pub fn run(
    options: &RestoreOptions,
    on_event: &dyn Fn(RestoreEvent),
    cancel: &AtomicBool,
) -> Result<i32, String> {
    let tool = find_idevicerestore().ok_or_else(|| {
        "idevicerestore not found. Run scripts/install-toolchain.sh.".to_string()
    })?;
    let argv = build_command(&tool, options);
    on_event(RestoreEvent::Command(argv.join(" ")));

    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let lib = config::lib_dir();
    let mut ld = lib.to_string_lossy().to_string();
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        ld.push(':');
        ld.push_str(&existing.to_string_lossy());
    }
    command.env("LD_LIBRARY_PATH", ld);

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to start idevicerestore: {}", e))?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CANCELLED.to_string());
            }
            match line {
                Ok(text) => {
                    if let Some(event) = parse_progress_line(&text) {
                        on_event(event);
                    }
                }
                Err(_) => break,
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for process: {}", e))?;
    Ok(status.code().unwrap_or(-1))
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(erase: bool) -> RestoreOptions {
        RestoreOptions {
            ipsw_path: "/tmp/fw.ipsw".to_string(),
            erase,
            ecid: None,
            ignore_errors: false,
            variant: String::new(),
            cache_path: None,
            logfile: None,
        }
    }

    #[test]
    fn erase_flag_present() {
        let cmd = build_command(Path::new("idevicerestore"), &opts(true));
        assert!(cmd.contains(&"-e".to_string()));
    }

    #[test]
    fn update_has_no_erase_flag() {
        let cmd = build_command(Path::new("idevicerestore"), &opts(false));
        assert!(!cmd.contains(&"-e".to_string()));
    }

    #[test]
    fn non_interactive_and_plain() {
        let cmd = build_command(Path::new("idevicerestore"), &opts(true));
        assert!(cmd.contains(&"-y".to_string()));
        assert!(cmd.contains(&"-P".to_string()));
        assert_eq!(cmd.last().unwrap(), "/tmp/fw.ipsw");
    }

    #[test]
    fn ecid_hex() {
        let mut o = opts(true);
        o.ecid = Some(0x1234);
        let cmd = build_command(Path::new("idevicerestore"), &o);
        assert!(cmd.contains(&"0x1234".to_string()));
    }

    #[test]
    fn parse_progress() {
        match parse_progress_line("progress: 2 0.500000") {
            Some(RestoreEvent::Progress { step, overall, .. }) => {
                assert_eq!(step, 2);
                assert!(overall > 0.0 && overall <= 1.0);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn parse_detail() {
        match parse_progress_line("Sending KernelCache:  42.5") {
            Some(RestoreEvent::Detail { percent, .. }) => assert_eq!(percent, 42.5),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn parse_log() {
        assert!(matches!(
            parse_progress_line("Found device in DFU mode"),
            Some(RestoreEvent::Log(_))
        ));
    }

    #[test]
    fn parse_ansi_progress() {
        assert!(matches!(
            parse_progress_line("\u{1b}[1mprogress: 0 0.1\u{1b}[0m"),
            Some(RestoreEvent::Progress { .. })
        ));
    }
}
