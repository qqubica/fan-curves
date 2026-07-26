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
  `gh release create` makes the tag remotely — run `git fetch --tags` right
  after, so the tag exists locally (git-solo and other history tools only see
  local refs; the v0.1.0 tag was missed this way once).

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
  - `PowerBudget.cs` — `ThermalModel` (learned C / R(fan%) / base, persisted per
    channel; `Reset()` returns it to the seeds, `Resistances` exposes the anchors
    for display) + `PowerBudgetController` (power-driven control with the heatsink
    as energy credit + the hard-override fuse). Since 2026-07-25 the controller
    holds **no hard-coded tuning constants** — ceiling/aim margins, trend, slope
    and live-draw windows, fuse release and the learning switch are all settable
    properties the engine feeds from the profile each tick — see the
    thermal-budget entry in the behaviour contract.
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
  Since 2026-07-25 it also draws the **budget ceiling** as a dotted reference on the
  temp scale (power-controlled channels only), and since 2026-07-27 the **sustained
  aim** as a dimmer dotted line — the aim label seats BELOW its line (the ceiling's
  sits above) so the two never collide when the margins put them a few pixels apart.
  Left/right padding matches `CurveEditor` so the two plots align.
  **Budget strip** (`BudgetChart.cs`, 2026-07-25, dev mode only, sits under the history
  strip): the same 10-min window seen from the thermal-budget controller's side —
  instantaneous power draw (faint dashed) and its sustained average (bright trace +
  under-fill) on a **watts** scale that auto-ranges to the window's own peak
  (`NiceWatts` ladder), against the predicted headroom `TauSeconds` (quiet line, right
  scale in **seconds**) and a dotted `ramp lead` line = the threshold that triggers a
  step up. Since 2026-07-27 the headroom is measured to the sustained aim (behaviour
  contract): it drains under a sustained fans-off load and recovers only on a fan
  step or load end, instead of pinning at ∞ near any under-ceiling equilibrium.
  The headroom axis is **logarithmic, 10 s → 30 min** (2026-07-26, after
  Kuba's "headroom is not decreasing while the avg temp rises": the old linear axis
  capped at 3× the lead drew "3 hours", "20 min and falling" and "∞" as the same flat
  line at the top — a slow warm-up's drain from hours to minutes was invisible).
  Top of scale = ≥30 min/∞ (the chip prints ∞ there), refs at 1:00 and 10:00, the
  trigger line sits in the lower third so a genuine dive is a long visible descent. Hard-override spans are shaded with a "fuse" label; hover chip reads
  `time · draw · avg · buffer kJ · headroom · needs %` (same vocabulary as the dev
  panel readout). No under-fill under the headroom trace — a healthy buffer pins it to
  the top of the scale and a fill would flood the strip (tried it, looked like a solid
  block). Channels without power sensors get a centred "no power sensor on this
  channel" note instead of empty axes. Both strips share `StripChart.cs` (padding,
  title, legend, time axis, hover crosshair/chip, trace + under-fill helpers) and both
  read the same per-channel `ChannelHistory` ring, whose `HistorySample` carries the
  budget telemetry (watts, avg, credit, tau, demand, ceiling, aim, override flag).
  Dev mode's fixed window grew to **1320×830** (from 1010×660) to fit the second strip
  without squeezing the curve editor; `EnterFixed` clamps the height to the work area
  on short screens. Hover chips take several wordings and draw the widest that fits the
  plot (`DrawChip(dc, x, wide, narrow)`), so quarter-screen windows get a two-line chip
  instead of one running off the edge — `FormattedText` honours `\n`. Reference labels
  (ceiling, ramp lead) are drawn last and seated on the card colour, otherwise the
  spiky draw trace runs straight through them.
  Verified by a scratchpad WPF harness that renders the strips to PNG from scripted
  histories (fuse span, hover chip, empty state, 420 px-wide window) — it pokes
  `StripChart._hoverX` by reflection, because `SetCursorPos` cannot hover a window that
  another window covers, and a 4 s `--sim` capture never reaches those states.
  **Render robustness (2026-07-27, from Kuba's resize crash)**: a mid-resize arrange
  pass handed `CurveEditor` a near-zero size and its axis-chip `Math.Clamp` threw on
  an inverted range (min 16 > max 13.8 — ArgumentException), killing the process —
  which left the Super I/O frozen at the last written PWM with nothing watching the
  die while a load pushed it through 80°. Now every chart render bails out below a
  minimum plot size (`StripChart.TooSmallToRender` / CurveEditor's inline check),
  every computed-range label clamp uses the `Math.Max(lo, hi)` guard, and
  `App.DispatcherUnhandledException` logs (throttled to one entry / 5 s) and
  **swallows** UI faults instead of dying — fan control must outlive the UI.
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
    beyond; mapping in `MainWindow.AvgNotchToSeconds`), sensor/header assignment (since
    2026-07-24 each sensor row leads with its live temp and each fan-header row with its
    live rpm — value column left-aligned so the reading hugs the checkbox
    (was right-aligned through 2026-07-24; Kuba wanted the checkbox-to-value gap
    smaller), refreshed per engine tick via
    `RefreshSourceReadouts`, "—" when unreadable; the whole row — value and name —
    renders in Cascadia Mono 11.5 after Kuba's "use uniform font" the same day; since
    later that day long names **wrap** onto extra lines so the full name is always
    visible — the lists' horizontal scrollers are gone, and the shared CheckBox
    template's root is a Grid with a star content column because its old horizontal
    StackPanel measured content at infinite width and silently disabled wrapping), backend + config-path info. Any manual tweak marks the profile "Custom" (preset highlight
    clears). Presets overwrite tuning but keep sensor/header assignments
    (`Profile.AdoptTuning`).
  Dark theme, code-behind (no MVVM framework).
  **Design language = "precision instrument"** (2026-07-20 redesign; grew out of the
  speed-pill lookbook after "looks like 2012" feedback): borderless window with custom
  chrome (`WindowChrome`, DWM rounded corners + dark frame via `Chrome.Apply` in
  `Ui.cs`, custom caption buttons, title-bar fan glyph that spins while applying).
  **Exactly three window sizes** (Kuba's choice 2026-07-20): the fixed floating window
  (1010×660, 1320×830 in dev mode), quarter-of-screen (half work-area width × height, snapped to the nearest
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
dotnet run --project src/FanCurves -- --sim --screenshot out.png      # UI verify aid (4 s, then exits)
dotnet run --project src/FanCurves -- --sim --screenshot out.png 330  # …after 330 s, so the strips have history
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
- **Thermal-budget control ("control with power, not temperature", added 2026-07-24)**:
  channels with assigned power sensors (`ChannelConfig.PowerSensorIds`, watts summed;
  AutoAssign wires CPU package power to the CPU channel, CPU+GPU to the case channel)
  are driven by `PowerBudgetController` in `PowerBudget.cs` instead of `ResponseFilter`.
  Philosophy: the 60 s power average is the real dissipation demand; the heatsink's
  thermal mass is credit — `E = C·(ceiling − trendTemp)` (trend = 30 s avg, filters the
  die's instant jump). Fan steps UP only when the predicted **headroom** drops under
  `Profile.RampLeadSeconds` (default 45; Kuba runs 90), directly to the lowest
  curve-ladder level whose predicted equilibrium (`base + R(level)·PowerNow`) holds
  the **sustained aim**. **Headroom redefined 2026-07-27** (Kuba: "headroom is almost
  always at 100% no matter what the temperature or the fan speed; it should almost
  only go down — up without fans only when hot with no load"): `TauSeconds` = predicted
  seconds until the sink trend crosses the **guarded line — the sustained aim while
  below it, the ceiling once past it** — min of a model prong (exact first-order time
  `R·C·ln((eq−T)/(eq−line))`, finite whenever the equilibrium `base+R(output)·PowerAvg`
  clears the line by ≥ HysteresisC) and a measured prong (`(line−trend)/slope`). The
  old pair (`E/surplus` + slope-to-CEILING) read ∞ at any equilibrium under the
  ceiling — on this cooler, nearly every real state — and even recovered to ∞
  mid-warm-up as the climb decelerated toward a hotter-than-wanted equilibrium with
  the fans still parked; now headroom recovers only when the fans step up or the load
  ends. **Prong gating (redesigned later on 2026-07-27 after Kuba's "temperature
  rising and rising through 80° and the fans not kicking in")**: the MODEL prong
  runs UNGATED — it reads only sustained quantities (PowerAvg, trend) and its
  aim-clearance margin (equilibrium must clear the line by ≥ HysteresisC) absorbs
  power-window aliasing. The first cut gated BOTH prongs on burst-quietness, which
  silenced the whole predictive layer under a real game: a live draw never stops
  fluctuating (peak−avg ≈ 55 W for the entire session), the gate never opened,
  headroom sat pinned at ∞ and only reactive StepUpHold caught the climb — late,
  above the aim (harness S7 reproduces this with a realistic swinging load). The
  SLOPE prong keeps the strict peak gate (no burst above `max(10 W, 25%)` over
  PowerAvg within the last SlopeWindow+TrendAvg seconds) — its contamination is
  physical: an 8 s spike's die jump reads as a rush toward the aim for as long as
  the fit can see it (harness S3). LEARNING gets the longest quiet horizon
  (slope+trend+power windows ≈ 115 s at defaults): the 250 W spike train poisoned
  R(0) to ~0.78 through 30 s-"stable" windows by pairing the still-elevated trend
  with trough power, and a poisoned R is what turns the ungated model prong into a
  false accuser — with honest learning, the margin alone keeps spike trains silent
  (150 W and 250 W verified: fans never move, min tau 632 s). The wall-jump cost
  stays: a 188 W step is spike-indistinguishable early on, fires at ~68° trend and
  peaks 80° die — the fuse stays the hard stop. **One ramp step per slope
  window (2026-07-26, from Kuba's "fans ramped instantly to 100%")**: the measured
  slope keeping tau low is backward-looking and knows nothing of the step just taken,
  so re-firing every tick used to climb the whole ladder in seconds; within the
  settle window a further step needs the chosen level to have become objectively
  insufficient (draw rose; its predicted equilibrium no longer holds the aim). A ramp
  step taken with the gate open **brands** the level it left exactly like a
  StepUpHold step (an optimistically-wrong model otherwise argues the fan straight
  back down after every predictive step — slow on/off hunt, harness S5); gate-closed
  (model-only) fires don't brand — no measured corroboration yet.
  Steps DOWN one ladder level
  per StepDownHoldSeconds once the power average no longer needs the current one —
  after a load ends this reacts minutes before the cooling temp average would — and
  (2026-07-27) only onto a level predicted to hold the aim even at **5/4 of today's
  sustained draw**: hysteresis in the DRAW dimension, where the swings actually
  live. A ±HysteresisC temperature band alone is worth ~4 W at a 20% fan level —
  a constant load equilibrating exactly AT the aim hunted 20↔40% across it, and a
  game's minute-scale swells re-crossed it constantly (harness S7). Deliberately
  NOT stacked with the temperature band: the double margin blocked the final step
  to silence at warm idle (harness S5b). `ChannelConfig.HysteresisC` still feeds
  the controller — as the model prong's aim-clearance margin.
  **The sustained aim is also enforced UPWARD (2026-07-26, from Kuba's report
  "headroom dips briefly, recovers to max, fans never kick in")**: even the
  aim-referenced tau goes blind at a settled equilibrium that a frozen or unlearned
  model believes is under the aim (model prong claims sustainable, slope reads 0) —
  which used to park a 90 W game at ~84° die with fans stopped forever, and a 188 W
  load at 86.1°, above its own ceiling, showing "buffer 0.0 kJ · headroom ∞". So: trend
  past the steady aim AND (power average demands a higher ladder level OR the temp
  has settled there, slope > −0.002 — the model-free prong catches frozen/unlearned
  models) → one ladder step up per StepDownHoldSeconds (why-chip reason
  `StepUpHold`). Each step up brands the level it left as measured-insufficient at
  that PowerAvg — DemandLevel is clamped above the branded level so the model can't
  argue the fan straight back down into a slow on/off limit cycle; the brand lifts
  once the sustained draw has stayed max(5 W, 10%) below the draw that failed for a
  full StepDownHold (sustained forgiveness since 2026-07-27 — instant forgiveness
  re-armed the descent on every lull of a fluctuating load, harness S7). A spent
  budget (trend ≥ ceiling) with the trend not falling reads `TauSeconds = 0`, not ∞,
  which also routes it into the ramp branch.
  `ThermalModel` (per channel, persisted in profile.json as `LearnedThermalMassJPerC` /
  `LearnedBaseTempC` / `LearnedResistances`, UI saves every ~5 min) learns online:
  C from surplus-watts vs temp slope, R anchors (0/20/…/100%) + base from quasi-steady
  points; seeds are NH-D15-class (C 450 J/°C). **Learning guardrails (2026-07-26)**:
  base (the ZERO-watt baseline, ambient + case air) is clamped to ≤45° and R samples
  to a fan-speed-sliding floor (0.20 °C/W at 0% → 0.05 at 100%; `MinR()`), and
  `LoadFrom` rejects out-of-bounds saved values as unlearned. Without them the
  (base, R) pair is unidentifiable from single-operating-point data (fans parked at
  0% all day = one point) and drifts along a ridge — base eats the steady temp level,
  R collapses, and the more-fan-never-cools-worse clamp used to flatten the whole
  anchor ladder to the collapsed value (now it also respects the per-anchor floors).
  Found on Kuba's machine 2026-07-26 as base 52°/R flat ~0.08–0.10 on BOTH channels
  (model claimed ~200 W dissipation at 44 W draw → every energy-side prediction
  blind; partly legacy of the pre-2026-07-25 bug where `--sim` runs saved sim-learned
  models into the real profile). His profile was healed in place (learned values
  zeroed; backup `profile.json.bak-2026-07-26`) and — his preference, stated twice:
  ~70° die should mean fans — **his machine runs `SteadyTargetMarginC` 20 (sustained
  aim 70°) and `RampLeadSeconds` 90 ("fans should kick in much earlier")**; preset
  defaults stay 10/45 — revert via the dev-panel sliders. **Fuse**: raw temp ≥ `Profile.OverrideTempC`
  (default 90) → the channel's own staircase evaluated on the RAW temp is written
  instantly, no slew, output never decays while latched (release: 3° below for 10 s);
  stop probe + idle kick are bypassed during override. Under a load that pins Tctl at
  the fuse this degrades into exactly the hand-tuned temp staircase. The curve stays
  meaningful in power mode: ladder of allowed levels + fuse fallback + why-chip
  comparison. App-level settings (don't mark "Custom"): `PowerControlEnabled`
  (default true), `PowerAveragingSeconds` (60), `RampLeadSeconds`, `OverrideTempC` —
  dev-panel checkbox + three sliders + live `draw · avg` / `buffer · needs` /
  `headroom` readout (that line renamed from `lead` 2026-07-27);
  channels without power sensors keep the temp filter. Why-chip reasons: BudgetHold /
  BudgetRamp / HardOverride.
  **Every remaining knob is exposed too (2026-07-25, "add all the new parameters to
  be shown in developer mode")** — what used to be `private const` in
  `PowerBudgetController` are now app-level profile settings with dev-panel sliders
  under a `BUDGET INTERNALS` sub-header: `BudgetCeilingMarginC` (4, ceiling =
  override − this), `SteadyTargetMarginC` (10, sustained aim; clamped in `Step` so it
  can never sit above the ceiling), `PowerTrendSeconds` (30), `PowerSlopeSeconds`
  (25), `PowerNowSeconds` (10), `OverrideReleaseC` (3) and `OverrideReleaseSeconds`
  (10), plus a live `ceiling · aim` / `trend · °C/s` readout — and under it (Kuba's
  ask 2026-07-27, "add separate line with slider options that are used in new
  algorithm") a `headroom: lead · ±band` / `avg · quiet gate` line gathering every
  knob the aim-referenced headroom reads (lead, the channel's HysteresisC dead band,
  power averaging, trend+slope = the quiet-gate span), since those sliders live in
  three different panel sections; tooltip maps each value back to its slider. A `LEARNED MODEL`
  sub-header follows: `ThermalLearningEnabled` (default true — unchecking freezes the
  model where it stands), a readout of mass / base / R-at-the-current-speed / the six
  R anchors (read straight off `ChannelConfig`, which the engine rewrites every tick),
  and a **Reset learned model** button (`FanEngine.ResetThermalModels()` — clears the
  live models and the persisted values without touching control state, so the fans
  don't jump). Dev-panel readout lines are hand-broken with `\n`: the panel fits
  ~31 mono characters, and past that WPF wraps mid-token (`°C/W` → `°` + `C/W`).
  The four long setting checkboxes now hold a wrapping `TextBlock` instead of a
  string `Content` — a plain string clips at the panel edge. `SimulatedBackend` is now a real plant (sink 420 J/°C
  behind fan-dependent resistance, die rides ~0.055 °C/W above sink, power sensors
  `sim/cpu-pwr`/`sim/gpu-pwr`) so `--sim` exercises the controller honestly. All
  scenarios (burst immunity, predictive ramp, fuse with corrupt model, power step-down,
  C/R learning convergence) verified by a scratchpad console harness on 2026-07-24.
  The upward enforcement was verified by a third harness on 2026-07-26 (sim plant
  replica, deterministic): 90 W sustained now steps 0→20% ~8 min in and settles at
  ~71° die (was: parked at 84.2° with fans off forever); 188 W converges to 100%
  holding ~79.7° (the aim); 8 s/150 W spikes still never move the fans; learning-off
  behaves the same as learning-on thanks to the model-free prong + the brand; after
  the load ends the brand is forgiven and the fan winds back to 0.
  **Fourth harness 2026-07-27 (the aim-referenced headroom; 10 scenarios by the end
  of the day)**, deterministic sim-plant replica, Kuba's margins (aim 70 / ceiling
  86 / lead 90) plus a preset-defaults scenario: settled idle 44 W reads ∞ with fans
  off; a constant 90 W step gets fans at +70 s (trend 64°) and settles 65° at 40%
  with tau recovering ONLY on the fan steps; a 44→90 W creep over 8 min shows a
  242 s visible countdown before firing at 66°; 150 W and 250 W spike trains never
  move the fans (min tau 632 s, never near the trigger); 188 W reaches 100% in
  ~1 min peaking 80.1° die, no fuse; a frozen optimistic-corner model still gets
  fans via the measured prong and holds a stable 40% thanks to the ramp brand;
  preset defaults fire at 78.9° and settle 70.7°; **S7, the report repro** — a
  realistic game (95 W mean swinging 111↔167 W, bursts to ~155, Kuba's learned
  values, model reset mid-rise) — fires predictively at +58 s / trend 67° and then
  holds one steady 50% for ten minutes straight through the swings; S5b (90 W onto
  a warm 63° idle) fires at +56 s / 68.2° and the warm idle itself unwinds to
  fully-stopped fans.
  Gotcha: don't lower `OverrideTempC` much below 90 on the 9950X3D — the ceiling is
  `Override − 4` and the steady target `Override − 10`, so 85 forces near-100% fan for
  loads Kuba's Quiet curve holds at 81% (found in harness scenario 2). Second harness
  (2026-07-25, the newly exposed knobs): a bigger ceiling margin ramps earlier and
  peaks cooler (first fan at 156/131/87 s for 4/10/20 °C), a tighter aim settles
  higher (81/90/100 %), inverted margins clamp instead of misbehaving, extreme windows
  stay finite — but **long trend/slope windows are the risky end**: at 120 s/120 s the
  ramp waits until 205 s and the die reaches 89.9 °, i.e. it only just stays under the
  fuse.
- Changing ChannelConfig field names breaks saved `%AppData%\FanCurves\profile.json`
  (old fields silently ignored, defaults kick in) — delete it after schema changes.
- Sensor/control IDs are backend-specific; `AutoAssign` prunes IDs the current backend
  doesn't know and re-assigns empty channels on every launch (a profile saved in `--sim`
  works on real hardware and vice versa).
- **Dev flows never write the config** (`Profile.ReadOnly`, set from `--sim` /
  `--screenshot` in `App.OnStartup`): `Save()` is a no-op and the sensor dump goes to
  `sensors.sim.txt` instead of overwriting `sensors.txt`. Before this (bug found
  2026-07-25) every `--sim` run rewrote the real `profile.json`: AutoAssign pruned the
  hardware IDs the sim backend doesn't know and saved `sim/*` in their place. Auto-
  assignable headers heal on the next real launch, but **manual assignments do not** —
  Kuba's Pump Fan header (never auto-assigned by design) was silently lost that way.
  Keyed off the flags, not `hw.IsSimulated`, so an elevated launch that falls back to
  simulation because the kernel driver failed still saves the user's edits.
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
- **Installed & verified on real hardware 2026-07-23** (the X870 Steel Legend /
  9950X3D build): v0.1.0 exe at `%LOCALAPPDATA%\Programs\FanCurves\FanCurves.exe`,
  autostart task registered, PawnIO 2.2.0 installed (silent flags are
  `-install -silent`, dash-style — `/S` fails with exit 87). LhmBackend sees the
  NCT6686D: 8 controllable headers + all board temps; CPU temp from
  `/amdcpu/0/temperature/2` (Tctl/Tdie). Control works — the Quiet preset stopped
  the CPU fans at idle within the first minute. Kuba manually assigned the
  "Pump Fan" header (`/lpc/nct6686d/0/control/1`, physically the second NH-D15
  fan) to the CPU cooler channel, as on the previous test setup.
