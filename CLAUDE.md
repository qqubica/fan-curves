# fan-curves — Windows fan control with a MacBook-like default

WPF (.NET 8) app that drives motherboard fan headers the way a MacBook manages its
fans: silent flat floor at idle, no reaction to short temperature spikes, smooth
ramp only under sustained load (class-level default slew 8 %/s both directions; the Quiet preset's CPU channel runs 9 up / 8 down since 2026-07-22). Built for Kuba's 2026 PC
(ASRock X870 Steel Legend WiFi + 9950X3D + NH-D15 + Arctic P14 Pro chain in a
Phanteks P600S), but hardware-agnostic.

**Temperature-only since 2026-08-06** (Kuba's ask: "remove all modes except the
temperature mode"): the thermal-budget controller, power-curve mode, control-mode
switch, hard-override fuse, learned thermal model, futility probe, downward
relief, power floor and every power sensor/knob were removed wholesale. Every
channel follows its temperature staircase through `ResponseFilter`, full stop.
The removed machinery's documentation and rationale live in `docs/history.md`
(dated entry 2026-08-06); the code is in git history before that date.

Development history and harness logs live in `docs/history.md`.

## Repo / releases

**Public GitHub repo `qqubica/fan-curves`** (MIT) since 2026-07-22 — nested repo
gitignored by the workspace parent; earlier history stays in the workspace repo.
No PRs, push straight to main. Public-facing docs = `README.md` + `docs/*.png`
(CLAUDE.md is published too — keep it accurate, nothing secret in it).

- README screenshots: exit the running instance (`exit.signal`), move
  `profile.json` aside so the pristine defaults show (Quiet preset checked), run
  the built **DLL** — `dotnet src/FanCurves/bin/Debug/net8.0-windows/FanCurves.dll
  --sim --screenshot docs/screenshot-simple.png` (+ `--dev` for the second shot) —
  which bypasses the requireAdministrator manifest (it lives only in the apphost
  exe), then restore profile.json and `schtasks /Run /TN FanCurves`.
- Release: bump `<Version>` in `src/FanCurves/FanCurves.csproj` →
  `dotnet publish src/FanCurves -c Release -r win-x64 --self-contained
  -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -o publish`
  → `Compress-Archive` the exe as `FanCurves-X.Y.Z-win-x64.zip` →
  `gh release create vX.Y.Z <zip>`. publish/ and *.zip are gitignored.
  `gh release create` makes the tag remotely — run `git fetch --tags` right
  after, so the tag exists locally (git-solo and other history tools only see
  local refs; the v0.1.0 tag was missed this way once).

## Rust port (`rust/`, started 2026-08-06)

Kuba's decision: port the app to Rust for minimal resource usage and a
universal Windows + Linux build. Target architecture: headless engine daemon +
on-demand egui UI + native per-OS hardware backends (PawnIO + NCT6686D on
Windows, hwmon/sysfs on Linux) — LibreHardwareMonitorLib eventually drops out.
Cargo workspace `rust/`: `crates/fan-core` (one module per C# Core file;
serde-compatible with the SAME `%AppData%\FanCurves\profile.json` — PascalCase
names, unknown fields ignored, missing fields take the C# defaults) and
`crates/fan-daemon` (jittered tick loop, auto-assign port, `--sim` / `--ticks` /
`--profile` / `--no-apply`, BIOS handback on drop/Ctrl+C). The WPF app stays
the shipping app until the port reaches feature parity.

- **Parity is enforced, not assumed**: `rust/parity-harness` (C# console app
  referencing FanCurves.Core) writes golden per-tick traces into
  `crates/fan-core/tests/golden/`; `cargo test` replays them through the port
  and compares every output column at 1e-9 (in practice bit-identical).
  **After any intentional behaviour change in FanCurves.Core, regenerate**
  (`dotnet run --project rust/parity-harness`) — otherwise the golden tests
  pin the old behaviour and fail honestly.
- The daemon must never save the real profile from a sim run (same hazard as
  C# `Profile.ReadOnly`): it only writes the profile when `--profile` names an
  explicit path.
- Engine components take monotonic `now` seconds as parameters — the daemon
  owns the clock; tests replay any timeline deterministically.
- Toolchain (installed 2026-08-06): rustup + VS Build Tools via winget. cargo
  is NOT on PATH in this harness's shells — use
  `& "$env:USERPROFILE\.cargo\bin\cargo.exe"` from `rust/`.
- **Footprint rules** (the port's reason to exist): release profile runs fat
  LTO + `codegen-units 1` + symbol strip, but `panic` stays **"unwind" on
  purpose** — a panicking daemon must unwind through `FanEngine::drop` so the
  headers go back to the BIOS; `panic = "abort"` would freeze the Super I/O at
  the last written PWM (the failure the C# engine treats as worse than dying).
  The daemon prints CHANGES ONLY by default (fan ON/OFF, target steps, reason
  transitions minus pure None↔Ramp flips — behavior.txt vocabulary);
  `--verbose` restores the per-tick line. No async runtime — one thread and a
  jittered sleep is the whole scheduler.
- **UI performance rules for the egui phase**, translated from the WPF render
  rules ahead of time so they bind when the UI is built: repaint via
  `request_repaint()` once per engine tick only (egui otherwise repaints on
  input); any perpetual animation (the fan glyph) capped at ≤ 20 fps; while
  hidden/minimized feed histories only, paint nothing; the UI is a separate
  process that fully exits on close — the daemon is the only resident part.
- **Phase 2 (done 2026-08-06): telemetry + IPC.** The daemon writes the review
  log with the SAME schema/formats/vocabulary as the C# `TelemetryLog` (sim
  runs append to the same `telemetry-sim-*.csv` files interchangeably; local
  UTC offset captured once at startup — DST flips shift timestamps until
  restart). IPC = local socket `fan-curves-daemon.sock` (named pipe on
  Windows), line-delimited JSON: ping / status / profile / set_profile /
  preset / apply / pause / shutdown; binding doubles as the single-instance
  lock; `--send <json>` is the built-in client. Service/autostart wiring
  deferred to the hardware-backend phase (autostarting a sim-only daemon is
  pointless).
- **UI feature parity with the WPF app is TRACKED, not assumed.** Done:
  simple mode, **developer mode** (`devpanel.rs`, all eleven groups with the
  WPF labels/ranges/step snapping/value formats/tooltips, master-checkbox
  headers, 45 % dim when off but still editable, SOURCES with live readings
  and header exclusivity), **curve editing** (drag / double-click add /
  right-click remove, committing on RELEASE so one drag is one undo entry),
  **undo-redo** (Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z, CurveEdit + TuningEdit), and
  the **why-chip** in the chart corner with the original wordings.
  Also done: the **app icon** (`icon.rs` — one geometry drawn twice, the WPF
  title-bar canvas and TrayIcon's 32×32; the glyph is STATIC on purpose, a
  perpetual spin violates the repaint rules), the strip's **hover crosshair +
  readout chip**, **stopped-time spans**, **CLEAR**, and the **three-size
  window cycle**.
  **Still missing**: history SCROLLBACK (wheel/drag/LIVE — needs the two-tier
  spill-file storage first; the port holds 600 samples in RAM and nothing on
  disk), tray presence with close-to-tray / autostart (needs a decision on
  whether the daemon or a tray helper owns residency), and custom window
  chrome. Full inventory of the WPF UI — every control, range and interaction
  rule — was extracted before porting; work from it rather than from memory
  (guessed ranges were wrong).
- **The layout follows the WPF grid `252 | * | Auto`** (verified by
  screenshotting the running .NET app and comparing): sidebar = hero label,
  int/frac hero numeral, status-chip pill, PROFILE, preset CARDS, full-width
  pause; middle = **ONE card** holding the segmented channel header, the
  detail line, the curve, the hint and the history strip; dev panel on the
  **RIGHT** as one card with letter-spaced headers. Do not reintroduce
  per-group boxes or move the panel between the hero and the chart.
  - egui: a `Frame` inside a horizontal row does NOT constrain its width — a
    right-to-left layout inside it claims the whole row (it ate the dev
    panel's space). Allocate the exact region with `allocate_ui_with_layout`
    first, then draw the card inside it.
  - The default font has no U+2713; the preset tick is drawn with two line
    segments, not typed.
  - Edits go through **`update_profile`**, never `set_profile`: the latter
    calls `replace_profile`, which clears the filters and turns a slider nudge
    into an averaging-window reset.
  - A **tuning** edit (curve, response knobs, floor value) renames the profile
    "Custom"; app-level switches and sensor/header assignment do not.
  - egui gotchas that cost time: a child `Ui` inside a horizontal parent
    inherits horizontal flow (the panel column needs an explicit
    `allocate_ui_with_layout(top_down)`), and `with_layout(right_to_left)`
    inside a vertical `Ui` claims ALL remaining height — put value labels in a
    `horizontal` row or the group stretches to fill the window.
- **Phase 3 (done 2026-08-06): egui UI v1** (`crates/fan-ui`) — on-demand
  viewer/controller over the IPC socket: hero average numeral, channel
  selector, readouts + why-chip, staircase chart with amber live overlay,
  10-min history strip, preset/apply/pause. Repaints ONLY when the 1 Hz poll
  delivers (the repaint rules hold); auto-spawns a sibling `fan-daemon --sim`;
  exits fully on close. ~115 MB working set while OPEN (egui + GPU context —
  the on-demand/exit design is what keeps that off the resident budget).
  Curve editing/undo/dev panel/scrollback stay WPF-only for now. egui gotcha
  recorded in `main.rs`: `Color32` is PREMULTIPLIED — white-at-alpha needs
  equal components (`from_rgba_premultiplied(a,a,a,a)`); components above
  alpha render as solid/additive white (the "white pill" bug).
- **Phase 4 (done 2026-08-06): native Windows backend** (`fan-daemon/src/
  pawnio.rs` + `nct6686.rs`) — LibreHardwareMonitorLib replaced. PawnIO is
  reached by **DeviceIoControl on `\\?\GLOBALROOT\Device\PawnIO`** (no DLL
  link; IOCTLs 0xA1B22084 load / 0xA1B22104 execute), with the **official
  signed 0.2.10 `LpcIO.bin` + `AMDFamily17.bin` embedded** via `include_bytes!`
  — the driver rejects unsigned modules, so self-compiled ones are impossible.
  Super I/O detect on 0x2E/0x4E (NCT6686D = id 0xD4 rev 0x4x; **`ioctl_find_bars`
  must run while still in config mode** or every later port read is
  ACCESS_DENIED) → EC window at `base+4/5/6` with the wait-for-0xFF /
  force-after-500 ms / always-release-to-0xFF page handshake → temps
  `0x100+2i`, tach `0x140+2i`, duty `0x160+i`; PWM writes are
  `0xA01=0x80 → 50 ms → set bit i in 0xA00 → 0xA28+i=duty → 0xA01=0x40 → 50 ms`,
  with the firmware's mode bit and duty saved on first write and restored on
  release. CPU temps over SMN: Tctl `0x59800` (>>21 ×0.125, −49 on range/tj
  bits), CCDs `0x59B08+4n` (&0xFFF ×0.125 −305). Mutexes
  `Global\Access_ISABUS.HTP.Method` + `Global\Access_PCI`, held around every
  sequence; a busy bus SKIPS the operation (never write unlocked), except
  handback which waits 1 s.
  - **Identifiers are LHM-compatible on purpose**: `/lpc/nct6686d/0/{temperature,
    fan,control}/N` and `/amdcpu/0/temperature/{2,3,4}` (Tctl, CCD1, CCD2) —
    one profile.json for both apps. Diverging makes each app prune the other's
    assignments (that is how a manual Pump Fan pick disappears). All 8 headers
    are exposed like LHM; the FANOUT_CFG capability probe hid two real ones.
  - The daemon treats the shared profile as **read-only unless `--save-profile`**
    while the WPF app is the shipping controller.
  - **Fully verified on the real board** (2026-08-06), reads AND writes.
    Detect + EC reads + Tctl/CCD matched the running WPF app tick-for-tick
    (same temps, same 20%/10% outputs); non-elevated runs refuse cleanly and
    fall back to simulation. The write self-test
    (`fan-daemon --selftest-write 7 40` elevated — header 7 has no fan and is
    in no channel) drove ch7's command register 0x80 → 0x66 → 0x80 with every
    other channel untouched: write handshake and handback both correct.
  - **What the register dump taught us** (`--selftest-write` prints all eight
    channels before/after/restore): `0xA28+i` is the COMMAND byte (what we
    write), `0x160+i` is the EC's ACTUAL output duty — they agree on driven
    headers (ch0/1 read 0x33 = the WPF app's 20%, ch2–5 read 0x19 = its 10%),
    but an unused header holds a command while reporting 0 output, so
    `0x160+i` alone can never prove a write landed. The mode register `0xA00`
    reads **0xFF on this board — every channel already "manual" from the
    BIOS**, so the `mode | (1<<i)` step is a no-op here and the restore
    correctly leaves the bit set rather than clearing state the firmware owns.
  - **The IPC pipe needs an explicit security descriptor** (found 2026-08-06
    during the first live handover): the daemon runs ELEVATED and the UI is a
    normal user process, so the default DACL on a high-integrity named pipe
    locks the UI out entirely ("access denied", UI shows OFFLINE and its
    auto-spawned sim daemon can't bind either). `ipc::bind` now creates the
    pipe with `D:(A;;GA;;;IU)` — generic-all for Interactive Users (whoever is
    logged in at the console; not Everyone, so a service or remote session
    cannot command the fans).
  - Never run both control loops at once: last writer wins per header.
  - **Live handover verified 2026-08-06** (Kuba's ask "run it and close the
    .NET version"): exit.signal stopped the WPF app, the Rust daemon took the
    same profile and drove the real headers through the full MacBook sequence
    — 40% → 25 s step-down hold → hysteresis → slew ramp to 20%, both NH-D15
    fans at ~330/373 rpm, exactly what the C# app produced at the same duty.
    The daemon has **no autostart and no tray**: after a reboot the scheduled
    task starts the WPF app again, which is the intended safe default until
    the port takes over for real.
- **Phase 5 (done 2026-08-06): Linux hwmon backend** (`fan-core/src/hwmon.rs`)
  — sysfs enumeration, millidegree temps, tach, pwm 0–255 writes with
  `pwmN_enable` saved on first write and restored on release (the SetDefault
  equivalent). Daemon `Backend` enum picks hwmon on Linux without `--sim`.
  Compile-checked via `cargo check --target x86_64-unknown-linux-gnu`; NOT yet
  run on real Linux hardware (nct6683 needs `force=1` for PWM writes on many
  boards).

## Layout

- `src/FanCurves.Core` — engine, no UI deps:
  - `FanCurve` — STAIRCASE curve: each point opens a flat band (at ≥ TempC run at
    Percent until the next point). No interpolation between points.
  - `ResponseFilter` — **the MacBook feel lives here, not in the curve shape**:
    the curve is driven by the rolling AVERAGE temp over AveragingSeconds (default
    20 s — spikes barely move it), the target may jump multiple steps at once,
    step-down hold timer starts the moment the avg ITSELF maps lower (2026-07-29,
    Kuba's ask — was: timer only ran once the offset was met, so the total wait
    was offset + full hold); the drop fires once StepDownHoldSeconds (default
    10 s) are served AND (avg + HysteresisC) maps lower at that moment (time
    served but offset unmet shows the Hysteresis why-chip; timer resets if the
    average pops back into the band) — no flapping at band edges,
    slew-rate limit (%/s, default 8 up and 8 down) for gradual audible ramps.
  - `FanEngine` — ~1 s tick, jittered (2026-07-22: each tick re-arms a one-shot timer
    with a random 850–1150 ms delay so sampling never phase-locks onto periodic system
    activity; safe because all time logic — averaging window, step-down hold, slew dt,
    idle kick — runs on the monotonic clock, not tick counts): read temps (max of
    assigned sensors) → filter → write PWM.
    `StopApplying()`/`Dispose()` hand every header back to the BIOS (`Control.SetDefault`).
  - `LhmBackend` — real hardware via LibreHardwareMonitorLib (needs admin for the
    kernel driver / Super I/O access). `SimulatedBackend` — fake AM5-ish CPU with
    spikes + load phases (a real thermal plant: sink 420 J/°C behind fan-dependent
    resistance, die ~0.055 °C/W above sink — written PWM actually changes the
    temperature), used automatically when not elevated or nothing controllable.
- `src/FanCurves` — WPF UI, **two modes**. Under the curve chart sits the
  **history strip** (`HistoryChart.cs`, Developer mode only — collapsed in
  simple mode, the curve chart takes the freed space): a 10-min window of the
  selected channel's rolling-average temp (bright trace), raw "now" temp (faint
  dashed) and commanded fan % (dim trace + under-fill), one sample per engine
  tick. **Storage is two-tier** (2026-08-04, "scrollable, don't keep everything
  in memory"): `ChannelHistory` keeps only the last 600 samples in a RAM ring
  (the live window, fed from `MainWindow.OnEngineTicked` in both modes so
  history is already populated when dev mode opens) and appends every sample to
  a per-channel binary spill file in `%TEMP%\FanCurves\` (fixed 10-byte
  QUANTIZED records — tenths int16 for temps/percent, whole-second uint32 time
  — the ring keeps exact doubles, only the disk is lossy, below display
  resolution; `FileOptions.DeleteOnClose` so it vanishes with the process even
  on a crash, ~24 h ≈ 0.9 MB/channel retention via a once-a-day compaction at
  2×; any file error silently degrades to the RAM-only ring). `HistorySample`
  carries the wall-clock `Time` of the measurement since the same change.
  **The strip scrolls back** through the spill via a `HistoryViewport` (in
  StripChart.cs, owned by MainWindow): wheel ≈1 min/notch (Shift 10×), drag
  pans, double-click or the **LIVE text-button** (appears next to CLEAR only
  while scrolled) returns to the live edge; scrolled reads go through a
  one-window cache so hover redraws don't touch the disk, reaching "now" snaps
  back to following live, channel switch resets to live, and a scrolled window
  is anchored to absolute sample indices so incoming ticks don't move it. The
  time axis prints wall-clock `HH:mm` labels at 5-minute boundaries with "now"
  (live) or the window-end `HH:mm:ss` (scrolled) at the right edge. Right edge
  = now when live (amber live dots — the only amber there; they and the live
  counting-up stopped-span vanish while scrolled); hover crosshair with a
  `clock · ago · avg · now · %` chip. Fan turn-ON events get a baseline tick,
  turn-OFF a dimmer one, with the m:ss stopped time (turn-OFF → next turn-ON)
  on a dim span between them (label seats on the card background; skipped only
  when it would overlap the previous span's label; an ongoing stop counts up
  live; no span when the stop predates the window). Left/right padding matches
  `CurveEditor` so the plots align. `StripChart.cs` holds the shared skeleton
  (padding, title, legend, wall-clock time axis, scroll/drag handling, hover
  crosshair/chip, trace + under-fill helpers); `TakeSnapshot()` at the top of
  `OnRender` fills the protected `Win`/`WinCount`/`IsLive` from the viewport.
  Dev mode's fixed window is **1336×830**; `EnterFixed` clamps both dimensions
  to the work area on small screens. A **CLEAR text-button** right of the
  HISTORY title (dev mode only) wipes ring + spill file on EVERY channel so the
  strip restarts from the right edge — a XAML overlay in the chart-card grid
  row (the strip is OnRender-only and can't host children), position tuned to
  seat beside the drawn title; the LIVE button is the same pattern one seat
  further right. Hover chips take several wordings and draw the widest that
  fits the plot (`DrawChip(dc, x, wide, narrow)`; `FormattedText` honours `\n`)
  so quarter-screen windows get a two-line chip.
  **Render robustness**: every chart render bails out below a minimum plot size
  (`StripChart.TooSmallToRender` / CurveEditor's inline check),
  every computed-range label clamp uses the `Math.Max(lo, hi)` guard, and
  `App.DispatcherUnhandledException` logs (throttled to one entry / 5 s) and
  **swallows** UI faults instead of dying — fan control must outlive the UI.
  **Render/tick performance rules** (2026-08-04, "very lightweight and fast"):
  (1) every chart pen/brush/typeface is a FROZEN static (`Paint.cs` builders;
  per-chart `static readonly` fields) — never allocate a Freezable inside
  OnRender, the charts repaint every tick and on every hover move; (2) while the
  window is hidden in the tray or minimized, `OnEngineTicked` only feeds the
  histories and the tray tooltip (deduped — Shell_NotifyIcon is called only when
  the whole-degree text changed) — ALL painting lives in `RefreshLiveUi()`, run
  per tick when visible and once from `IsVisibleChanged`/`StateChanged` to
  catch up on reopen/restore; (3) the title-bar fan spin (the app's one
  perpetual animation) is capped at 20 fps via `Timeline.SetDesiredFrameRate` —
  at the default 60 it forces a composition pass per frame the whole time the
  window shows.
  **Why-chip**: notification-style chip in the curve chart's top-left
  corner (`WhyChip` in MainWindow.xaml, both modes) explaining why the commanded %
  differs from the curve's configured level; hidden when they match. The engine
  classifies every tick (`OutputReason` in `FanEngine.cs`, most-specific-wins chain
  fed by diagnostics `ResponseFilter` exposes — CurveLevel/TargetLevel/
  PendingDownLevel/DownHoldRemaining/HysteresisHolding/SnappedToZero): slew ramp
  up/down (with target %), step-down hold (pending level + countdown), hysteresis,
  zero snap (names the sub-threshold curve %), safety floor, idle kick, stop probe.
  - Simple (default): two preset buttons (`Quiet · MacBook-like` — default,
    `Performance`), the two fan channels, read-only curve illustration with live
    overlay, Apply/Stop. No settings visible.
  - Developer (top-bar toggle, or `--dev`): **single-column panel, 300 px
    wide**. Every toggleable feature is a titled group whose **master checkbox
    IS the group header** (MicroLabel-styled content inside the checkbox;
    explanations in tooltips): CHANNEL RESPONSE (per-channel filter knobs,
    always active, no checkbox — the one deliberate exception: following the
    curve at all has no switch, its knobs can be flattened individually),
    SAFETY FLOOR ("Never below" slider; see the behaviour contract),
    STOPPED-FAN KICK, STOP INSTEAD OF SLOW (zero snap), TRIAL STOPS (stop
    probe), INSTANT APPLY (bare header checkbox), then SOURCES, REVIEW LOGGING
    and HIGH PROCESS PRIORITY (bare header checkboxes — app plumbing, not
    control), SENSOR HISTORY, BACKEND. A group whose switch is off **dims to
    45 % opacity but stays editable** (settings must be reachable with the
    checkbox off).
    Curve editing: drag points, double-click add, right-click remove; edits snap
    to whole °C / whole %, bands stay ≥1 °C wide, max 12 points per channel;
    Ctrl+Z / Ctrl+Y (also Ctrl+Shift+Z) undo/redo — see the undo entry in the
    behaviour contract.
    Behaviour sliders: the averaging window covers 0–5 min on a non-linear
    slider — its Value is a notch index (5 s steps up to 2 min, 30 s beyond;
    mapping in `MainWindow.AvgNotchToSeconds`). Sensor/header assignment: each
    sensor row leads with its live temp and each header row with its live rpm
    (value column left-aligned so the reading hugs the checkbox, refreshed per
    engine tick via `RefreshSourceReadouts`, "—" when unreadable); the whole
    row renders in Cascadia Mono 11.5; long names **wrap** so the full name is
    always visible — no horizontal scrollers, and the shared CheckBox template's
    root is a Grid with a star content column because a horizontal StackPanel
    measures content at infinite width and silently disables wrapping. Backend +
    config-path info. Any manual tweak marks the profile "Custom" (preset
    highlight clears). Presets overwrite tuning but keep sensor/header
    assignments (`Profile.AdoptTuning`).
  Dark theme, code-behind (no MVVM framework).
  **Design language = "precision instrument"**: borderless window with custom
  chrome (`WindowChrome`, DWM rounded corners + dark frame via `Chrome.Apply` in
  `Ui.cs`, custom caption buttons, title-bar fan glyph that spins while applying).
  **Exactly three window sizes**: the fixed floating window
  (1010×660, 1336×830 in dev mode), quarter-of-screen (half work-area width × height, snapped to the nearest
  screen corner), and maximized — cycled in that order by the maximize caption
  button (its glyph previews the next size); drag-resize disabled
  (`ResizeMode=CanMinimize`, `ResizeBorderThickness=0`), and a `WM_GETMINMAXINFO` hook
  in `Chrome` clamps maximize to the work area (borderless windows otherwise cover the
  taskbar). Further elements: layered near-black surfaces (canvas `#0a0a0d`, cards `#111116` with light-from-above
  gradient hairline + drop shadow), monochrome white at graded opacities, and ONE
  accent — warm amber `#FF9E5E` — reserved strictly for live thermal state (chart
  operating dot + crosshair with axis readout chips, status-chip dot, and the
  dev-mode raw-temp dashed line + its "now …°" label). Hero = large
  Segoe UI Variable Display Light numeral of the selected channel's rolling average;
  micro-labels letter-spaced via `Tracked.Text` (hair spaces, `Ui.cs`); numerals in
  Cascadia Mono with invariant "." decimals. Channels switch via a segmented control
  (restyled ListBox) in the chart-card header; presets are selectable cards; global
  state lives in a status chip under the hero (no status bar). Chart: sparse horizontal
  grid only, glow-under white staircase, dashed amber vertical = raw temp (dev mode).
  Keep everything else monochrome — the amber stays live-data-only.

## Build / run

```
dotnet build                                # needs the .NET 8 SDK
dotnet run --project src/FanCurves          # real hardware if elevated, else simulation
dotnet run --project src/FanCurves -- --sim # force simulation
dotnet run --project src/FanCurves -- --sim --screenshot out.png      # UI verify aid (4 s, then exits)
dotnet run --project src/FanCurves -- --sim --screenshot out.png 330  # …after 330 s, so the strip has history
```

`dotnet` may not be on PATH in Git Bash — use `/c/Program Files/dotnet/dotnet`.

Continuous rebuild while developing: `.\dev-watch.ps1` from an **elevated** terminal —
rebuilds + relaunches on every source save using the clean cycle (exit.signal → fans
to BIOS → build → start), safe on real hardware. Don't use `dotnet watch run` against
real fans: its hard-kill restart skips the fans-to-BIOS handback, so a failed build
leaves the Super I/O frozen at the last written PWM. (`dotnet watch` is fine with `--sim`.)

## Behaviour contract

- **Admin-only**: `app.manifest` sets `requireAdministrator` (Super I/O access needs
  the kernel driver). Dev runs from a non-elevated shell trigger a UAC prompt.
- Default profile `Quiet (MacBook-like)` (**Kuba's hand-tuned settings promoted to
  default**): **all fans fully stopped at idle** (0% floor, 0% bottom step): CPU
  cooler 0% through <50°C avg, then 10/20/40% at 50/55/62°C — Kuba's hand-tuned
  low/mid bands — topped since 2026-08-06 with the original temperature-only
  ladder 50/65/81/90/100% at 70/76/84/88/92°C (restored from v0.1.0 when the
  power side, which used to carry sustained load above the old 50% cap, was
  removed), with a **90 s averaging window**, 1.5°C hysteresis, 25 s step-down
  hold, slew 9 %/s up / 8 down; case fans 0% below 40°C avg, then
  5/10/20/40/60/82/100% at 40/42/60/70/75/82/86°C, 25 s averaging, 4°C
  hysteresis, slew 7 %/s (whisper bands are fine on the NF-A14s, which start at
  any duty >0%).
- **Stopped-fan kick**: any channel the engine is actually driving that has sat at
  0% output for `IdleKickStoppedSeconds` gets spun to `IdleKickPercent` for
  `IdleKickSeconds`, then stops again (repeats after each stillness period). Curve
  demand >0% cancels the kick and resets the clock.
  `IdleKick` in Core (per-channel state in `FanEngine.Tick`, bypasses the slew
  filter); not active while paused/BIOS-controlled. Configurable in the dev panel
  (all global/app-level like the tray/autostart toggles — don't mark the profile
  "Custom", presets don't touch them): the STOPPED-FAN KICK group's master checkbox
  (`Profile.IdleKickEnabled`, **default false** — Kuba runs with the kick off)
  plus three sliders — idle time before
  kick (`IdleKickStoppedSeconds`, 10–300 s, default 10), kick speed
  (`IdleKickPercent`, 5–100 %, default 5), kick duration (`IdleKickSeconds`,
  5–60 s, default 5); engine pushes them into
  each `IdleKick` every tick. Sliders stay editable while the kick is disabled
  (settings must be reachable with the checkbox off). Gotcha: slider `ValueChanged` fires mid-XAML-parse
  (setting `Minimum`), so `OnKickParamChanged` bails while `KickTimeValue == null` —
  removing that guard crashes the app at startup.
- **Safety floor switch**: `Profile.SafetyFloorEnabled` (default true,
  app-level like the other feature toggles — doesn't mark "Custom", presets don't
  touch it) is the master switch of the per-channel `MinPercent`. Implemented in
  `FanEngine.Tick` as ONE gated local — `double minPct = Profile.SafetyFloorEnabled
  ? ch.MinPercent : 0` — that every floor read in the tick uses (the output/target,
  the `MinFloor` why-chip, the stop probe's `minPct <= 0` gate), and the settings
  fingerprint reads the gated value too, so flipping the switch snaps instantly
  like any other edit. With it off a floored channel can go to a full stop AND
  becomes trial-stoppable (the probe gate is the floor's, not a separate rule);
  the per-channel value itself is kept, so switching back restores it. Dev-panel
  group SAFETY FLOOR (master checkbox) + the "Never below" slider (0–60%, per
  channel, still marks the profile "Custom"); the behavior log's settings line
  prints `min off` in place of `min NN%`.
- **Zero snap (stop instead of running slow)**: any filter target
  above 0% but strictly below the threshold (default 20%) runs the fan at 0% —
  meaningful speed or fully stopped, never a slow crawl. Implemented in
  `ResponseFilter` (`ZeroSnapPercent`, snap applied to BOTH curve evaluations so
  hysteresis/step-down hold reason about the snapped level; slew still glides
  to/from real steps, so ramps pass through the low range transiently — that's
  intended). Channel `MinPercent` is applied after the filter and still wins while
  the safety floor is on (Performance CPU keeps its 30% floor);
  a snapped-to-0 channel counts as stopped
  for the idle kick. App-level like the kick (doesn't mark "Custom", presets don't
  touch it): `Profile.ZeroSnapEnabled` (default true) + `Profile.ZeroSnapPercent`
  (default 20), dev-panel group STOP INSTEAD OF SLOW (master checkbox) + "Stop
  below" slider (1–50%). With the 20% threshold and the
  default curves, no band is snapped (the snap is strictly-below, so
  the CPU's 20% band runs at 20%) — the feature only bites when a curve/threshold
  edit puts a band under the threshold.
- **Stop probe (trial-stop steadily running fans)**: once a driven
  channel has run continuously for `StopProbeRunSeconds` (default 30) with its RAW
  temp inside a `StopProbeStableRangeC` band (default 3.5°C), the fan is stopped
  as a trial. While stopped, the 5 s-average temp is compared to the pre-stop
  baseline every tick (continuous is safer than a periodic recheck):
  rise beyond the band, or curve demand above the level at stop → immediate resume.
  A stop that dies within `ProbeSeconds + RunSeconds` (~60 s at the defaults —
  trial length `StopProbeSeconds` default 30 s) counts as FAILED and
  arms a `StopProbeRetrySeconds` backoff (default 60 s) so a loaded fan never
  settles into an on/off cycle; a later rise resumes without backoff. `StopProbe`
  in Core (per-channel state in `FanEngine.Tick`, runs BEFORE the idle kick →
  probe-stopped counts as stopped for the kick, same choice as zero snap); channels
  with `MinPercent > 0` are never probed (safety floor wins — Performance CPU;
  unless `SafetyFloorEnabled` is off, which drops the floor to 0 everywhere).
  App-level like kick/zero-snap (doesn't mark "Custom", presets don't touch it):
  `Profile.StopProbeEnabled` (**default true**) + the five params, dev-panel
  group TRIAL STOPS (master checkbox) + sliders
  "Steady running before trial" (10–300 s), "Trial stop length" (5–60 s), "Stable
  band · rise to resume" (0.5–5°C), "Retry after failed trial" (60–900 s), "No
  trials above" (`StopProbeMaxTempC`, 50–90°C, default 78 —
  no trial starts while any raw-temp sample in the stability window is above it,
  and a running trial resumes the moment the 5 s average crosses it even inside
  the stable band, arming the failed-trial backoff as usual). The max-temp guard
  is also what keeps a die-limited load (CPU clamping its own temperature reads
  perfectly "stable" at full draw) from being trial-stopped — the clamped die
  sits near TjMax, far above the 78° ceiling.
- **Instant apply on settings change**: the engine fingerprints every
  control-shaping setting each tick (`FanEngine.SettingsSignature` — zero snap,
  per-channel tuning, the curve, sensor assignments; kick/probe params
  deliberately excluded — a false positive jumps the fan). A changed fingerprint
  means a user edit landed (slider, curve drag, preset), and every live
  `ResponseFilter` gets a one-shot `ApplyNow()`: on that tick the new settings'
  verdict is adopted immediately — no step-down hold, no hysteresis carry-over,
  no slew glide (before this an edit was read within ~1 s but *felt* only after
  the 25 s hold + ramp). Measurement state — the averaging window — is
  untouched: it encodes evidence, not settings.
  Switchable: `Profile.InstantApplyEnabled` (default true,
  app-level) — off, the fingerprint is still tracked but no `ApplyNow()` is sent,
  so an edit is read within a tick and *felt* after the normal hold + ramp.
  Dev-panel group INSTANT APPLY (bare header checkbox);
  settings line logs `instantApply on/off`.
- **Process priority High**: set best-effort in `App.OnStartup` (via
  `App.ApplyProcessPriority`, called right after the profile loads so the
  `Profile.HighPriorityEnabled` switch — dev panel HIGH PROCESS PRIORITY, default
  true — is honoured; toggling it applies High/Normal immediately). High priority
  keeps both the UI and the engine tick scheduled under full load (a 32-core load
  can otherwise starve the normal-priority UI thread until Windows ghosts the
  window); the app uses a few % of one core. Note: the 2026-07-28 Kernel-Power 41
  reboot was an OS-level hard crash, not the app — watch for recurrence, it may
  be platform instability under load on the new build.
- **Sensor-history cap (RAM)**: LibreHardwareMonitor keeps a rolling
  `List<SensorValue>` history for EVERY sensor it tracks internally (~150+ on this
  machine — all clocks, loads, voltages, not just the ones the app exposes), default
  window 1 day, appended ~every 4th `Update()` — tens of MB the app never reads
  (the 2026-08-05 "why is this 120+ MB" finding: 280 MB private bytes after 26 h).
  `Profile.SensorHistoryHours` (app-level, default **0 = off**) → the engine applies
  `IHardwareBackend.SetSensorHistoryWindow` at the top of `TickCore`, BEFORE
  `Update()` and on the tick thread — the setter mutates the same per-sensor lists
  the update appends to, so a UI-thread set would race. Dev-panel SENSOR HISTORY
  slider (plumbing area above BACKEND), notches off · 5/10/15/30 min · 1/2/4/8/12/24 h,
  value label shows the estimated RAM from the backend's real
  `InternalSensorCount` (sensors × windowSec/4 × 16 B). `LhmBackend` walks all
  hardware incl. SubHardware and hooks `SensorAdded` so late-appearing sensors
  inherit the window; `SimulatedBackend` is a no-op (nothing behind it keeps
  history).
- Changing ChannelConfig field names breaks saved `%AppData%\FanCurves\profile.json`
  (old fields silently ignored, defaults kick in) — delete it after schema changes.
  (The 2026-08-06 power removal only DELETED fields — remaining names unchanged,
  so existing profiles load fine; the orphaned power fields are dropped on the
  next save.)
- Sensor/control IDs are backend-specific; `AutoAssign` prunes IDs the current backend
  doesn't know and re-assigns empty channels on every launch (a profile saved in `--sim`
  works on real hardware and vice versa).
- **Dev flows never write the config** (`Profile.ReadOnly`, set from `--sim` /
  `--screenshot` in `App.OnStartup`): `Save()` is a no-op and the sensor dump goes to
  `sensors.sim.txt` instead of overwriting `sensors.txt` — a saving `--sim` run
  would let AutoAssign prune the hardware IDs the sim backend doesn't know and
  write `sim/*` in their place; auto-assignable headers heal on the next real
  launch, but **manual assignments (like Kuba's Pump Fan header) do not**.
  Keyed off the flags, not `hw.IsSimulated`, so an elevated launch that falls back to
  simulation because the kernel driver failed still saves the user's edits.
- Temperature display: simple mode shows only the rolling average (the thing that
  actually drives the steps) — hero numeral + segment readouts; Developer mode adds
  the raw "now" temp (amber dashed line on the chart, "now …°" in the card header).
- **Undo/redo covers preset switches, not just curve points** (2026-07-30, Kuba's
  ask — a preset click used to be unrecoverable: it overwrites every curve AND every
  behaviour knob, and it cleared the undo stack on the way through). One stack in
  MainWindow (`Edit`, Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z) carries two entry kinds:
  `CurveEdit` (per-channel point lists) and `TuningEdit`, a pair of
  whole-profile `TuningSnapshot`s. The snapshot type lives in Core next to
  `Profile.CaptureTuning()` / `ApplyTuning()`, and `AdoptTuning(preset)` is
  `ApplyTuning(preset.CaptureTuning())` — so what a preset overwrites and what an
  undo restores are one list that cannot drift apart. It carries curve +
  averaging/hysteresis/hold/slew/floor per channel and the profile name; sensor and
  header assignments (the machine) are deliberately outside it. Notes: record
  equality compares the point Lists by
  reference, so `TuningSnapshot.Matches` does the value comparison — re-clicking the
  preset you are already on must not push an entry; the stacks are no longer cleared
  on preset adoption (undo is LIFO, so a curve edit below a `TuningEdit` is only
  reached once that entry has put its state back); and stepping a `TuningEdit`
  re-captures the live tuning as the opposite stack's snapshot, so behaviour-slider
  edits made after the switch survive the round trip instead of being silently
  reverted (the sliders themselves are still not undoable). Restoring runs
  `OnChannelSelected` so the dev-panel sliders follow, and the engine picks the
  restored values up through the instant-apply fingerprint like any other edit.
- **Curves apply automatically from launch**; preset switches and point edits take
  effect within one engine tick (the engine reads the live Profile objects).
  **Pause (BIOS control)** hands headers back to the BIOS; app exit does too.
- Close button hides to tray by default (persisted `Profile.MinimizeToTrayOnClose`,
  default true since 2026-07-21 — matches Kuba's setting); unchecking the sidebar
  checkbox "Keep running in tray on close" makes X exit (fans back to BIOS). Sidebar checkbox "Start with Windows"
  (persisted `Profile.AutostartEnabled`, default true) registers/removes the Task
  Scheduler task immediately on toggle. Tray tooltip shows live temps→%.
- **Restart cycle without UAC** (Claude can do this for the user): create
  `%AppData%\FanCurves\exit.signal` → the running elevated instance exits cleanly
  within ~1 s (fans to BIOS); `dotnet build`; `schtasks /Run /TN FanCurves` starts
  the new bin exe elevated (hidden in tray — user clicks the tray icon to open).
- **Autostart**: every normal launch registers/refreshes a Task Scheduler logon task
  `FanCurves` (`/RL HIGHEST`, runs `<exe> --tray` = hidden start). A HKCU Run key
  can't elevate, so the task is the only mechanism (old Run entries are cleaned up).
  Dev flows (`--sim` / `--screenshot`) skip registration.
- Config: `%AppData%\FanCurves\profile.json` (auto-saved on every edit).

## Status / open items (2026-07-21)

- **RESOLVED 2026-08-06: the Smart App Control blocker.** SAC flipped to
  enforcement overnight 2026-08-05 and blocked every locally built binary; Kuba
  turned it off (registry `VerifiedAndReputablePolicyState` = 0 confirmed
  2026-08-06) and the app is running again via the scheduled task. If local
  builds ever start failing with 0x800711C7 again, check that key first — SAC
  cannot be re-enabled without an OS reinstall, but Windows can re-evaluate on
  major updates.

- **Defaults track Kuba's hand-tuned profile** ("make the current settings the
  default ones", 2026-07-21 and again 2026-07-22): his profile.json values are the
  `Quiet (MacBook-like)` preset and the app-level defaults (kick off, zero snap
  20%) — see Behaviour contract. Since the 2026-08-06 power removal the Quiet CPU
  staircase adds the v0.1.0 top steps above his 62°C:40% band (the power side used
  to carry sustained load); **his live profile.json is untouched by that change** —
  at the time of the removal it held a hand-made high-duty test curve (90% from
  ~21–24°C avg), presumably part of the chassis-header investigation below.
  One unreconciled 2026-07-21 point remains: his "restart setting button" ask was
  interpreted as the "Start with Windows" checkbox — if he meant an in-app restart
  button, it doesn't exist.
- **Windows 11 Memory Integrity (HVCI) blocks LHM's WinRing0 driver** → no CPU temp
  (Tctl reads 0), no motherboard sensors, no fan headers; only GPU data (via the GPU
  driver) survives. Fix: install **PawnIO** (signed, HVCI-compatible;
  https://pawnio.eu) — LibreHardwareMonitorLib 0.9.7-pre (referenced since
  2026-07-20) uses it automatically. Required on any HVCI-on machine, incl. the
  new X870 build.
- GPU fan controls are filtered out in LhmBackend — this app drives motherboard
  headers only; GPU temps still appear as sensors for case-fan curves.
- **Pump headers are never auto-assigned** (a case curve once slewed a live pump
  toward 0%); the case channel only auto-takes System/Case/Chassis/CHA-named
  headers. Manual dev-mode assignment can still pick anything.
- **A fan header belongs to exactly ONE channel** (2026-07-29). A header obeys a
  single PWM value, so with the same id on two channels `FanEngine.Tick` writes
  both in `Profile.Channels` order and the LAST one silently wins every tick —
  the symptom is a fan that will not turn on, with no why-chip and nothing in the
  UI to explain it (Kuba's Pump Fan header ended up on Case fans too, whose curve
  was flat 0%, so the second NH-D15 fan sat at 0 rpm while the CPU pinned at
  85.3°). Enforced in two places: the dev-panel header checkbox's `Checked`
  handler removes the id from every OTHER channel first (`RebuildSourceChecks` in
  MainWindow — it must NOT rebuild the lists from inside that handler, the panel
  only shows the selected channel and clearing `ControlChecks.Items` there is
  re-entrant), and `AutoAssign` de-duplicates across channels on every launch
  (first channel listed keeps the header) so an existing bad profile heals.
  Sensors are deliberately NOT exclusive — two channels may read one temp.
- Kuba's current test PC: the second CPU fan sits on the board's "Pump Fan"
  header — manually assigned to the CPU cooler channel (lives in profile.json,
  survives because pruning only drops IDs the backend doesn't know).
- **Two low-duty header faults (2026-07-29) — RESOLVED 2026-07-30: BIOS
  per-header drive mode, not the board, not the fans, not LHM.** The Pump Fan
  header dropped its fan at the curve's 20% step (0 ↔ ~370 rpm) and
  `System Fan #1` was dead below ~40% duty; both reproduced under BIOS control
  (never an app bug — full diagnosis journal in `docs/history.md`: swap test,
  per-hour dropout stats, sustain-vs-startup analysis, DC-mode sweep). Root
  cause: headers left on ASRock's **Auto** drive mode, which re-probes
  3-pin vs 4-pin at each boot and can guess DC — in DC mode "20%" is ~2.4 V,
  below the fans' stall voltage, while PWM pulses full 12 V. Fix applied in the
  BIOS that night: **every header set explicitly to PWM, never Auto** (the
  water-pump header has its own menu entry — easy to miss in a "set everything
  to PWM" pass). Verified over the following morning's telemetry: Pump header
  0% dropouts in 28 336 samples at 20% duty (was 64–98% the previous
  afternoon), all four chassis headers statistically identical at every duty.
  Enduring rules: (a) if a low-duty fault reappears, suspect a header reverted
  to Auto/DC first — falsifiable check: at 27% from a stop all four case fans
  break away to ~350 rpm within ~15 s; only if one refuses while its siblings
  run suspect the board. (b) From a dead stop the P14s need ~27% duty and up
  to ~15 s to break away, but once spinning they sustain down to ~5% — a curve
  whose bottom band is below break-away only keeps fans turning if they were
  already turning. (c) If the chassis headers ever run DC again: slowest
  sustainable point ~28% (~600 rpm; enforce with zero snap 28–30), and
  `rpm_per_header` reads garbage (>2500 rpm) below break-away. (d)
  Cross-session `rpm_per_header` comparisons: LHM's `System Fan #N` numbering
  does NOT map onto ASRock's CHA_FAN labels, and the `ControlIds` order (= the
  column order) changes when AutoAssign re-assigns — split analysis per app
  session.
- **OPEN (2026-07-30 afternoon): chassis-group kill events + the low-duty
  break-away trap.** Distinct from the resolved sustain fault. With a test
  curve holding the case fans at 5–10% duty, every ~30–45 min ALL FOUR chassis
  headers cut out within one tick (~475 rpm → 0 with no coast-down; garbage
  tach reads — 907, 10 810 — exactly at the transition) at CONSTANT commanded
  duty. Kills observed 12:47:42, 13:19:42, 13:47:39, 14:31:05, 15:04:34. The
  CPU headers on the same NCT6686D, written identically every tick, show zero
  anomalies in 96k samples over the same hours; behavior.txt and the Windows
  System log are silent at those instants; no other monitoring software
  running. The same signature exists in the 2026-07-29 telemetry (pre-PWM-fix),
  including one kill at 53% duty that self-healed within seconds — so the kill
  pulses likely fire at every duty and only become visible outages below
  break-away (~27%), where a stalled P14 needs stochastic minutes to restart
  (measured at 10%: 2–7.5 min; at 5%: never). Emergent limit cycle follows:
  kill → stall → case temp climbs → curve steps 5→10% → break-away minutes
  later → cools → back to 5% → next kill ("once working, once not"). Prime
  suspect: the ASRock EC periodically re-asserting its own chassis-group fan
  config — a re-assert on the CPU group would command ≈ the app's own duty and
  stay invisible, which explains the group asymmetry. Diagnostic + likely fix:
  set the BIOS chassis fan curve to a constant moderate duty (~50–60%), so a
  re-assert pulse spins the fans UP for a second (visible spike, self-rescuing)
  instead of down to 0 — if the outages become 1 s rpm spikes, the theory is
  proven. App-side hardening either way: keep case bands out of the 1–27% trap
  zone (zero snap ~28–30; OFF in the current test profile) — 0% or ≥28% means
  a kill either costs nothing or self-heals in seconds.
  **Update 2026-08-02 (three more days of telemetry, 07-30→08-02):** the kills
  persist every ~5–15 min whenever the case band sits at 10–20% (42 group stops
  on 07-31, 24 on 08-01; at 20% duty the fans stood still for 53–90% of all
  samples), CPU headers still spotless across all four days — and the
  "≥28% self-heals in seconds" rule is now DISPROVEN: on 08-02 15:55–16:25+,
  headers control/3 + control/4 sat at 0 rpm at a commanded 37% for 30+ minutes
  while `FanEngine` rewrote duty to all four headers every tick (unconditional
  `SetControl` loop) and control/2 + control/5 ran ~985 rpm alongside. So the
  override can LATCH individual chassis channels for tens of minutes, not just
  pulse the group; which pair is affected varies between events. The tach-freeze
  alternative was ELIMINATED by eye 2026-08-02 ~16:30: during the latched
  episode Kuba saw two chassis fans spinning, one fully still and one
  repeatedly twitching — jerking slightly and falling back — i.e. the fans are
  physically stalled and the twitcher is getting brief drive pulses that are
  immediately cut, consistent with the app's 1 Hz duty write landing for a
  fraction of a second before the EC re-zeros the channel (a write-fight the
  app loses). Note this also caps what the stall-rescue feature can promise:
  a 100% rescue pulse fights the same fight. SCEWIN nvram dumps CANNOT audit any of this — ASRock's H/W
  Monitor per-header config is not in the AMI setup NVRAM (the whole 391 KB
  export contains only 3 AMD-CBS fan items). Next steps unchanged (BIOS
  chassis curve at constant 50–60% as the EC-re-assert litmus test) plus a
  candidate app feature: per-header stall rescue — commanded >0 with tach 0
  for N s → pulse that one header high for a few seconds, then restore.
  **Tach floor (2026-08-03, verified by eye):** the NCT6686D reads 0 rpm below
  roughly ~300 rpm — at 10% duty both NH-D15 fans visibly spin (~250–280 rpm)
  while the app and `rpm_per_header` show 0. A zero read is only proof of a
  stopped fan when the expected speed is comfortably above ~300 rpm; below
  that, telemetry cannot distinguish slow-spinning from stopped (matters for
  any stall-rescue feature and for kill-event analysis at very low duties).
  Case fans replaced 2026-08-03 with 4× used Noctua NF-A14 PWM (OLX, 270 zł
  set) — min ~300 rpm, documented start at any duty >0%, no fan-stop trap;
  the Arctic P14 Pros (27% break-away) are retired from the chassis headers.
  **Update 2026-08-02 evening (after Kuba's new BIOS settings, reboot 21:27):
  the override is active FROM BOOT and its release correlates with the case
  temp crossing 50 °C.** From 21:31:33 the app commanded a constant 27% and all
  four chassis fans sat at 0 rpm for a full 30 min — the same 27% that
  breaks these fans away from a dead stop in ~15 s when the headers are free.
  At 22:01:22 the raw case temp touched 50.0° (curve dipped the duty
  27→10→27 over the next 40 s) and at 22:02:04 THREE of the four headers
  released simultaneously (0 → ~630 rpm within 2 s at the same 27%);
  control/4 stayed in the fight — for the rest of the 22h hour it read 0 in
  37% of samples and only ~390 rpm (vs siblings' ~630) when turning, i.e. the
  visible "twitcher": drive pulses landing between EC re-zeros. Same-day
  earlier sessions fit the same picture: h16–h17 had control/4+3 pinned at 0
  through TWO reboots (17:27, 17:41) and every duty change, while case temps
  stayed below ~50°. Working hypothesis sharpened: the EC enforces the BIOS
  chassis-fan state — including its own fan-stop/zero region below a ~50 °C
  board temp — against the app's writes; the group releases when the EC's own
  temp source crosses its threshold and the EC starts commanding nonzero
  itself. Duty-change dislodging is the weaker alternative (contradicted by
  h17: reboots + ramps didn't free control/4+3). Consequence: BIOS chassis
  settings must have NO fan-stop / 0% / "silent" low-temp region — a constant
  50–60% chassis curve (the standing litmus test) makes an EC win spin fans
  UP instead of down, and per-header "Allow Fan Stop"-style toggles must be
  off. CPU channel remains spotless all day, incl. the Pump header at 20%
  (~370 rpm steady — the 07-29 fault stays fixed).
  **Continuation 22:30–22:35 (this session's telemetry check):** Kuba's curve
  edit landed 22:30:48–55 (dropped the 50:10 and 58:20 points, bumped
  37:27 → 37:28); within seconds ALL FOUR headers jumped ~640 → ~725 rpm
  (+~90 rpm for a 1-point duty change — more than PWM scaling explains,
  consistent with an EC state flip at the same moment) and control/4's
  0↔~300 rpm cycling STOPPED: continuous clean spin from 22:31:00 (last
  stall 22:30:44), still ~715 rpm at 22:35 with the case temp back at 43° —
  well BELOW the ~50° release point, and the EC zero-enforcement did not
  re-engage. So either the EC's own curve has wide hysteresis / releases
  latch-like once tripped, or the 50° correlation was coincidental with the
  duty wiggle. Watch item: if control/4 resumes cycling at 28% duty while
  the case temp stays in the 40s, the temp-threshold theory is out and
  per-header hardware (fan or header on control/4 — also half of the
  afternoon latched pair) moves up; distinguishing test remains the July
  method: physically swap the fans on control/4 and control/2 and see
  whether the cycling follows the header or the fan. Not DC mode: only 9
  garbage tach reads (>2500 rpm) in 13 199 Case rows all day — rule (c)'s
  DC signature is absent.
- Real hardware wins over simulation whenever it reads any temp sensor, even with
  zero controllable headers (channels then show "[no fan header]").
- Diagnostics in `%AppData%\FanCurves\`: `sensors.txt` (all sensors + values at
  launch and +15 s), `events.txt` (startup/backend/crash/exit log), and since
  2026-07-27 the **review log** in `logs\` (`TelemetryLog.cs` in Core, hooked to
  `engine.Ticked` in App on the engine thread so it records even if the UI faults;
  Kuba's ask "log the data and the behavior so I can explain which parts I don't
  like"): `telemetry-YYYY-MM-DD.csv` — one row per channel per tick with every
  controller input/output (raw/avg temp, out/target %, why-chip reason + its
  level/seconds), daily rotation, 7 days kept. The `rpm` column is only
  `ControlIds[0]`; the trailing
  **`rpm_per_header`** column (`ChannelStatus.Rpms`, added 2026-07-29) carries
  EVERY assigned header's rpm `|`-joined in `ControlIds` order — a channel driving
  six headers can have one fan stalled or spinning up far slower than its
  siblings, and the single-header column made that invisible (it also read 0 for a
  whole day once because `ControlIds[0]` happened to be an empty header). The
  header line is written when the day's file is created, so a schema change needs
  the day's CSV deleted, not just a restart (done 2026-08-06 for the power-removal
  schema).
  Also written: `behavior.txt` (4 MB cap → `behavior-old.txt`) with
  CHANGES only: fan ON/OFF, target steps, reason transitions (pure
  None↔Ramp flips suppressed — the target line already implies the ramp),
  driving/released flips, `·· <event>` markers (`App.Telemetry?.Event`), and a
  full settings line whenever any tuning knob or curve point changes (snapshot
  diffed every tick — catches presets/sliders/edits with no per-handler wiring).
  Dev flows write
  `telemetry-sim-*.csv` / `behavior-sim.txt` (same rule as sensors.sim.txt).
  UTF-8 **with BOM** — Windows PowerShell 5.1 reads BOM-less UTF-8 as ANSI and
  mangles °/·/∞. Writers buffer (CSV flushed every 5 s, behavior per event);
  first failed write disables logging for the session — it must never take the
  engine down. Switchable since 2026-07-29: `Profile.TelemetryLoggingEnabled`
  (default true, dev-panel REVIEW LOGGING above BACKEND) gates the `engine.Ticked`
  hook in `App`; the toggle writes a `·· review logging on/off` marker and calls
  `TelemetryLog.Flush()` when switching off, so the ≤5 s of buffered CSV
  rows are not stranded until process exit.
- **Installed & verified on real hardware** (the X870 Steel Legend / 9950X3D
  build): exe at `%LOCALAPPDATA%\Programs\FanCurves\FanCurves.exe`, autostart
  task registered, PawnIO 2.2.0 (silent flags are `-install -silent`, dash-style —
  `/S` fails with exit 87). LhmBackend sees the NCT6686D: 8 controllable headers +
  all board temps; CPU temp from `/amdcpu/0/temperature/2` (Tctl/Tdie). The
  "Pump Fan" header (`/lpc/nct6686d/0/control/1`, physically the second NH-D15
  fan) is manually assigned to the CPU cooler channel.
