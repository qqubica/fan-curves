//! Start-with-Windows for the daemon.
//!
//! A HKCU Run key cannot elevate, and the daemon needs administrator for
//! Super I/O access — so, exactly like the WPF app, the only workable
//! mechanism is a Task Scheduler logon task with `/RL HIGHEST`. Registering
//! one itself requires elevation, which the daemon already has when it is
//! driving hardware.
//!
//! The task is deliberately NOT registered automatically. The WPF app has its
//! own `FanCurves` task, and two controllers writing the same headers every
//! second is last-writer-wins; turning this on is therefore an explicit act,
//! and [`conflicting_wpf_task`] reports the collision so the daemon can say so.
#![cfg(windows)]

use std::process::Command;

/// Our task name. Distinct from the WPF app's `FanCurves`, so both can exist
/// and be enabled/disabled independently.
pub const TASK_NAME: &str = "FanCurvesDaemon";
/// The WPF app's task — the one that would fight us at boot.
pub const WPF_TASK_NAME: &str = "FanCurves";

fn schtasks(args: &[&str]) -> std::io::Result<(bool, String)> {
    let out = Command::new("schtasks").args(args).output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    Ok((out.status.success(), text))
}

fn task_exists(name: &str) -> bool {
    schtasks(&["/Query", "/TN", name]).map(|(ok, _)| ok).unwrap_or(false)
}

pub fn installed() -> bool {
    task_exists(TASK_NAME)
}

/// True when the WPF app would also start itself at logon.
pub fn conflicting_wpf_task() -> bool {
    task_exists(WPF_TASK_NAME)
}

/// Register (or refresh) the logon task pointing at this executable.
pub fn install() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let (ok, text) = schtasks(&[
        "/Create",
        "/TN",
        TASK_NAME,
        "/TR",
        &format!("\"{}\"", exe.display()),
        "/SC",
        "ONLOGON",
        "/RL",
        "HIGHEST",
        "/F",
    ])?;
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("schtasks /Create failed: {}", text.trim())))
    }
}

pub fn uninstall() -> std::io::Result<()> {
    let (ok, text) = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"])?;
    // Deleting something that isn't there is a success from our side.
    if ok || !task_exists(TASK_NAME) {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("schtasks /Delete failed: {}", text.trim())))
    }
}
