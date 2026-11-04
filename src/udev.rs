//! Automatic udev rule installation.
//!
//! The AppImage bundles every runtime dependency, but Linux still needs a udev
//! rule so the current user can open Apple USB devices in DFU / recovery mode.
//! This module checks for the rule and installs it on first launch using the
//! system's privilege escalation helper (`pkexec`, `sudo` with a graphical
//! askpass helper, or `sudo` inside a terminal emulator).
//!
//! The rule itself is embedded at compile time from
//! `scripts/70-pear-apple.rules`, so the AppImage does not need any external
//! files.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

pub const RULE_FILE_NAME: &str = "70-pear-apple.rules";
pub const RULE_CONTENT: &str = include_str!("../scripts/70-pear-apple.rules");

/// Absolute path of the installed udev rule.
pub fn rule_path() -> PathBuf {
    PathBuf::from("/etc/udev/rules.d").join(RULE_FILE_NAME)
}

/// Returns `true` when our cross-distro rule is already installed.
///
/// The old 99-prefixed rule ran after systemd applied ACLs and is deliberately
/// not accepted. The 70-prefixed rule is installed on the next launch.
/// An old rule that only used `GROUP="plugdev"` is treated as missing, because
/// that group does not exist on Arch or Fedora. Re-installing replaces it with
/// the `uaccess` version.
pub fn is_installed() -> bool {
    match fs::read_to_string(rule_path()) {
        Ok(content) => content.contains("idVendor") && content.contains("uaccess"),
        Err(_) => false,
    }
}

/// Installs the udev rule, asking for administrator permission if needed.
///
/// Returns `Ok(())` once the rule file exists and udev has been reloaded.
pub fn install() -> Result<(), String> {
    if is_installed() {
        return Ok(());
    }

    let script = write_install_script()?;
    let result = run_privileged(&script);
    let _ = fs::remove_file(&script);
    result?;

    // Give udev a moment to apply the rule before verifying it.
    thread::sleep(Duration::from_millis(300));
    if is_installed() {
        Ok(())
    } else {
        Err("The udev rule was not found after installation.".to_string())
    }
}

fn write_install_script() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("pear-udev-install-{}.sh", std::process::id()));
    let rule = RULE_CONTENT.trim_end();
    let target = rule_path();
    let target = target.display();

    let body = format!(
        "#!/bin/sh\n\
         set -e\n\
         cat > '{target}' <<'PEAR_RULE_EOF'\n\
         {rule}\n\
         PEAR_RULE_EOF\n\
         chmod 644 '{target}'\n\
         \n\
         UDEVADM=\"$(command -v udevadm || true)\"\n\
         if [ -z \"$UDEVADM\" ]; then\n\
           for candidate in /usr/sbin/udevadm /sbin/udevadm /usr/bin/udevadm; do\n\
             if [ -x \"$candidate\" ]; then UDEVADM=\"$candidate\"; break; fi\n\
           done\n\
         fi\n\
         if [ -n \"$UDEVADM\" ]; then\n\
           \"$UDEVADM\" control --reload-rules || true\n\
           \"$UDEVADM\" trigger --subsystem-match=usb || \"$UDEVADM\" trigger || true\n\
         fi\n\
         \n\
         if [ \"${{1:-}}\" = \"--pause\" ]; then\n\
           printf '\\nPress Enter to close this window...'\n\
           read -r _\n\
         fi\n",
        target = target,
        rule = rule,
    );

    fs::write(&path, body).map_err(|e| format!("Cannot write helper script: {e}"))?;
    set_executable(&path)?;
    Ok(path)
}

fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path)
        .map_err(|e| format!("Cannot read helper script permissions: {e}"))?
        .permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms)
        .map_err(|e| format!("Cannot make helper script executable: {e}"))
}

fn run_privileged(script: &Path) -> Result<(), String> {
    if is_root() {
        return run_sh(script, false);
    }

    if which("pkexec").is_some() {
        let sh = if Path::new("/bin/sh").exists() {
            "/bin/sh"
        } else {
            "/usr/bin/sh"
        };
        let status = Command::new("pkexec")
            .arg(sh)
            .arg(script)
            .status()
            .map_err(|e| format!("Could not start pkexec: {e}"))?;

        if status.success() {
            return Ok(());
        }
        // Exit code 127 usually means "no polkit agent / not authorized".
        // Fall back to sudo so the app still works on minimal desktops.
        if status.code() == Some(127) && which("sudo").is_some() {
            return run_sudo(script);
        }
        return check_status(status);
    }

    if which("sudo").is_some() {
        return run_sudo(script);
    }

    Err(
        "Neither pkexec nor sudo was found. Install polkit (pkexec) or sudo and try again."
            .to_string(),
    )
}

fn run_sh(script: &Path, pause: bool) -> Result<(), String> {
    let mut cmd = Command::new("sh");
    cmd.arg(script);
    if pause {
        cmd.arg("--pause");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("Could not run the installer: {e}"))?;
    check_status(status)
}

fn run_sudo(script: &Path) -> Result<(), String> {
    // Prefer a graphical askpass helper so the password prompt is a dialog.
    if let Some(askpass) = write_askpass_helper() {
        let status = Command::new("sudo")
            .arg("-A")
            .arg("sh")
            .arg(script)
            .env("SUDO_ASKPASS", &askpass)
            .status()
            .map_err(|e| format!("Could not start sudo: {e}"))?;
        let _ = fs::remove_file(&askpass);
        return check_status(status);
    }

    // Last resort: run sudo inside a terminal emulator.
    run_in_terminal(script)
}

fn write_askpass_helper() -> Option<PathBuf> {
    let (program, args) = if which("zenity").is_some() {
        ("zenity", "--password --title=\"PEAR Configurator\"")
    } else if which("kdialog").is_some() {
        ("kdialog", "--password \"PEAR Configurator\"")
    } else {
        return None;
    };

    let path = std::env::temp_dir().join(format!("pear-askpass-{}.sh", std::process::id()));
    let body = format!("#!/bin/sh\nexec {program} {args}\n");
    fs::write(&path, body).ok()?;
    set_executable(&path).ok()?;
    Some(path)
}

fn run_in_terminal(script: &Path) -> Result<(), String> {
    const TERMINALS: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--wait", "--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-e"]),
        ("mate-terminal", &["-e"]),
        ("xterm", &["-e"]),
    ];

    for (term, args) in TERMINALS {
        if which(term).is_none() {
            continue;
        }

        let mut cmd = Command::new(term);
        cmd.args(*args);
        cmd.arg("sudo").arg("sh").arg(script).arg("--pause");

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start {term}: {e}"))?;

        let deadline = Instant::now() + Duration::from_secs(180);
        let mut exited: Option<ExitStatus> = None;
        let mut grace: Option<Instant> = None;

        loop {
            if is_installed() {
                return Ok(());
            }

            if exited.is_none() {
                if let Ok(Some(status)) = child.try_wait() {
                    exited = Some(status);
                    // Some terminals fork and return immediately; wait a bit
                    // for the actual install script to finish.
                    grace = Some(Instant::now() + Duration::from_secs(10));
                }
            }

            if let (Some(status), Some(grace)) = (exited.as_ref(), grace) {
                if Instant::now() >= grace {
                    return check_status(status.clone());
                }
            }

            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err("Timed out waiting for the administrator password.".to_string());
            }

            thread::sleep(Duration::from_millis(300));
        }
    }

    Err("No graphical terminal emulator was found for the sudo password prompt.".to_string())
}

fn check_status(status: ExitStatus) -> Result<(), String> {
    if status.success() {
        return Ok(());
    }
    match status.code() {
        Some(126) => Err("Administrator permission was cancelled.".to_string()),
        Some(127) => Err(
            "Administrator permission was denied or no authentication agent is available."
                .to_string(),
        ),
        Some(code) => Err(format!("Installation failed (exit code {code}).")),
        None => Err("Installation was interrupted.".to_string()),
    }
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_is_cross_distro() {
        assert!(RULE_CONTENT.contains("idVendor"));
        assert!(RULE_CONTENT.contains("05ac"));
        assert!(RULE_CONTENT.contains("uaccess"));
    }

    #[test]
    fn rule_precedes_seat_acl_application() {
        assert!(RULE_FILE_NAME < "73-seat-late.rules");
    }

    #[test]
    fn rule_path_is_standard() {
        assert_eq!(
            rule_path(),
            PathBuf::from("/etc/udev/rules.d/70-pear-apple.rules")
        );
    }

    #[test]
    fn install_script_contains_rule_and_reload() {
        let path = write_install_script().expect("helper script");
        let body = fs::read_to_string(&path).expect("read helper script");
        let _ = fs::remove_file(&path);

        assert!(body.contains("PEAR_RULE_EOF"));
        assert!(body.contains("idVendor"));
        assert!(body.contains("udevadm"));
        assert!(body.contains("--reload-rules"));
        assert!(body.contains("--subsystem-match=usb"));
    }
}
