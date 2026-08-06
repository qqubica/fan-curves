//! Tray launcher — the one job the tray has in this architecture.
//!
//! The daemon is the resident part and the UI is disposable, so there is
//! nothing to "minimise to tray": this exists purely so the app has a place to
//! live in the notification area, showing live status and opening the window.
//!
//! It runs **non-elevated on purpose**. The daemon needs administrator, and
//! anything it spawned would inherit that token — the UI would silently run as
//! admin. Keeping the launcher at normal privilege means the UI it starts is
//! normal too, which is also why the daemon's IPC pipe grants Interactive
//! Users (see `fan-daemon/src/ipc.rs`).
//!
//! Hand-rolled Shell_NotifyIcon rather than a tray crate: this process should
//! stay tiny, and the port already binds Win32 directly where it matters.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![cfg(windows)]

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, PostQuitMessage,
    RegisterClassW, SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage, HICON,
    HWND_MESSAGE, ICONINFO, MF_SEPARATOR, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_RETURNCMD,
    TPM_RIGHTALIGN, WM_APP, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericNamespaced, Stream, ToNsName};

const SOCKET_NAME: &str = "fan-curves-daemon.sock";
const WM_TRAY: u32 = WM_APP + 1;
const ID_OPEN: usize = 1;
const ID_PAUSE: usize = 2;
const ID_EXIT: usize = 3;
const TIMER_ID: usize = 1;
/// Tooltip refresh. Slow on purpose: this process must cost nothing.
const TOOLTIP_MS: u32 = 5_000;

/// Whether the daemon reported it is applying, for the menu wording.
static APPLYING: Mutex<bool> = Mutex::new(false);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---- daemon IPC (one short call, no persistent connection) ----

fn call(request: &str) -> Option<serde_json::Value> {
    let name = SOCKET_NAME.to_ns_name::<GenericNamespaced>().ok()?;
    let conn = Stream::connect(name).ok()?;
    let (recv, send) = conn.split();
    let mut w = BufWriter::new(send);
    w.write_all(request.as_bytes()).ok()?;
    w.write_all(b"\n").ok()?;
    w.flush().ok()?;
    let mut line = String::new();
    BufReader::new(recv).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// `Fan Curves — 46°→25% · 39°→10%`, or why not.
fn status_text() -> String {
    let Some(v) = call(r#"{"cmd":"status"}"#) else {
        return "Fan Curves — daemon not running".into();
    };
    *APPLYING.lock().unwrap() = v["applying"].as_bool().unwrap_or(false);
    let parts: Vec<String> = v["channels"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    let t = c["effective_temp"].as_f64().unwrap_or(f64::NAN);
                    let p = c["output_percent"].as_f64().unwrap_or(0.0);
                    if t.is_nan() {
                        format!("—→{p:.0}%")
                    } else {
                        format!("{t:.0}°→{p:.0}%")
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let head = if *APPLYING.lock().unwrap() { "Fan Curves" } else { "Fan Curves (paused)" };
    let text = if parts.is_empty() { head.to_string() } else { format!("{head} — {}", parts.join(" · ")) };
    // NOTIFYICONDATA tooltips are capped; stay well inside it.
    if text.chars().count() > 60 {
        text.chars().take(60).collect::<String>() + "…"
    } else {
        text
    }
}

/// Launch the UI beside this executable. Non-elevated, because we are.
fn open_ui() {
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let _ = std::process::Command::new(dir.join("fan-ui.exe")).spawn();
        }
    }
}

// ---- tray icon ----

/// Turn the shared RGBA mark into an HICON (32-bit DIB + empty mask).
unsafe fn make_icon() -> HICON {
    let size = fan_core::icon::SIZE as i32;
    let rgba = fan_core::icon::rgba();

    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = size;
    bi.bmiHeader.biHeight = -size; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let color = CreateDIBSection(std::ptr::null_mut(), &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if color.is_null() || bits.is_null() {
        return std::ptr::null_mut();
    }
    // Win32 wants BGRA, premultiplied by alpha.
    let px = bits as *mut u8;
    for i in 0..(size as usize * size as usize) {
        let (r, g, b, a) = (rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2], rgba[i * 4 + 3]);
        let m = |c: u8| ((c as u32 * a as u32) / 255) as u8;
        *px.add(i * 4) = m(b);
        *px.add(i * 4 + 1) = m(g);
        *px.add(i * 4 + 2) = m(r);
        *px.add(i * 4 + 3) = a;
    }

    // An all-zero mask: the alpha channel does the shaping.
    let mask: HBITMAP = CreateBitmap(size, size, 1, 1, std::ptr::null());
    let mut info: ICONINFO = std::mem::zeroed();
    info.fIcon = 1;
    info.hbmMask = mask;
    info.hbmColor = color;
    let icon = CreateIconIndirect(&info);
    DeleteObject(color as _);
    DeleteObject(mask as _);
    icon
}

unsafe fn notify_data(hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    let t = wide(tip);
    let n = t.len().min(nid.szTip.len());
    nid.szTip[..n].copy_from_slice(&t[..n]);
    nid
}

static mut ICON: HICON = std::ptr::null_mut();

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONDBLCLK => open_ui(),
                WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            let mut nid = notify_data(hwnd, ICON, &status_text());
            nid.uFlags = NIF_TIP;
            Shell_NotifyIconW(NIM_MODIFY, &nid);
            0
        }
        WM_DESTROY => {
            let nid = notify_data(hwnd, ICON, "");
            Shell_NotifyIconW(NIM_DELETE, &nid);
            KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    AppendMenuW(menu, MF_STRING, ID_OPEN, wide("Open Fan Curves").as_ptr());
    let pause_label =
        if *APPLYING.lock().unwrap() { "Pause — fans to BIOS" } else { "Resume curves" };
    AppendMenuW(menu, MF_STRING, ID_PAUSE, wide(pause_label).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    // Closing the tray does NOT stop the daemon: the fans keep being managed.
    AppendMenuW(menu, MF_STRING, ID_EXIT, wide("Close tray (fans keep running)").as_ptr());

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // Required so the menu dismisses when focus goes elsewhere.
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    DestroyMenu(menu);
    match cmd as usize {
        ID_OPEN => open_ui(),
        ID_PAUSE => {
            let applying = *APPLYING.lock().unwrap();
            let _ = call(if applying { r#"{"cmd":"pause"}"# } else { r#"{"cmd":"apply"}"# });
            let mut nid = notify_data(hwnd, ICON, &status_text());
            nid.uFlags = NIF_TIP;
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
        ID_EXIT => {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
        _ => {}
    }
}

fn main() {
    // `--open` just launches the UI (handy for a shortcut) and exits.
    if std::env::args().any(|a| a == "--open") {
        open_ui();
        return;
    }

    unsafe {
        let class = wide("FanCurvesTray");
        let hinstance = GetModuleHandleW(std::ptr::null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);

        // A message-only window: no taskbar entry, no visible surface.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Fan Curves").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            eprintln!("could not create the tray window");
            std::process::exit(1);
        }

        ICON = make_icon();
        let nid = notify_data(hwnd, ICON, &status_text());
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            eprintln!("could not add the tray icon");
            std::process::exit(1);
        }
        SetTimer(hwnd, TIMER_ID, TOOLTIP_MS, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
