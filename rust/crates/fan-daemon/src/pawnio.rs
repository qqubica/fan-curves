//! PawnIO transport: the signed kernel-driver bridge that gives user mode the
//! port I/O and PCI-config access this app needs (HVCI-compatible, which is why
//! the C# app requires it too).
//!
//! No link-time dependency on PawnIOLib.dll — we speak to `\\?\GLOBALROOT\Device\PawnIO`
//! with DeviceIoControl directly, exactly as LibreHardwareMonitor does. The
//! module blobs are the OFFICIAL SIGNED 0.2.10 binaries embedded at build time;
//! the driver rejects anything else (self-compiled modules cannot be loaded).
#![cfg(windows)]

use std::ffi::c_void;
use std::io;
use std::ptr;

// Module blobs (LGPL-2.1, github.com/namazso/PawnIO.Modules release 0.2.10).
pub const LPC_IO_BLOB: &[u8] = include_bytes!("../modules/LpcIO.bin");
pub const AMD_FAMILY17_BLOB: &[u8] = include_bytes!("../modules/AMDFamily17.bin");

const DEVICE_PATH: &str = r"\\?\GLOBALROOT\Device\PawnIO";
const IOCTL_PIO_LOAD_BINARY: u32 = 0xA1B2_2084;
const IOCTL_PIO_EXECUTE_FN: u32 = 0xA1B2_2104;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: isize = -1;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x80;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> isize;
    fn DeviceIoControl(
        device: isize,
        code: u32,
        in_buf: *const u8,
        in_size: u32,
        out_buf: *mut u8,
        out_size: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn CreateMutexW(sa: *mut c_void, initial_owner: i32, name: *const u16) -> isize;
    fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn WaitForSingleObject(h: isize, ms: u32) -> u32;
    fn ReleaseMutex(h: isize) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// One loaded PawnIO module instance.
pub struct Module {
    handle: isize,
}

impl Module {
    pub fn load(blob: &[u8]) -> io::Result<Self> {
        let path = wide(DEVICE_PATH);
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let me = Self { handle };
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_PIO_LOAD_BINARY,
                blob.as_ptr(),
                blob.len() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(me)
    }

    /// Call a module public. `in_`/`out` are counts of u64 slots, as the driver
    /// expects; the name occupies a fixed 32-byte prefix of the input buffer.
    pub fn execute(&self, name: &str, in_: &[u64], out: &mut [u64]) -> io::Result<usize> {
        let mut buf = vec![0u8; 32 + in_.len() * 8];
        let bytes = name.as_bytes();
        let n = bytes.len().min(31);
        buf[..n].copy_from_slice(&bytes[..n]);
        for (i, v) in in_.iter().enumerate() {
            buf[32 + i * 8..32 + i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_PIO_EXECUTE_FN,
                buf.as_ptr(),
                buf.len() as u32,
                out.as_mut_ptr() as *mut u8,
                (out.len() * 8) as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            // Never mask a failure as zeros — LHM's wrapper does, and that is
            // exactly how "Tctl reads 0" hid a dead driver for a whole session.
            return Err(io::Error::last_os_error());
        }
        Ok(returned as usize / 8)
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

/// Is the PawnIO driver present? (Registry check, no device open — mirrors
/// LHM's `PawnIo.IsInstalled`.)
pub fn installed_version() -> Option<String> {
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO",
            "/v",
            "DisplayVersion",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|l| l.contains("DisplayVersion"))
        .and_then(|l| l.split_whitespace().last())
        .map(|s| s.to_string())
}

/// Cross-process hardware-access mutex. The SAME names every monitoring tool
/// uses (HWiNFO, FanControl, AIDA64, LHM) — taking them is what keeps two tools
/// from interleaving an index/data pair. If it cannot be created or opened we
/// run without it, degraded but functional (LHM does the same).
pub struct HwMutex {
    handle: isize,
}

impl HwMutex {
    pub const ISA_BUS: &'static str = r"Global\Access_ISABUS.HTP.Method";
    pub const PCI: &'static str = r"Global\Access_PCI";

    pub fn open(name: &str) -> Self {
        let n = wide(name);
        let mut handle = unsafe { CreateMutexW(ptr::null_mut(), 0, n.as_ptr()) };
        if handle == 0 {
            const MUTEX_MODIFY_STATE: u32 = 0x0001;
            const SYNCHRONIZE: u32 = 0x0010_0000;
            handle = unsafe { OpenMutexW(MUTEX_MODIFY_STATE | SYNCHRONIZE, 0, n.as_ptr()) };
        }
        Self { handle }
    }

    /// Returns a guard when the lock is held (or when there is no mutex to
    /// take); `None` means "another tool is mid-transaction — skip this
    /// operation", which is what LHM does rather than proceeding unlocked.
    ///
    /// The guard carries the raw handle rather than a borrow, so holding it
    /// does not freeze the backend as immutable while it drives the chip.
    pub fn lock(&self, timeout_ms: u32) -> Option<HwMutexGuard> {
        if self.handle == 0 {
            return Some(HwMutexGuard { handle: 0 });
        }
        let r = unsafe { WaitForSingleObject(self.handle, timeout_ms) };
        // An abandoned mutex still transfers ownership to us.
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
            Some(HwMutexGuard { handle: self.handle })
        } else {
            None
        }
    }
}

impl Drop for HwMutex {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

pub struct HwMutexGuard {
    handle: isize,
}

impl Drop for HwMutexGuard {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe { ReleaseMutex(self.handle) };
        }
    }
}
