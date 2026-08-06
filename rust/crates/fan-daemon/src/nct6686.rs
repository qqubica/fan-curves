//! Native Windows hardware backend: Nuvoton NCT6686D Super I/O (board temps,
//! fan tach, PWM control) + AMD Tctl over SMN — the LibreHardwareMonitorLib
//! replacement, talking to the same signed PawnIO driver the C# app uses.
//!
//! Protocol transcribed from LibreHardwareMonitor's `Nct677X.cs` (the
//! NCT6683D/6686D/6687D branch — the exact code path proven on this ASRock
//! X870 Steel Legend) and cross-checked against the Linux `nct6683.c` driver.
//!
//! Handback contract, the reason this app exists: the first write to a channel
//! saves the firmware's manual-mode bit and command byte; `release_control`
//! puts both back, returning the header to the EC's own curve. That runs from
//! `FanEngine::drop` — which is why the release profile keeps `panic = unwind`.
#![cfg(windows)]

use std::collections::HashMap;
use std::io;
use std::thread::sleep;
use std::time::{Duration, Instant};

use fan_core::backend::{HardwareBackend, HwControl, HwSensor, SensorKind};

use crate::pawnio::{self, HwMutex, Module};

// ---- Super I/O config ----
const CONFIG_PORTS: [(u64, u16); 2] = [(0, 0x2E), (1, 0x4E)]; // (slot, port)
const HWM_LDN: u64 = 0x0B;
const CHIP_ID_REG: u64 = 0x20;
const LDN_SELECT_REG: u64 = 0x07;
const BASE_ADDR_REG: u64 = 0x60;

// ---- EC register map (page<<8 | index) ----
const HWM_CFG: u16 = 0x180;
const TEMP_BASE: u16 = 0x100; // 16-bit, stride 2
const TEMP_COUNT: u16 = 7; // same slots LHM exposes for this chip
const FAN_RPM_BASE: u16 = 0x140; // 16-bit, stride 2
const PWM_READ_BASE: u16 = 0x160; // 8 channels, byte
const FAN_COUNT: u16 = 8; // exposed tachs (LHM shows 8 on this chip)
const FAN_CONTROL_MODE_REG: u16 = 0xA00; // one manual-mode bit per channel
const FAN_PWM_REQUEST_REG: u16 = 0xA01; // 0x80 = config phase, 0x40 = commit
const FAN_PWM_COMMAND_BASE: u16 = 0xA28; // target duty byte per channel
const FAN_CFG_REQ: u8 = 0x80;
const FAN_CFG_DONE: u8 = 0x40;
const CONTROL_COUNT: usize = 8;
const PAGE_FREE: u8 = 0xFF;

/// Header names by channel index (NCT6686D fan-control map).
const CONTROL_NAMES: [&str; CONTROL_COUNT] = [
    "CPU Fan",
    "Pump Fan",
    "System Fan #1",
    "System Fan #2",
    "System Fan #3",
    "System Fan #4",
    "System Fan #5",
    "System Fan #6",
];

/// Board temperature slots — LHM's exact labels, so the two apps show the same
/// names for the same registers.
const TEMP_NAMES: [&str; TEMP_COUNT as usize] =
    ["CPU", "System", "VRM MOS", "PCH", "CPU Socket", "PCIe x1", "M2 #1"];

// ---- AMD SMN ----
const THM_TCON_CUR_TMP: u64 = 0x0005_9800;
/// Per-CCD die temperatures (family 1Ah Granite Ridge), stride 4.
const CCD_TEMP_BASE: u64 = 0x0005_9B08;
const CCD_COUNT: usize = 2; // 9950X3D

// ---- identifiers ----
// The SAME strings LibreHardwareMonitorLib produces, so `profile.json` is
// interchangeable between the WPF app and this daemon. Changing them would make
// each app prune the other's assignments on launch — which is exactly how a
// manual header pick (Kuba's Pump Fan) gets silently lost.
const LPC_PREFIX: &str = "/lpc/nct6686d/0";
const CPU_PREFIX: &str = "/amdcpu/0/temperature";

struct Saved {
    mode_bit: u8,
    pwm: u8,
}

pub struct Nct6686Backend {
    description: String,
    lpc: Module,
    amd: Option<Module>,
    isa: HwMutex,
    pci: HwMutex,
    ec_base: u16,
    sensors: Vec<HwSensor>,
    controls: Vec<HwControl>,
    values: HashMap<String, f64>,
    rpms: HashMap<String, f64>,
    saved: HashMap<usize, Saved>,
}

impl Nct6686Backend {
    /// Detect the chip and build the sensor/control inventory. Returns an error
    /// (never a half-working backend) when PawnIO is missing, the driver refuses
    /// us, or no supported Super I/O answers.
    pub fn detect() -> io::Result<Self> {
        let version = pawnio::installed_version();
        let lpc = Module::load(pawnio::LPC_IO_BLOB)?;
        let isa = HwMutex::open(HwMutex::ISA_BUS);
        let pci = HwMutex::open(HwMutex::PCI);

        let (chip_id, ec_base) = {
            let _guard = isa
                .lock(100)
                .ok_or_else(|| io::Error::other("ISA bus busy — another monitoring tool is mid-transaction"))?;
            Self::detect_chip(&lpc)?
        };

        // CPU temperature module is optional: without it the board sensors still work.
        let amd = Module::load(pawnio::AMD_FAMILY17_BLOB).ok();

        let mut me = Self {
            description: format!(
                "NCT6686D (id {:04X}, EC base 0x{:03X}) via PawnIO {}{}",
                chip_id,
                ec_base,
                version.unwrap_or_else(|| "?".into()),
                if amd.is_some() { " + AMD SMN" } else { " (no CPU temp module)" }
            ),
            lpc,
            amd,
            isa,
            pci,
            ec_base,
            sensors: Vec::new(),
            controls: Vec::new(),
            values: HashMap::new(),
            rpms: HashMap::new(),
            saved: HashMap::new(),
        };
        me.init_and_enumerate()?;
        Ok(me)
    }

    /// Walk both config-port slots looking for a supported chip; leaves the
    /// module with BARs discovered so the EC window is reachable afterwards.
    fn detect_chip(lpc: &Module) -> io::Result<(u16, u16)> {
        for (slot, port) in CONFIG_PORTS {
            lpc.execute("ioctl_select_slot", &[slot], &mut [])?;
            // Enter extended-function mode (0x87 twice).
            lpc.execute("ioctl_pio_outb", &[port as u64, 0x87], &mut [])?;
            lpc.execute("ioctl_pio_outb", &[port as u64, 0x87], &mut [])?;

            let mut out = [0u64; 1];
            lpc.execute("ioctl_superio_inb", &[CHIP_ID_REG], &mut out)?;
            let id = out[0] as u8;
            lpc.execute("ioctl_superio_inb", &[CHIP_ID_REG + 1], &mut out)?;
            let rev = out[0] as u8;
            let chip = ((id as u16) << 8) | rev as u16;

            // Linux masks the low nibble; NCT6686D = 0xD44x (LHM: id 0xD4, rev 0x40/0x41).
            let supported = matches!(chip & 0xFFF0, 0xD440 | 0xC730 | 0xD590);
            if !supported {
                lpc.execute("ioctl_pio_outb", &[port as u64, 0xAA], &mut [])?; // exit config
                continue;
            }

            // BAR discovery must happen while still in config mode.
            lpc.execute("ioctl_find_bars", &[], &mut [])?;
            lpc.execute("ioctl_superio_outb", &[LDN_SELECT_REG, HWM_LDN], &mut [])?;
            lpc.execute("ioctl_superio_inw", &[BASE_ADDR_REG], &mut out)?;
            let base = out[0] as u16;
            sleep(Duration::from_millis(1));
            lpc.execute("ioctl_superio_inw", &[BASE_ADDR_REG], &mut out)?;
            let verify = out[0] as u16;
            lpc.execute("ioctl_pio_outb", &[port as u64, 0xAA], &mut [])?; // exit config

            if base != verify || base < 0x100 || (base & 0xF007) != 0 {
                continue; // unstable or nonsense window
            }
            return Ok((chip, base));
        }
        Err(io::Error::other("no supported Super I/O chip found on ports 0x2E/0x4E"))
    }

    // ---- EC window ----

    fn page_port(&self) -> u64 {
        (self.ec_base + 4) as u64
    }
    fn index_port(&self) -> u64 {
        (self.ec_base + 5) as u64
    }
    fn data_port(&self) -> u64 {
        (self.ec_base + 6) as u64
    }

    fn inb(&self, port: u64) -> io::Result<u8> {
        let mut out = [0u64; 1];
        self.lpc.execute("ioctl_pio_inb", &[port], &mut out)?;
        Ok(out[0] as u8)
    }

    fn outb(&self, port: u64, value: u8) -> io::Result<()> {
        self.lpc.execute("ioctl_pio_outb", &[port, value as u64], &mut [])?;
        Ok(())
    }

    /// Claim the page/index/data window: wait for another tool to release it
    /// (0xFF), force-claim after 500 ms rather than hanging forever. Always
    /// released back to 0xFF, including on error paths.
    fn claim_window(&self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if self.inb(self.page_port())? == PAGE_FREE {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.outb(self.page_port(), PAGE_FREE)?; // force-claim
                return Ok(());
            }
            sleep(Duration::from_millis(1));
        }
    }

    fn read_ec(&self, addr: u16) -> io::Result<u8> {
        self.claim_window()?;
        let r = (|| {
            self.outb(self.page_port(), (addr >> 8) as u8)?;
            self.outb(self.index_port(), (addr & 0xFF) as u8)?;
            self.inb(self.data_port())
        })();
        let _ = self.outb(self.page_port(), PAGE_FREE);
        r
    }

    fn write_ec(&self, addr: u16, value: u8) -> io::Result<()> {
        self.claim_window()?;
        let r = (|| {
            self.outb(self.page_port(), (addr >> 8) as u8)?;
            self.outb(self.index_port(), (addr & 0xFF) as u8)?;
            self.outb(self.data_port(), value)
        })();
        let _ = self.outb(self.page_port(), PAGE_FREE);
        r
    }

    fn read_ec16(&self, addr: u16) -> io::Result<u16> {
        let hi = self.read_ec(addr)? as u16;
        let lo = self.read_ec(addr + 1)? as u16;
        Ok((hi << 8) | lo)
    }

    // ---- init / enumeration ----

    fn init_and_enumerate(&mut self) -> io::Result<()> {
        let _guard = self
            .isa
            .lock(100)
            .ok_or_else(|| io::Error::other("ISA bus busy during init"))?;

        // Start the monitoring engine if the firmware left it off.
        let cfg = self.read_ec(HWM_CFG)?;
        if cfg & 0x80 == 0 {
            self.write_ec(HWM_CFG, cfg | 0x80)?;
        }

        // CPU sensors keep LHM's index numbering: 2 = Tctl/Tdie, 3/4 = CCD1/CCD2.
        if self.amd.is_some() {
            self.sensors.push(HwSensor {
                id: format!("{CPU_PREFIX}/2"),
                name: "CPU Core (Tctl/Tdie)".into(),
                kind: SensorKind::Temp,
            });
            for ccd in 0..CCD_COUNT {
                self.sensors.push(HwSensor {
                    id: format!("{CPU_PREFIX}/{}", 3 + ccd),
                    name: format!("CPU CCD{} (Tdie)", ccd + 1),
                    kind: SensorKind::Temp,
                });
            }
        }
        for i in 0..TEMP_COUNT {
            self.sensors.push(HwSensor {
                id: format!("{LPC_PREFIX}/temperature/{i}"),
                name: format!("Nuvoton NCT6686D — {}", TEMP_NAMES[i as usize]),
                kind: SensorKind::Temp,
            });
        }
        for i in 0..FAN_COUNT {
            self.sensors.push(HwSensor {
                id: format!("{LPC_PREFIX}/fan/{i}"),
                name: format!("Nuvoton NCT6686D — {}", CONTROL_NAMES[i as usize]),
                kind: SensorKind::Rpm,
            });
        }
        // All eight headers, like LHM — a capability probe (FANOUT_CFG bit 7)
        // hid two headers this board really does drive.
        for i in 0..CONTROL_COUNT {
            self.controls.push(HwControl {
                id: format!("{LPC_PREFIX}/control/{i}"),
                name: format!("Nuvoton NCT6686D — {}", CONTROL_NAMES[i]),
            });
        }
        Ok(())
    }

    fn control_index(id: &str) -> Option<usize> {
        id.strip_prefix(LPC_PREFIX)?.strip_prefix("/control/")?.parse().ok()
    }

    /// Tctl + per-CCD die temperatures from SMN (families 17h/19h/1Ah share the
    /// thermal register; the CCD block is family-1Ah Granite Ridge).
    fn read_cpu_temps(&self) -> Vec<(String, f64)> {
        let Some(amd) = self.amd.as_ref() else { return Vec::new() };
        let Some(_guard) = self.pci.lock(10) else { return Vec::new() };
        let mut out = Vec::new();
        let mut buf = [0u64; 1];

        if amd.execute("ioctl_read_smn", &[THM_TCON_CUR_TMP], &mut buf).is_ok() {
            let raw = buf[0] as u32;
            let mut t = (raw >> 21) as f64 * 0.125;
            let range_sel = raw & 0x0008_0000 != 0;
            let tj_sel = raw & 0x0003_0000 == 0x0003_0000;
            if range_sel || tj_sel {
                t -= 49.0;
            }
            // Zen 5 carries no k10temp offset; an out-of-range value means the
            // read failed and must NOT be published as a temperature.
            if t > 0.0 && t < 130.0 {
                out.push((format!("{CPU_PREFIX}/2"), t));
            }
        }

        for ccd in 0..CCD_COUNT {
            let addr = CCD_TEMP_BASE + 4 * ccd as u64;
            if amd.execute("ioctl_read_smn", &[addr], &mut buf).is_ok() {
                let raw = buf[0] as u32 & 0xFFF;
                let t = raw as f64 * 0.125 - 305.0;
                if raw != 0 && t < 125.0 && t > 0.0 {
                    out.push((format!("{CPU_PREFIX}/{}", 3 + ccd), t));
                }
            }
        }
        out
    }

    /// The per-channel PWM write, bracketed by the EC's config-phase handshake.
    fn write_pwm(&mut self, index: usize, duty: u8) -> io::Result<()> {
        if self.saved.get(&index).is_none() {
            let mode = self.read_ec(FAN_CONTROL_MODE_REG)?;
            let pwm = self.read_ec(FAN_PWM_COMMAND_BASE + index as u16)?;
            self.saved.insert(index, Saved { mode_bit: mode & (1 << index), pwm });
        }
        self.write_ec(FAN_PWM_REQUEST_REG, FAN_CFG_REQ)?;
        sleep(Duration::from_millis(50));
        let mode = self.read_ec(FAN_CONTROL_MODE_REG)?;
        self.write_ec(FAN_CONTROL_MODE_REG, mode | (1 << index))?;
        self.write_ec(FAN_PWM_COMMAND_BASE + index as u16, duty)?;
        self.write_ec(FAN_PWM_REQUEST_REG, FAN_CFG_DONE)?;
        sleep(Duration::from_millis(50));
        Ok(())
    }
}

impl Nct6686Backend {
    /// Full register snapshot: the mode register plus, per channel, the command
    /// byte we write (0xA28+i) and the EC's reported duty (0x160+i) with its
    /// tach. Comparing a header the other controller is actively driving
    /// against an idle one tells us which register actually reflects output.
    fn dump_registers(&self, label: &str) -> io::Result<()> {
        let mode = self.read_ec(FAN_CONTROL_MODE_REG)?;
        println!("--- {label}: mode 0x{mode:02X} (bits {mode:08b})");
        for i in 0..CONTROL_COUNT {
            let cmd = self.read_ec(FAN_PWM_COMMAND_BASE + i as u16)?;
            let duty = self.read_ec(PWM_READ_BASE + i as u16)?;
            let rpm = self.read_ec16(FAN_RPM_BASE + 2 * i as u16)?;
            println!(
                "    ch{i} {:<14} manual={} cmd=0x{cmd:02X} ({:>3.0}%) duty=0x{duty:02X} ({:>3.0}%) rpm={rpm}",
                CONTROL_NAMES[i],
                (mode >> i) & 1,
                cmd as f64 / 2.55,
                duty as f64 / 2.55,
            );
        }
        Ok(())
    }

    /// Write-path self-test against real silicon: snapshot every fan register,
    /// drive one header to a duty, snapshot again, restore, snapshot again.
    /// Intended for an UNUSED header (no fan attached, not in any channel) so
    /// the protocol can be proven without moving a fan or fighting another
    /// controller. The command register (0xA28+i) is the authoritative proof
    /// that a write reached the chip — the duty register can legitimately stay
    /// 0 on a header whose output the EC has disabled.
    pub fn selftest_write(&mut self, index: usize, percent: f64) -> io::Result<()> {
        if index >= CONTROL_COUNT {
            return Err(io::Error::other("header index out of range"));
        }
        let id = format!("{LPC_PREFIX}/control/{index}");
        let duty = (percent.clamp(0.0, 100.0) * 2.55) as u8;
        {
            let _guard = self.isa.lock(1000).ok_or_else(|| io::Error::other("ISA bus busy"))?;
            self.dump_registers("BEFORE")?;
            println!("\n>>> writing {percent:.0}% (0x{duty:02X}) to channel {index}\n");
            self.write_pwm(index, duty)?;
            self.dump_registers("AFTER WRITE")?;
        }
        self.release_control(&id);
        let _guard = self.isa.lock(1000).ok_or_else(|| io::Error::other("ISA bus busy"))?;
        self.dump_registers("AFTER RESTORE")?;

        let cmd = self.read_ec(FAN_PWM_COMMAND_BASE + index as u16)?;
        println!(
            "\nverdict: command register {} after restore (0x{cmd:02X})",
            if cmd == duty { "STILL HOLDS THE TEST VALUE — restore did not take" } else { "returned to its original value" }
        );
        Ok(())
    }
}

impl HardwareBackend for Nct6686Backend {
    fn description(&self) -> &str {
        &self.description
    }

    fn is_simulated(&self) -> bool {
        false
    }

    fn sensors(&self) -> &[HwSensor] {
        &self.sensors
    }

    fn controls(&self) -> &[HwControl] {
        &self.controls
    }

    fn update(&mut self) {
        let Some(_guard) = self.isa.lock(10) else { return }; // busy → keep last values
        for (id, t) in self.read_cpu_temps() {
            self.values.insert(id, t);
        }
        for i in 0..TEMP_COUNT {
            let Ok(msb) = self.read_ec(TEMP_BASE + 2 * i) else { continue };
            let Ok(lsb) = self.read_ec(TEMP_BASE + 2 * i + 1) else { continue };
            let t = (msb as i8) as f64 + 0.5 * ((lsb >> 7) & 1) as f64;
            self.values.insert(format!("{LPC_PREFIX}/temperature/{i}"), t);
        }
        for i in 0..FAN_COUNT {
            let Ok(rpm) = self.read_ec16(FAN_RPM_BASE + 2 * i) else { continue };
            self.values.insert(format!("{LPC_PREFIX}/fan/{i}"), rpm as f64);
        }
        for i in 0..CONTROL_COUNT {
            // The tach that belongs to control i is fan i on this chip.
            if let Some(rpm) = self.values.get(&format!("{LPC_PREFIX}/fan/{i}")).copied() {
                self.rpms.insert(format!("{LPC_PREFIX}/control/{i}"), rpm);
            }
            if let Ok(duty) = self.read_ec(PWM_READ_BASE + i as u16) {
                self.values.insert(format!("{LPC_PREFIX}/pwm/{i}"), duty as f64 / 2.55);
            }
        }
    }

    fn read_value(&self, sensor_id: &str) -> Option<f64> {
        self.values.get(sensor_id).copied()
    }

    fn set_control(&mut self, control_id: &str, percent: f64) {
        let Some(index) = Self::control_index(control_id) else { return };
        let Some(_guard) = self.isa.lock(10) else { return }; // busy → skip this tick, never write unlocked
        let duty = (percent.clamp(0.0, 100.0) * 2.55) as u8;
        let _ = self.write_pwm(index, duty);
    }

    fn release_control(&mut self, control_id: &str) {
        let Some(index) = Self::control_index(control_id) else { return };
        let Some(saved) = self.saved.remove(&index) else { return };
        // Handback must not be skipped just because the bus is busy: wait longer.
        let Some(_guard) = self.isa.lock(1000) else { return };
        let _ = (|| -> io::Result<()> {
            let mode = self.read_ec(FAN_CONTROL_MODE_REG)?;
            // Restore the firmware's own manual-mode bit exactly: clear it if
            // the BIOS had it clear (we set it), leave it if the BIOS had it
            // set. Restoring the saved value states that directly instead of
            // LHM's `mode & ~saved` shorthand.
            let bit = 1u8 << index;
            self.write_ec(FAN_CONTROL_MODE_REG, (mode & !bit) | saved.mode_bit)?;
            self.write_ec(FAN_PWM_REQUEST_REG, FAN_CFG_REQ)?;
            sleep(Duration::from_millis(50));
            self.write_ec(FAN_PWM_COMMAND_BASE + index as u16, saved.pwm)?;
            self.write_ec(FAN_PWM_REQUEST_REG, FAN_CFG_DONE)?;
            sleep(Duration::from_millis(50));
            Ok(())
        })();
    }

    fn read_control_rpm(&self, control_id: &str) -> Option<f64> {
        self.rpms.get(control_id).copied()
    }

    fn internal_sensor_count(&self) -> usize {
        self.sensors.len() // no hidden library history here — nothing to cap
    }
}
