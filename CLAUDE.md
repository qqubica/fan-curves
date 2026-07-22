# fan-curves — Windows fan control with a MacBook-like default

WPF (.NET 8) app that drives motherboard fan headers the way a MacBook manages its
fans: silent flat floor at idle, no reaction to short temperature spikes, smooth
ramp only under sustained load (class-level default slew 8 %/s both directions; the Quiet preset's CPU channel runs 9 up / 8 down since 2026-07-22). Built for Kuba's 2026 PC
(ASRock X870 Steel Legend WiFi + 9950X3D + NH-D15 + Arctic P14 Pro chain in a
Phanteks P600S), but hardware-agnostic.

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

## Layout

- `src/FanCurves.Core` — engine, no UI deps:
  - `FanCurve` — STAIRCASE curve: each point opens a flat band (at ≥ TempC run at
    Percent until the next point). No interpolation between points.
  - `ResponseFilter` — **the MacBook feel lives here, not in the curve shape**:
    the curve is driven by the rolling AVERAGE temp over AveragingSeconds (default
    20 s — spikes barely move it), the target may jump multiple steps at once,
    step-down needs (avg + HysteresisC) to map lower AND that condition to hold
    continuously for StepDownHoldSeconds (default 10 s; timer resets if the average
    pops back up) — no flapping at band edges,
    slew-rate limit (%/s, default 8 up and 8 down) for gradual audible ramps.
  - `FanEngine` — ~1 s tick, jittered (2026-07-22: each tick re-arms a one-shot timer
    with a random 850–1150 ms delay so sampling never phase-locks onto periodic system
    activity; safe because all time logic — averaging window, step-down hold, slew dt,
    idle kick — runs on the monotonic clock, not tick counts): read temps (max of
    assigned sensors) → filter → write PWM.
    `StopApplying()`/`Dispose()` hand every header back to the BIOS (`Control.SetDefault`).
  - `LhmBackend` — real hardware via LibreHardwareMonitorLib (needs admin for the
    kernel driver / Super I/O access). `SimulatedBackend` — fake AM5-ish CPU with
    spikes + load phases, used automatically when not elevated or nothing controllable.
- `src/FanCurves` — WPF UI, **two modes**. Under the curve chart sits a **history
  strip** (`HistoryChart.cs`, added 2026-07-22; **Developer mode only** since later
  that day — collapsed in simple mode, the curve chart takes the freed space): the
  last 10 min of the selected channel's rolling-average temp (bright trace), raw
  "now" temp (faint dashed trace) and commanded fan % (dim trace + under-fill), one
  sample per engine tick in a per-channel ring buffer (`ChannelHistory`, fed from
  `MainWindow.OnEngineTicked` in both modes so history is already populated when
  dev mode opens; not persisted). Right edge = now (amber live dots — the only
  amber there); hover shows a crosshair with a `time · avg · now · %` readout chip.
  Fan turn-ON events (commanded % leaves 0) get a baseline tick, turn-OFF events a
  dimmer one, and the m:ss stopped time (turn-OFF → next turn-ON; was start-to-start
  until Kuba's correction later that day) is written on a dim span between the ticks
  (label seats on the card background so narrow gaps stay readable; a label is
  skipped only when it would overlap the previous span's label — an earlier
  gap-width guard silently dropped stops under ~1 min at dev-mode width, fixed
  after Kuba's report later that day; an ongoing stop draws a live span counting
  up to "now"; no span when the stop predates the 10-min window) — added
  2026-07-22 to make the idle-kick / zero-snap cycling cadence readable.
  Left/right padding matches `CurveEditor` so the two plots align.
  **Why-chip (2026-07-22)**: notification-style chip in the curve chart's top-left
  corner (`WhyChip` in MainWindow.xaml, both modes) explaining why the commanded %
  differs from the curve's configured level; hidden when they match. The engine
  classifies every tick (`OutputReason` in `FanEngine.cs`, most-specific-wins chain
  fed by diagnostics `ResponseFilter` now exposes — CurveLevel/TargetLevel/
  PendingDownLevel/DownHoldRemaining/HysteresisHolding/SnappedToZero): slew ramp
  up/down (with target %), step-down hold (pending level + countdown), hysteresis,
  zero snap (names the sub-threshold curve %), safety floor, idle kick, stop probe.
  Classification verified by a scratchpad console harness (ramp/hold/hysteresis/
  snap/floor scenarios) on 2026-07-22.
  - Simple (default): two preset buttons (`Quiet · MacBook-like` — default,
    `Performance`), the two fan channels, read-only curve illustration with live
    overlay, Apply/Stop. No settings visible.
  - Developer (top-bar toggle, or `--dev`): curve editing (drag points, double-click
    add, right-click remove; edits snap to whole °C / whole %, bands stay ≥1 °C wide,
    max 12 points per channel; Ctrl+Z / Ctrl+Y — also Ctrl+Shift+Z — undo/redo point
    edits, history in MainWindow via before/after snapshots, cleared on preset
    adoption), behaviour sliders (averaging window covers 0–5 min on a
    non-linear slider — its Value is a notch index: 5 s steps up to 2 min, 30 s steps
    beyond; mapping in `MainWindow.AvgNotchToSeconds`), sensor/header assignment, backend +
    config-path info. Any manual tweak marks the profile "Custom" (preset highlight
    clears). Presets overwrite tuning but keep sensor/header assignments
    (`Profile.AdoptTuning`).
  Dark theme, code-behind (no MVVM framework).
  **Design language = "precision instrument"** (2026-07-20 redesign; grew out of the
  speed-pill lookbook after "looks like 2012" feedback): borderless window with custom
  chrome (`WindowChrome`, DWM rounded corners + dark frame via `Chrome.Apply` in
  `Ui.cs`, custom caption buttons, title-bar fan glyph that spins while applying).
  **Exactly three window sizes** (Kuba's choice 2026-07-20): the fixed 1010×660 floating
  window, quarter-of-screen (half work-area width × height, snapped to the nearest
  screen corner), and maximized — cycled in that order by the maximize caption button
  (its glyph previews the next size); drag-resize disabled
  (`ResizeMode=CanMinimize`, `ResizeBorderThickness=0`), and a `WM_GETMINMAXINFO` hook
  in `Chrome` clamps maximize to the work area (borderless windows otherwise cover the
  taskbar). Further elements: layered near-black surfaces (canvas `#0a0a0d`, cards `#111116` with light-from-above
  gradient hairline + drop shadow), monochrome white at graded opacities, and ONE
  accent — warm amber `#FF9E5E` — reserved strictly for live thermal state (chart
  operating dot + crosshair with axis readout chips, status-chip dot). Hero = large
  Segoe UI Variable Display Light numeral of the selected channel's rolling average;
  micro-labels letter-spaced via `Tracked.Text` (hair spaces, `Ui.cs`); numerals in
  Cascadia Mono with invariant "." decimals. Channels switch via a segmented control
  (restyled ListBox) in the chart-card header; presets are selectable cards; global
  state lives in a status chip under the hero (no status bar). Chart: sparse horizontal
  grid only, glow-under white staircase, dashed white vertical = raw temp (dev mode).
  Keep everything else monochrome — the amber stays live-data-only.

## Build / run

```
dotnet build                                # .NET 8 SDK (installed via winget 2026-07-20)
dotnet run --project src/FanCurves          # real hardware if elevated, else simulation
dotnet run --project src/FanCurves -- --sim # force simulation
dotnet run --project src/FanCurves -- --sim --screenshot out.png  # UI verify aid (4 s, then exits)
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
  default — first 2026-07-21, re-promoted from his live profile 2026-07-22**):
  **all fans fully stopped at idle** (0% floor, 0% bottom step): CPU cooler 0%
  through <57°C avg, then 20/40/50/65/81/90/100% at 57/62/70/76/84/88/92°C, with
  a **90 s averaging window**, 1.5°C hysteresis, 25 s step-down hold, slew 9 %/s
  up / 8 down; case fans 0% below 58°C avg, then 25/40/55/70%, 25 s averaging,
  slew 7 %/s — Arctic P14 Pro stops below 5% PWM.
- **Stopped-fan kick**: any channel the engine is actually driving that has sat at
  0% output for `IdleKickStoppedSeconds` gets spun to `IdleKickPercent` for
  `IdleKickSeconds`, then stops again (repeats after each stillness period). Curve
  demand >0% cancels the kick and resets the clock.
  `IdleKick` in Core (per-channel state in `FanEngine.Tick`, bypasses the slew
  filter); not active while paused/BIOS-controlled. Configurable in the dev panel
  (all global/app-level like the tray/autostart toggles — don't mark the profile
  "Custom", presets don't touch them): checkbox "Stopped-fan kick (all channels)"
  (`Profile.IdleKickEnabled`, **default false since 2026-07-22** — Kuba runs with
  the kick off; was default-on at 60 s/20%/20 s through 2026-07-21) plus three
  sliders — idle time before
  kick (`IdleKickStoppedSeconds`, 10–300 s, default 10), kick speed
  (`IdleKickPercent`, 5–100 %, default 5), kick duration (`IdleKickSeconds`,
  5–60 s, default 5); engine pushes them into
  each `IdleKick` every tick. Sliders stay editable while the kick is disabled
  (Kuba 2026-07-21: settings must be reachable with the checkbox off). Gotcha: slider `ValueChanged` fires mid-XAML-parse
  (setting `Minimum`), so `OnKickParamChanged` bails while `KickTimeValue == null` —
  removing that guard crashes the app at startup.
- **Zero snap (stop instead of running slow, added 2026-07-22)**: any filter target
  above 0% but strictly below the threshold (default 20% since later that day;
  was 30%) runs the fan at 0% —
  meaningful speed or fully stopped, never a slow crawl. Implemented in
  `ResponseFilter` (`ZeroSnapPercent`, snap applied to BOTH curve evaluations so
  hysteresis/step-down hold reason about the snapped level; slew still glides
  to/from real steps, so ramps pass through the low range transiently — that's
  intended). Channel `MinPercent` is applied after the filter and still wins
  (Performance CPU keeps its 30% floor); a snapped-to-0 channel counts as stopped
  for the idle kick. App-level like the kick (doesn't mark "Custom", presets don't
  touch it): `Profile.ZeroSnapEnabled` (default true) + `Profile.ZeroSnapPercent`
  (default 20), dev-panel checkbox "Stop fans instead of running slow (all
  channels)" + "Stop below" slider (1–50%). With the 20% threshold and the
  2026-07-22 default curves, no band is snapped (the snap is strictly-below, so
  the CPU's 20% band runs at 20%) — the feature only bites when a curve/threshold
  edit puts a band under the threshold.
- **Stop probe (trial-stop steadily running fans, added 2026-07-22)**: once a driven
  channel has run continuously for `StopProbeRunSeconds` (default 30) with its RAW
  temp inside a `StopProbeStableRangeC` band (default 3.5°C since 2026-07-22's
  defaults re-promotion; was 2°C), the fan is stopped as a trial. While stopped, the 5 s-average temp is compared to the pre-stop baseline
  every tick (a superset of Kuba's "recheck every 30 s" ask — continuous is safer):
  rise beyond the band, or curve demand above the level at stop → immediate resume.
  A stop that dies within `ProbeSeconds + RunSeconds` (~60 s at the defaults —
  trial length `StopProbeSeconds` default 30 s since the re-promotion, was 10)
  counts as FAILED and
  arms a `StopProbeRetrySeconds` backoff (default 60 s since the re-promotion,
  was 300) so a loaded fan never
  settles into an on/off cycle; a later rise resumes without backoff. `StopProbe`
  in Core (per-channel state in `FanEngine.Tick`, runs BEFORE the idle kick →
  probe-stopped counts as stopped for the kick, same choice as zero snap); channels
  with `MinPercent > 0` are never probed (safety floor wins — Performance CPU).
  App-level like kick/zero-snap (doesn't mark "Custom", presets don't touch it):
  `Profile.StopProbeEnabled` (**default true**) + the four params, dev-panel
  checkbox "Trial-stop fans when temps are stable (all channels)" + sliders
  "Steady running before trial" (10–300 s), "Trial stop length" (5–60 s), "Stable
  band · rise to resume" (0.5–5°C), "Retry after failed trial" (60–900 s).
  State-machine scenarios verified by a scratchpad console harness on 2026-07-22
  (probe timing, in-trial rise + backoff, late rise without backoff, unstable
  temps never probe).
- Changing ChannelConfig field names breaks saved `%AppData%\FanCurves\profile.json`
  (old fields silently ignored, defaults kick in) — delete it after schema changes.
- Sensor/control IDs are backend-specific; `AutoAssign` prunes IDs the current backend
  doesn't know and re-assigns empty channels on every launch (a profile saved in `--sim`
  works on real hardware and vice versa).
- Temperature display: simple mode shows only the rolling average (the thing that
  actually drives the steps) — hero numeral + segment readouts; Developer mode adds
  the raw "now" temp (white dashed line on the chart, "now …°" in the card header).
- **Curves apply automatically from launch**; preset switches and point edits take
  effect within one engine tick (the engine reads the live Profile objects).
  **Pause (BIOS control)** hands headers back to the BIOS; app exit does too.
- Close button hides to tray by default (persisted `Profile.MinimizeToTrayOnClose`,
  default true since 2026-07-21 — matches Kuba's setting); unchecking the sidebar
  checkbox "Keep running in tray on close" makes X exit (fans back to BIOS). Sidebar checkbox "Start with Windows"
  (persisted `Profile.AutostartEnabled`, default true) registers/removes the Task
  Scheduler task immediately on toggle. Tray tooltip shows live temps→%. (NotifyIcon is WinForms —
  csproj removes the WinForms global usings to avoid Point/Application ambiguity.)
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

- **Defaults track Kuba's hand-tuned profile** ("make the current settings the
  default ones", 2026-07-21 and again 2026-07-22): his profile.json values are the
  `Quiet (MacBook-like)` preset and the app-level defaults (kick off, zero snap
  20%) — see Behaviour contract. The old idle-kick "60 s vs at least 30 s" open
  point is moot (kick now default-off). One unreconciled 2026-07-21 point remains:
  his "restart setting button" ask was interpreted as the "Start with Windows"
  checkbox — if he meant an in-app restart button, it doesn't exist.
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
- Kuba's current test PC: the second CPU fan sits on the board's "Pump Fan"
  header — manually assigned to the CPU cooler channel (lives in profile.json,
  survives because pruning only drops IDs the backend doesn't know).
- Real hardware wins over simulation whenever it reads any temp sensor, even with
  zero controllable headers (channels then show "[no fan header]").
- Diagnostics in `%AppData%\FanCurves\`: `sensors.txt` (all sensors + values at
  launch and +15 s), `events.txt` (startup/backend/crash/exit log).
- Fully working in simulation; **real-hardware fan control still untested** — needs
  the assembled X870 Steel Legend PC: verify Nuvoton Super I/O controls appear (via
  PawnIO) and that Fan #N ↔ Fan Control #N pairing holds on this board.
