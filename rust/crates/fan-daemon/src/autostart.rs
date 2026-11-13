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
use std::path::PathBuf;

/// Our task name. Distinct from the WPF app's `FanCurves`, so both can exist
/// and be enabled/disabled independently.
pub const TASK_NAME: &str = "FanCurvesDaemon";
pub const TRAY_TASK_NAME: &str = "FanCurvesTray";
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
    task_exists(TASK_NAME) && task_exists(TRAY_TASK_NAME)
}

/// True when the WPF app would also start itself at logon.
pub fn conflicting_wpf_task() -> bool {
    // The installer keeps the legacy task registered but disables it. Only an
    // enabled task can start a second controller and contend for the headers.
    // The task XML is not localized, unlike schtasks' list output, and
    // schtasks starts in ~40 ms where powershell.exe took ~800 ms.
    match schtasks(&["/Query", "/TN", WPF_TASK_NAME, "/XML"]) {
        Ok((true, xml)) => !task_xml_disabled(&xml),
        _ => false,
    }
}

/// The task-level switch is `<Settings><Enabled>false</Enabled>`, written only
/// when the task is disabled. Triggers carry their own `<Enabled>`, so only the
/// Settings block counts.
fn task_xml_disabled(xml: &str) -> bool {
    xml.split_once("<Settings>")
        .and_then(|(_, rest)| rest.split_once("</Settings>"))
        .is_some_and(|(settings, _)| settings.contains("<Enabled>false</Enabled>"))
}

#[cfg(test)]
mod tests {
    use super::task_xml_disabled;

    #[test]
    fn only_the_settings_switch_disables_a_task() {
        let task = |triggers: &str, settings: &str| {
            format!("<Task><Triggers><LogonTrigger>{triggers}</LogonTrigger></Triggers>\
                     <Settings><StartWhenAvailable>true</StartWhenAvailable>{settings}</Settings></Task>")
        };
        assert!(!task_xml_disabled(&task("", "")));
        assert!(task_xml_disabled(&task("", "<Enabled>false</Enabled>")));
        assert!(!task_xml_disabled(&task("<Enabled>false</Enabled>", "")));
        assert!(!task_xml_disabled(&task("", "<Enabled>true</Enabled>")));
    }
}

/// The installed copy is stable across source checkouts and temporary worktrees.
fn installed_exe() -> std::io::Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| std::io::Error::other("LOCALAPPDATA is unset"))?;
    let exe = PathBuf::from(local).join("Programs/FanCurvesRust/fan-daemon.exe");
    if !exe.is_file() {
        return Err(std::io::Error::other(format!(
            "installed daemon missing at {}; run install-windows.ps1 first",
            exe.display()
        )));
    }
    Ok(exe)
}

fn registration_script() -> std::io::Result<PathBuf> {
    let exe = installed_exe()?;
    let script = exe.with_file_name("register-autostart.ps1");
    if !script.is_file() {
        return Err(std::io::Error::other(format!(
            "startup registration script missing at {}; run install-windows.ps1 first",
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
