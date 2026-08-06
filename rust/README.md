# fan-curves Rust port

Rust rewrite of the temperature-only engine, targeting a headless daemon +
on-demand UI split across Windows and Linux.

## Layout

- `crates/fan-core` — the engine. Each module mirrors one C# file in
  `src/FanCurves.Core`: `curve` (staircase), `filter` (the MacBook feel:
  rolling average, step-down hold + hysteresis, slew, zero snap), `kick`,
  `probe`, `profile` (serde-compatible with the WPF app's `profile.json`),
  `engine` (tick orchestration), `backend` (hardware trait), `sim`
  (thermal-plant simulation). No dependencies beyond serde. Every component
  takes monotonic `now` seconds as a parameter — the caller owns the clock, so
  tests replay any timeline deterministically.
- `crates/fan-daemon` — headless engine loop: jittered ~1 s tick, profile
  loading/auto-assign, clean handback of every header to the BIOS on exit
  (engine `Drop`) and Ctrl+C. Prints output CHANGES only (fan on/off, target
  steps, reason transitions); `--verbose` adds a line per tick. Flags: `--sim`,
  `--ticks N`, `--profile <path>`, `--no-apply`, `--verbose`, `--send <json>`.
  Only the simulated backend exists so far. Release builds use fat LTO +
  stripped symbols; panic stays `unwind` so a panic still returns the headers
  to the BIOS through `Drop`.

  The daemon also writes the review log (port of `TelemetryLog.cs`, same file
  names, CSV schema, number formats and behavior vocabulary as the WPF app —
  existing analysis tooling parses both): `logs/telemetry[-sim]-DATE.csv`
  daily-rotated / 7 kept, `behavior[-sim].txt` changes-only with a 4 MB cap,
  UTF-8 with BOM, buffered, first write failure disables logging for the
  session. One divergence: the local-time offset is captured at startup, so a
  DST flip mid-run shifts timestamps until restart (C# tracks DST live).

  IPC (`ipc.rs`): a local socket — named pipe on Windows, Unix socket on Linux
  (`fan-curves-daemon.sock`) — speaking line-delimited JSON. Binding it is also
  the single-instance check. Commands: `ping`, `status`, `profile`,
  `set_profile`, `preset` (quiet/performance, adopts tuning but keeps
  assignments), `apply`, `pause`, `shutdown` (the daemon's `exit.signal`).
  `fan-daemon --send '{"cmd":"status"}'` is the built-in client.
- `crates/fan-ui` — the on-demand UI (eframe/egui): live status, staircase
  chart with amber operating point, 10-min history strip, presets and
  apply/pause over the daemon's IPC. Repaints only when the 1 Hz poll lands
  fresh data; auto-spawns a sibling `fan-daemon --sim` when none answers; exits
  fully on close. v1 is a viewer/controller — curve editing, undo, the dev
  panel and history scrollback remain in the WPF app for now.
- `fan-daemon/src/pawnio.rs` + `nct6686.rs` — the Windows backend, replacing
  LibreHardwareMonitorLib. Talks to the signed PawnIO driver by DeviceIoControl
  (no DLL link), embedding the official signed 0.2.10 `LpcIO.bin` /
  `AMDFamily17.bin` modules (LGPL-2.1, github.com/namazso/PawnIO.Modules — the
  driver refuses anything unsigned). Super I/O detect → EC page/index/data
  window → board temps, tach, PWM; AMD Tctl and per-CCD temps over SMN. Takes
  the same `Global\Access_ISABUS.HTP.Method` / `Global\Access_PCI` mutexes every
  monitoring tool uses. Needs administrator.
- `fan-core/src/hwmon.rs` — the Linux backend: `/sys/class/hwmon` enumeration,
  temp/tach reads, pwm writes with pwmN_enable save/restore as the BIOS
  handback. Compile-checked via `cargo check --target x86_64-unknown-linux-gnu`;
  not yet exercised on a real Linux machine. The `nct6683` kernel driver needs
  `force=1` for PWM writes on many boards.

### Sensor/control identifiers are LHM-compatible

The Windows backend emits the SAME id strings LibreHardwareMonitorLib does —
`/lpc/nct6686d/0/{temperature,fan,control}/N`, `/amdcpu/0/temperature/{2,3,4}`
(Tctl, CCD1, CCD2) — so one `profile.json` works in both apps. Diverging here
would make each app prune the other's assignments at launch, which is exactly
how a manual header pick gets silently lost.

For the same reason the daemon treats the shared profile as **read-only** while
the WPF app is still the shipping controller: it only writes with
`--save-profile`, or when `--profile <path>` names its own file.

### Verification status (2026-08-06)

| Path | Status |
|---|---|
| Chip detect, EC reads, temps, tach, Tctl/CCD | **Verified on the real board** (NCT6686D id D441, EC base 0xA20, PawnIO 2.2.0): readings and computed outputs matched the running WPF app tick-for-tick |
| Non-elevated behaviour | **Verified**: clean refusal, falls back to simulation, never half-works |
| PWM write + BIOS handback | **NOT yet verified on hardware** — transcribed from LHM, compiles, untested |

To verify the write path, from an **elevated** shell (header 7 = System Fan #6
has no fan attached and is in no channel, so nothing spins):

```
cargo build -p fan-daemon
target\debug\fan-daemon.exe --selftest-write 7 40
```

It prints the mode register and duty before / after the write / after restore.
Success = the manual bit goes 0→1→0 and the duty reads back ~40% then returns
to its original value. Only after that should the daemon be allowed to drive
real headers — and never while the WPF app is running (two controllers writing
the same header is last-writer-wins).
- `parity-harness` — C# console app referencing the real `FanCurves.Core`; it
  generates golden per-tick traces into `crates/fan-core/tests/golden/`.

## Build / test

```
cargo test                                  # unit tests + C# parity replay
cargo run -p fan-daemon -- --sim --ticks 10
```

## Parity contract

`tests/golden.rs` replays the committed golden traces through the port and
compares every output column (tolerance 1e-9; in practice bit-identical — both
sides run the same f64 operations in the same order). After any intentional
behaviour change in `FanCurves.Core`, regenerate the traces:

```
dotnet run --project rust/parity-harness
```

Otherwise the golden tests pin the old behaviour and fail honestly.
