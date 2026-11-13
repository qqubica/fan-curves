//! Start-with-Windows for the daemon and the tray.
//!
//! A HKCU Run key cannot elevate, and the daemon needs administrator for
//! Super I/O access — so the mechanism is a pair of Task Scheduler logon
//! tasks: the daemon elevated and hidden, the tray at ordinary privilege.
//! Registering them itself requires elevation, which the daemon already has
//! when it is driving hardware.
//!
//! Registration is always an explicit act (`--install-autostart`, the
//! `set_autostart` IPC command, or the installer), never a side effect of
//! running. The tasks always point at the installed copy, so a development
//! build can never register itself.
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

pub const TASK_NAME: &str = "FanCurvesDaemon";
pub const TRAY_TASK_NAME: &str = "FanCurvesTray";

fn task_exists(name: &str) -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", name])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn installed() -> bool {
    task_exists(TASK_NAME) && task_exists(TRAY_TASK_NAME)
}

/// The registration script that `windows/install.ps1` puts beside the
/// installed binaries.
fn registration_script() -> std::io::Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| std::io::Error::other("LOCALAPPDATA is unset"))?;
    let script = PathBuf::from(local).join("Programs/FanCurvesRust/register-autostart.ps1");
    if !script.is_file() {
        return Err(std::io::Error::other(format!(
            "{} is missing; run windows/install.ps1 first",
            script.display()
        )));
    }
    Ok(script)
}

fn change(verb: &str) -> std::io::Result<()> {
    let script = registration_script()?;
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .args(["-Action", verb])
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&out.stderr);
        Err(std::io::Error::other(format!(
            "autostart {verb} failed: {}",
            message.trim()
        )))
    }
}

/// Register both the elevated daemon and limited interactive tray tasks.
pub fn install() -> std::io::Result<()> {
    change("Install")
}

pub fn uninstall() -> std::io::Result<()> {
    change("Uninstall")
}
