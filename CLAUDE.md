# fan-curves — Windows fan control with a MacBook-like default

WPF (.NET 8) app that drives motherboard fan headers the way a MacBook manages its
fans: silent flat floor at idle, no reaction to short temperature spikes, smooth
ramp only under sustained load (class-level default slew 8 %/s both directions; the Quiet preset's CPU channel runs 9 up / 8 down since 2026-07-22). Built for Kuba's 2026 PC
(ASRock X870 Steel Legend WiFi + 9950X3D + NH-D15 + Arctic P14 Pro chain in a
Phanteks P600S), but hardware-agnostic.

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
- `src/FanCurves` — WPF UI, **two modes**. Under the curve chart sit two
  dev-mode strips. **History strip** (`HistoryChart.cs`, Developer mode only —
  collapsed in simple mode, the curve chart takes the freed space): a 10-min
  window of the selected channel's rolling-average temp (bright trace), raw
  "now" temp (faint dashed) and commanded fan % (dim trace + under-fill), one
  sample per engine tick. **Storage is two-tier** (2026-08-04, "scrollable, don't
  keep everything in memory"): `ChannelHistory` keeps only the last 600 samples
  in a RAM ring (the live window, fed from `MainWindow.OnEngineTicked` in both
  modes so history is already populated when dev mode opens) and appends every
  sample to a per-channel binary spill file in `%TEMP%\FanCurves\`
  (fixed 26-byte QUANTIZED records — tenths int16 for temps/watts/percent,
  whole-second uint16 tau with 0xFFFF = ∞, whole-second uint32 time — the ring
  keeps exact doubles, only the disk is lossy, below display resolution;
  `FileOptions.DeleteOnClose` so it vanishes with the
  process even on a crash, ~24 h ≈ 2.2 MB/channel retention via a once-a-day
  compaction at 2×; any file error silently degrades to the RAM-only ring). `HistorySample`
  carries the wall-clock `Time` of the measurement since the same change.
  **Both strips scroll back** through the spill as ONE timeline via a shared
  `HistoryViewport` (in StripChart.cs, owned by MainWindow): wheel ≈1 min/notch
  (Shift 10×), drag pans, double-click or the **LIVE text-button** (appears next
  to CLEAR only while scrolled) returns to the live edge; scrolled reads go
  through a one-window cache so hover redraws don't touch the disk, reaching
  "now" snaps back to following live, channel switch resets to live, and a
  scrolled window is anchored to absolute sample indices so incoming ticks
  don't move it. The time axis prints wall-clock `HH:mm` labels at 5-minute
  boundaries with "now" (live) or the window-end `HH:mm:ss` (scrolled) at the
  right edge. Right edge = now when live (amber live dots — the only amber
  there; they and the live counting-up stopped-span vanish while scrolled);
  hover crosshair with a `clock · ago · avg · now · %` chip. Fan turn-ON
  events get a baseline tick, turn-OFF a dimmer one, with the m:ss stopped time
  (turn-OFF → next turn-ON) on a dim span between them (label seats on the card
  background; skipped only when it would overlap the previous span's label; an
  ongoing stop counts up live; no span when the stop predates the window). Also
  drawn on the temp scale: the **budget ceiling** (dotted, power-controlled
  channels only) and the **sustained aim** (dimmer dotted; the aim label seats
  BELOW its line, the ceiling's above, so they never collide). Left/right
  padding matches `CurveEditor` so the plots align.
  **Budget strip** (`BudgetChart.cs`, dev mode only, underneath): the same
  window from the thermal-budget controller's side — instantaneous draw (faint
  dashed) and sustained average (bright trace + under-fill) on a **watts** scale
  auto-ranged to the window's own peak (`NiceWatts` ladder), against the
  predicted headroom `TauSeconds` (quiet line, right scale in **seconds**,
  **logarithmic 10 s → 30 min**: top = ≥30 min/∞, the chip prints ∞ there, refs
  at 1:00 and 10:00) and a dotted `ramp lead` line = the step-up trigger, seated
  in the lower third so a genuine dive is a long visible descent. Headroom is
  measured to the sustained aim (behaviour contract): it drains under a
  sustained fans-off load and recovers only on a fan step or load end.
  Hard-override spans are shaded with a "fuse" label; hover chip reads
  `clock · ago · draw · avg · buffer kJ · headroom · needs %` (dev-panel vocabulary).
  No under-fill under the headroom trace — a healthy buffer pins it to the top
  and a fill would flood the strip. Channels without power sensors get a centred
  "no power sensor on this channel" note. Both strips share `StripChart.cs`
  (padding, title, legend, wall-clock time axis, scroll/drag handling, hover
  crosshair/chip, trace + under-fill helpers) and render the window the shared
  `HistoryViewport` serves (`TakeSnapshot()` at the top of each `OnRender`
  fills the protected `Win`/`WinCount`/`IsLive`); `HistorySample`
  carries the budget telemetry (watts, avg, credit, tau, demand, ceiling, aim,
  override flag). Dev mode's fixed window is **1568×830**; `EnterFixed` clamps
  both dimensions to the work area on small screens. A **CLEAR text-button**
  right of the HISTORY title (dev mode only) wipes ring + spill file on EVERY
  channel so both strips restart from the right edge — a XAML overlay in the
  chart-card grid row (the strips are OnRender-only and can't host children),
  position tuned to seat beside the drawn title; the LIVE button is the same
  pattern one seat further right. Hover chips take several wordings and
  draw the widest that fits the plot (`DrawChip(dc, x, wide, narrow)`;
  `FormattedText` honours `\n`) so quarter-screen windows get a two-line chip.
  Reference labels (ceiling, ramp lead) are drawn LAST and seated on the card
  colour, otherwise the spiky draw trace runs straight through them.
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
  histories, the tray tooltip (deduped — Shell_NotifyIcon is called only when
  the whole-degree text changed) and the ~5-min model save — ALL painting lives
  in `RefreshLiveUi()`, run per tick when visible and once from
  `IsVisibleChanged`/`StateChanged` to catch up on reopen/restore; (3) the
  title-bar fan spin (the app's one perpetual animation) is capped at 20 fps
  via `Timeline.SetDesiredFrameRate` — at the default 60 it forces a
  composition pass per frame the whole time the window shows.
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
  - Developer (top-bar toggle, or `--dev`): **two-column panel, 520 px wide** —
    the CONTROL MODE switch leads at the top (full panel width, the headline
    choice), hairline seam between the columns, SOURCES/BACKEND full-width
    underneath; each column is as wide as the old single-column panel, so the
    ~31-mono-char readout line breaks still hold. Every toggleable feature is a
    titled group whose **master checkbox IS the group header** (MicroLabel-styled
    content inside the checkbox; explanations in tooltips). Left column: CHANNEL
    RESPONSE (per-channel filter knobs, always active, no checkbox), SAFETY
    FLOOR ("Never below" slider; see the behaviour contract), STOPPED-FAN KICK,
    STOP INSTEAD OF SLOW (zero snap), TRIAL STOPS (stop probe), INSTANT APPLY.
    Right column: POWER CONTROL (power averaging, power-curve hysteresis, ramp
    lead), AUTO FLOOR GUARD, FUTILITY PROBE, DOWNWARD RELIEF, POWER FLOOR, HARD
    OVERRIDE (fuse trigger + release drop/hold), BUDGET INTERNALS (ceiling/aim
    margins + the three windows), LEARNED MODEL. Knob-less groups are a bare
    header checkbox (AUTO FLOOR GUARD, FUTILITY PROBE, INSTANT APPLY, REVIEW
    LOGGING, HIGH PROCESS PRIORITY — the last two full-width above BACKEND: app
    plumbing, not control). Exactly two things have no switch, both on purpose:
    **HARD OVERRIDE** (the fuse — the last line before the BIOS would have to
    save the die) and **CHANNEL RESPONSE** (following the curve at all; its
    knobs can be flattened individually). A group whose switch is off **dims to
    45 % opacity but stays editable** (settings must be reachable with the
    checkbox off); DOWNWARD RELIEF's body also dims when the futility probe is
    off — relief can only arm behind the latch. Relief/power-floor "off"
    (`Profile.ReliefEnabled` / `Profile.PowerFloorEnabled`, both default true,
    app-level) is implemented in `FanEngine` by feeding the controller natural
    "never fires" values (relief cap 0 W — an active waiver dies on the next
    tick; floor anchors 0/0), so `PowerBudgetController` needed no changes; the
    behavior log's settings line prints `relief off` / `pfloor off`.
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
  (1010×660, 1568×830 in dev mode), quarter-of-screen (half work-area width × height, snapped to the nearest
  screen corner), and maximized — cycled in that order by the maximize caption button
  (its glyph previews the next size); drag-resize disabled
  (`ResizeMode=CanMinimize`, `ResizeBorderThickness=0`), and a `WM_GETMINMAXINFO` hook
  in `Chrome` clamps maximize to the work area (borderless windows otherwise cover the
  taskbar). Further elements: layered near-black surfaces (canvas `#0a0a0d`, cards `#111116` with light-from-above
  gradient hairline + drop shadow), monochrome white at graded opacities, and ONE
  accent — warm amber `#FF9E5E` — reserved strictly for live thermal state (chart
  operating dot + crosshair with axis readout chips, status-chip dot, and the
  dev-mode raw-temp dashed line + its "now …°" label — temperature is the amber
  quantity and the power reference lines stay monochrome). Hero = large
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
  default**):
  **all fans fully stopped at idle** (0% floor, 0% bottom step): CPU cooler 0%
  through <50°C avg, then 10/20/40/50% at 50/55/62/84°C (the temp staircase caps
  at 50% — sustained-load speed comes from the power side in Auto; synced to
  Kuba's live curve 2026-08-03), with a **90 s averaging window**, 1.5°C
  hysteresis, 25 s step-down hold, slew 9 %/s up / 8 down; case fans 0% below
  40°C avg, then 5/10/20/40/60/82/100% at 40/42/60/70/75/82/86°C, 25 s
  averaging, 4°C hysteresis, slew 7 %/s (whisper bands are fine on the NF-A14s,
  which start at any duty >0%).
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
  ? ch.MinPercent : 0` — that every floor read in the tick uses (all three control
  paths' output/target, the `MinFloor` why-chip, the stop probe's `minPct <= 0`
  gate), and the settings fingerprint reads the gated value too, so flipping the
  switch snaps instantly like any other edit. With it off a floored channel can go
  to a full stop AND becomes trial-stoppable (the probe gate is the floor's, not a
  separate rule); the per-channel value itself is kept, so switching back restores
  it. Dev-panel group SAFETY FLOOR (master checkbox) + the "Never below" slider
  (0–60%, per channel, still marks the profile "Custom"); the behavior log's
  settings line prints `min off` in place of `min NN%`.
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
  the stable band, arming the failed-trial backoff as usual).
  **Demand gate**: a budget-driven
  channel is never trial-stopped while `DemandLevel > 0` — the probe's stability
  test is blind to a die-limited load (a CPU clamping its own temperature reads
  perfectly "stable" at full draw, and because the clamped die can never "rise",
  an ungated trial can latch the fan at 0% under load while the CPU throttles).
  Temp-only channels are unaffected (their demandLevel is always 0).
- **Thermal-budget control ("control with power, not temperature")**: channels
  with assigned power sensors (`ChannelConfig.PowerSensorIds`, watts summed;
  AutoAssign wires CPU package power to the CPU channel, CPU+GPU to the case
  channel) are driven by `PowerBudgetController` in `PowerBudget.cs` instead of
  `ResponseFilter`. Philosophy: the 60 s power average is the real dissipation
  demand; the heatsink's thermal mass is credit — `E = C·(ceiling − trendTemp)`
  (trend = 30 s avg, filters the die's instant jump). Fan steps UP only when the
  predicted **headroom** drops under `Profile.RampLeadSeconds` (default 45; Kuba
  runs 90), directly to the lowest curve-ladder level whose predicted
  equilibrium (`base + R(level)·PowerNow`) holds the **sustained aim**.
  **Headroom**: `TauSeconds` = predicted seconds until the sink trend crosses
  the guarded line — the sustained aim while below it, the ceiling once past
  it — min of a model prong (exact first-order time `R·C·ln((eq−T)/(eq−line))`,
  finite whenever the equilibrium `base+R(output)·PowerAvg` clears the line by
  ≥ HysteresisC) and a measured prong (`(line−trend)/slope`); headroom recovers
  only on a fan step or load end. **Prong gating**: the MODEL prong runs UNGATED
  (it reads only sustained quantities; its aim-clearance margin absorbs
  power-window aliasing), with one veto — a measurably FALLING trend
  (slope < −0.01) contradicts its first-order-approach premise (else a
  just-ended load's still-hot minute-average fires a pointless ramp mid-decay);
  the settled StepUpHold arm's slope condition gates BOTH its disjuncts for the
  same reason. The SLOPE prong keeps a strict peak gate (no burst above
  `max(10 W, 25%)` over PowerAvg within the last SlopeWindow+TrendAvg seconds) —
  a spike's die jump reads as a rush toward the aim for as long as the fit can
  see it. LEARNING gets the longest quiet horizon (slope+trend+power windows
  ≈ 115 s at defaults) — a poisoned R turns the ungated model prong into a false
  accuser; with honest learning, the margin alone keeps spike trains silent. A
  sudden wall-jump load is spike-indistinguishable early on — the fuse stays the
  hard stop. **One ramp step per slope window**: the measured slope is
  backward-looking and knows nothing of the step just taken; within the settle
  window a further step needs the chosen level to have become objectively
  insufficient (draw rose; its predicted equilibrium no longer holds the aim). A
  gate-open ramp step **brands** the level it left exactly like a StepUpHold
  step (else an optimistically-wrong model argues the fan straight back down
  after every predictive step — a slow on/off hunt); gate-closed (model-only)
  fires don't brand — no measured corroboration yet. Steps DOWN after ONE
  StepDownHoldSeconds hold once the power average no longer needs the current
  level — **snapping straight to the lowest ladder level that holds the aim at
  5/4 of today's sustained draw** (in Auto the floor IS the curve, so the
  descent hands straight back to the hand-tuned staircase; one snap, never below
  a branded level, slew still glides the output) — reacting minutes before the
  cooling temp average would. The 5/4 margin is hysteresis in the DRAW
  dimension, where the swings actually live (a ±HysteresisC temp band alone is
  worth only ~4 W at a 20% fan level and hunts across it); deliberately NOT
  stacked with the temperature band (the double margin blocked the final step to
  silence at warm idle). `ChannelConfig.HysteresisC` still feeds the
  controller — as the model prong's aim-clearance margin.
  **The sustained aim is also enforced UPWARD** (the aim-referenced tau goes
  blind at a settled equilibrium a frozen/unlearned model believes is under the
  aim): trend past the steady aim AND (power average demands a higher ladder
  level OR the temp has settled there, slope > −0.002 — the model-free prong
  catches frozen/unlearned models) → one ladder step up per StepDownHoldSeconds
  (why-chip reason `StepUpHold`). Each step up brands the level it left as
  measured-insufficient at that PowerAvg — DemandLevel is clamped above the
  branded level so the model can't argue the fan back down into a slow on/off
  limit cycle; the brand lifts once the sustained draw has stayed max(5 W, 10%)
  below the failing draw for a full StepDownHold (sustained forgiveness —
  instant forgiveness re-armed the descent on every lull of a fluctuating load).
  A spent budget (trend ≥ ceiling, trend not falling) reads `TauSeconds = 0`,
  not ∞ — also routed into the ramp branch.
  `ThermalModel` (per channel, persisted in profile.json as
  `LearnedThermalMassJPerC` / `LearnedBaseTempC` / `LearnedResistances`, UI
  saves every ~5 min) learns online: C from surplus-watts vs temp slope, R
  anchors (0/20/…/100%) + base from quasi-steady points; seeds are NH-D15-class
  (C 450 J/°C). **Learning guardrails**: base (the ZERO-watt baseline, ambient +
  case air) is clamped to ≤45°, R samples to a fan-speed-sliding floor
  (0.20 °C/W at 0% → 0.05 at 100%; `MinR()`), and `LoadFrom` rejects
  out-of-bounds saved values as unlearned — without them the (base, R) pair is
  unidentifiable from single-operating-point data (fans parked at 0% all day =
  one point) and drifts along a ridge: base eats the steady temp level, R
  collapses, and the more-fan-never-cools-worse clamp flattens the whole anchor
  ladder to the collapsed value (it also respects the per-anchor floors).
  Kuba's machine (his preference, stated twice: ~70° die should mean fans) runs
  **`SteadyTargetMarginC` 20 (sustained aim 70°) and `RampLeadSeconds` 90
  ("fans should kick in much earlier")**; preset defaults stay 10/45 — revert
  via the dev-panel sliders. **Fuse**: raw temp ≥ `Profile.OverrideTempC`
  (default 90) → the channel's own staircase evaluated on the RAW temp is
  written instantly, no slew, output never decays while latched (release: 3°
  below for 10 s); stop probe + idle kick are bypassed during override. Under a
  load that pins Tctl at the fuse this degrades into exactly the hand-tuned temp
  staircase. The curve stays meaningful in power mode: ladder of allowed levels
  + fuse fallback + why-chip comparison. App-level settings (don't mark
  "Custom"): `ControlMode` (Temp/Power/Auto — see below; default Auto),
  `PowerAveragingSeconds` (60), `RampLeadSeconds`, `ReliefMaxWatts` (190),
  `OverrideTempC` — dev-panel CONTROL MODE switch + four sliders + live
  `draw · avg` / `buffer · needs` / `headroom` readout; channels without power
  sensors keep the temp filter. Why-chip reasons: BudgetHold / BudgetRamp /
  HardOverride.
  **Every internal knob is exposed too** — former `private const` values in
  `PowerBudgetController` are app-level profile settings with dev-panel sliders
  under a `BUDGET INTERNALS` sub-header: `BudgetCeilingMarginC` (4, ceiling =
  override − this), `SteadyTargetMarginC` (10, sustained aim; clamped in `Step`
  so it can never sit above the ceiling), `PowerTrendSeconds` (30),
  `PowerSlopeSeconds` (25), `PowerNowSeconds` (10), `OverrideReleaseC` (3),
  `OverrideReleaseSeconds` (10), plus a live `ceiling · aim` / `trend · °C/s`
  readout — and under it a `headroom: lead · ±band` / `avg · quiet gate` line
  gathering every knob the aim-referenced headroom reads (lead, the channel's
  HysteresisC dead band, power averaging, trend+slope = the quiet-gate span; the
  sliders live in three different panel sections; tooltip maps each value back
  to its slider). A `LEARNED MODEL` sub-header follows: `ThermalLearningEnabled`
  (default true — unchecking freezes the model where it stands), a readout of
  mass / base / R-at-the-current-speed / the six R anchors (read off
  `ChannelConfig`, which the engine rewrites every tick), and a **Reset learned
  model** button (`FanEngine.ResetThermalModels()` — clears the live models and
  the persisted values without touching control state, so the fans don't jump).
  Dev-panel readout lines are hand-broken with `\n`: the panel fits ~31 mono
  characters, past that WPF wraps mid-token (`°C/W` → `°` + `C/W`). Each readout
  must be ONE interpolated string: `Inv($"…" + $"…")` does not compile —
  concatenated interpolated strings lose the FormattableString conversion. The
  four long setting checkboxes hold a wrapping `TextBlock`, not a string
  `Content` (a plain string clips at the panel edge). `SimulatedBackend` is a
  real plant (sink 420 J/°C behind fan-dependent resistance, die ~0.055 °C/W
  above sink, power sensors `sim/cpu-pwr`/`sim/gpu-pwr`) so `--sim` exercises
  the controller honestly. Gotcha: don't lower `OverrideTempC` much below 90 on
  the 9950X3D — the ceiling is `Override − 4` and the steady target
  `Override − 10`, so 85 forces near-100% fan for loads Kuba's Quiet curve holds
  at 81%. Gotcha: **long trend/slope windows are the risky end** — at
  120 s/120 s the ramp waits until 205 s and the die reaches 89.9 °, only just
  under the fuse.
- **Futility edge + experiment latch**: a die-limited load — the conduction
  gradient under the die dwarfs what airflow can touch, so the die self-clamps at
  the same temperature at every fan speed (nearly all of the learned 0.29 °C/W is
  inside the package) — used to read as
  demand = max: no ladder level held the aim, so `DemandLevel` fell through to
  `ladder[^1]` and the settled StepUpHold arm marched one step per 25 s to 100%,
  buying <1° total. Two mechanisms in `PowerBudgetController`, both HysteresisC-
  scaled: (1) **model band** (`UsefulLevel`) — when no level's predicted equilibrium
  holds the aim, demand and both step-up paths go no further than the lowest level
  within HysteresisC of the best equilibrium the ladder offers (the ramp branch's
  blind `ladder[^1]` fallback got the same treatment); (2) **experiment latch** —
  the band alone loses to online learning (LearnSteady teaches the visited anchor
  the measured R while unvisited anchors stay stale-optimistic, so the model
  perpetually claims the NEXT step buys degrees), so every model-driven up-step
  from a settled
  state (|slope| < 0.005, draw settled) records (level, trend, draw); once the
  trend is flat again at unchanged draw (±max(5 W, 10%)) without having dropped
  HysteresisC, the step is taken back, the brand it set is cleared (the level
  wasn't weak — the aim is unreachable), and model-driven steps above the
  returned-to level are latched off until draw or trend leaves that neighbourhood
  (trend release is symmetric ±HysteresisC, so a wrong verdict self-heals as the
  temp climbs). Floor-adopt/fuse/step-down clear an in-flight experiment (the jump
  confounds it). Net effect in Auto: the hand-tuned staircase floor is what rules a
  clamped load, the budget adds ONE ~25 s probe
  step, proves it futile, and holds — a boost-clamped CPU converting fan into
  watts at constant temp counts as futile by
  design; a clearly grown draw (>10%) re-baselines instead.
  Switchable: `Profile.FutilityProbeEnabled` →
  `PowerBudgetController.FutilityProbeEnabled` (default true, app-level, dev-panel
  group FUTILITY PROBE). Off: no experiment is opened, no latch set (a standing
  one — and any relief waiver behind it — is dropped on the next `Step`), and both
  `UsefulLevel` and the ramp branch fall back to `ladder[^1]` when no level holds
  the aim, i.e. a march to max fan. **Downward relief arms only
  behind the latch, so it is off with this too.** Settings line logs
  `futility on/off`.
- **Downward relief**: while the latch stands, the draw is settled + under
  `Profile.ReliefMaxWatts` (default 190, DOWNWARD RELIEF group
  slider 50–400 W; the group's `Profile.ReliefEnabled` master checkbox
  turns the whole probe off) and the trend flat above the aim, the controller probes BELOW the
  running level — **the only case where measured evidence may waive the Auto
  floor** (`EffectiveFloor` in the controller; `_reliefLevel` is a standing
  waiver). First step down one ladder level, judged for a StepDownHold like the
  up-experiments; once it holds, SNAP straight to the relief bound —
  max(ZeroSnapPercent, half the start level), so 81% floor → 65 trial → 50 —
  less fan, never none. A level is "too low" when the short draw average sags
  max(5 W, 5%) under the step's baseline (missing airflow is paid in CLOCKS on a
  clamped die — the CPU throttles at constant temp); that level is remembered bad
  for the episode and the last proven level comes back. The whole waiver dies
  restoring the start level the instant the trend climbs HysteresisC off the flat
  baseline it was proven on or the draw crosses the cap; a VANISHED load instead
  keeps the waiver standing so the stale floor (90 s average, hot for another
  minute) cannot pull the fans UP right as the heat goes away — it clears
  quietly once the floor falls to it.
- **Power floor**: Kuba's calibration (200 W sustained draw → 80% fan,
  100 W → 30%) is a first-class mapping the die-referenced
  model cannot express (at 200 W the die clamps at every fan speed, so no
  die-temperature aim reproduces "80% is right") — a continuous line through
  (100 W → `PowerFloorPercentAt100W`, default 30) and (200 W →
  `PowerFloorPercentAt200W`, default 80), linear, extrapolated and clamped
  [0, 100], evaluated on the sustained power average each tick and folded into
  `EffectiveFloor` — so it binds the OUTPUT continuously (need not be a ladder
  level), outranks the futility latch AND the relief waiver (the waiver only
  waives the temperature staircase), and rides under the fuse too. At idle draws
  the line falls below the zero-snap threshold, so fans still stop; both sliders
  at 0 = off. Two POWER FLOOR group sliders (0–100%) behind the group's
  `Profile.PowerFloorEnabled` master checkbox, app-level like the
  rest; settings line logs `pfloor 30%@100W/80%@200W` (or `pfloor off`).
  Post-load descents glide down the line as the average decays instead of stepping.
- **Power curve — Curve control mode**: each
  channel carries a second staircase `ChannelConfig.PowerPoints`
  (`PowerPoint(Watts, Percent)` in FanCurve.cs) — the watts twin of `Points`. In
  the 4th control mode **Curve** (`ControlMode.PowerCurve`), channels with a
  power sensor are driven by it DIRECTLY: a second `ResponseFilter` instance runs
  in the watts dimension (input = instantaneous draw, averaging =
  `PowerAveragingSeconds`, hysteresis = `Profile.PowerCurveHysteresisW` —
  app-level setting, default 10 W, POWER CONTROL slider 2–50 W; hold/slew/zero-snap
  from the channel), so the whole MacBook feel — spike immunity, step-down hold,
  slew — applies to watts, deterministically, with the predictive budget layer off
  entirely. The temperature filter keeps running as a safety floor
  (output = max of the two), and a per-channel fuse latch in `FanEngine`
  (`FuseState`, same contract as the budget's: raw ≥ `OverrideTempC` → temp
  staircase on the RAW temp instantly, output never decays while latched, release
  −`OverrideReleaseC` for `OverrideReleaseSeconds`) stays armed. The watts-side
  target also feeds `DemandLevel`, which keeps the stop-probe demand gate working
  (a die-limited load would otherwise trial-stop at full draw).
  `FanCurve` got axis bounds (`axisMin/axisMax` ctor args; `FromPower` builds the
  watts staircase — without this, Normalize used to clamp watts into 15–100!).
  Empty `PowerPoints` = pure temp behaviour; `LoadOrDefault` seeds missing power
  curves from the Quiet preset (both presets ship defaults sized for a ~200 W CPU /
  CPU+GPU case sum; `AdoptTuning` copies them; editor never leaves <2 points, so
  empty means "never had one"). **UI**: `CurveEditor` is axis-generic
  (`PowerAxis` property; °C snaps whole degrees/1° min gap, W snaps 5 W/5 W gap;
  power axis top = max(300, top point + headroom) rounded to 50) — a `CURVE °C/W`
  toggle button in the chart-card header (dev mode only, resets to °C on leaving
  dev mode) flips the chart; on the power axis the operating dot/crosshair is
  WHITE (amber stays thermal-only) at (sustained draw, output) and the dashed
  vertical is the instantaneous draw. Undo/redo shares the one stack (`CurveEdit.Power`
  flag; power snapshots carry watts in the TempC slot, internal only); undoing a
  power edit flips the chart to the power axis so the restore is visible. Why-chip
  reason `PowerCurve` ("power curve: avg NNN W → X% · temp curve asks Y%") when the
  watts side out-asks the temp floor; behavior log describes it and the settings
  line carries `pwrHyst` + per-channel `pwrCurve W:%…`.
- **Control-mode switch**: dev-panel `CONTROL MODE` segmented switch
  (Temp · Power · Auto · Curve, at the TOP of the panel) → `Profile.ControlMode`
  (string-serialized enum; app-level — no "Custom", presets don't touch it). The
  old bool `PowerControlEnabled` stays as a JSON bridge property declared BEFORE
  the enum, so pre-change profiles map false→Temperature on load while a
  new-format file's `ControlMode`, deserialized after it, always wins (a legacy
  true leaves the mode at the default — Auto). **Default Auto**: the staircase
  is a guaranteed floor and the power side may ramp earlier/higher. Temperature
  forces every channel onto `ResponseFilter` — Temperature mode IS the old
  temperature-only behaviour (power sensors unread; budget strip note + power
  readout say "temperature mode — power side off"). **Auto runs both sides every
  tick and the higher demand wins**, implemented INSIDE the controller as
  `PowerBudgetController.FloorPercent`: the engine steps the ResponseFilter
  first and feeds its snapped `TargetLevel` in as a floor which the budget's
  published target and slewed output never drop below. The floor lives inside
  the controller — not as a max() in the engine — so the budget's physical
  output is what its physics read: surplus, the model prong's equilibrium and
  `LearnSteady` all see the fan that actually spins (an outside max() poisons
  the model — LearnSteady pairs floor-cooled temps with the budget's parked
  0 %). No new why-chip reasons: floor binding ⇒ output = the curve's level ⇒
  chip hidden. **Floor guard**: in Auto the buffer is also draining toward the
  staircase floor's NEXT step (the lowest curve temp commanding more than the
  current level, zero-snap respected — `NextFloorStep`), where the floor fires
  on the 90 s average whatever the model thinks — ungated, this produced a real
  limit cycle (a ~37 W desktop draw settling exactly on the curve's 57° first
  step cycled the fans on/off every 2–4 min while the budget read "headroom ∞ ·
  demand 0"). Two mechanisms, both `GuardFloor`-gated (engine sets it in Auto
  only, so Power mode stays tick-identical): (1) the MEASURED headroom prong
  also watches the trend drain toward the floor line — settled-draw-gated slope
  only (a model arm was tried and cut: near an unvisited operating point the
  learned R is at its least trustworthy); (2) **a floor step that out-ranks the
  budget's target is treated as measurement**: the budget brands the level it
  beat (same brand as StepUpHold, no draw-settled gate — the 90 s average
  crossing IS sustained evidence) and adopts the floor's level as its own
  target, so after the floor recedes the budget holds the fan steadily
  (why-chip: BudgetRamp "curve asks 0%") instead of following it back down into
  the cycle; the hold unwinds through normal brand forgiveness once the
  sustained draw genuinely drops. Switchable: `Profile.FloorGuardEnabled`
  (default true, app-level, dev-panel group AUTO FLOOR GUARD) — the engine feeds
  `budget.GuardFloor = filter != null && Profile.FloorGuardEnabled`, so off
  returns Auto to its pre-guard behaviour; Power mode is unaffected either way;
  settings line logs `floorGuard on/off`.
- **Instant apply on settings change**: the engine fingerprints every
  control-shaping setting each tick (`FanEngine.SettingsSignature` — mode, zero
  snap, power knobs, per-channel tuning, both curves, sensor assignments; learned
  model values and kick/probe params deliberately excluded — a false positive
  jumps the fan). A changed fingerprint means a user edit landed (slider, curve
  drag, preset, mode switch), and every live `ResponseFilter` /
  `PowerBudgetController` gets a one-shot `ApplyNow()`: on that tick the new
  settings' verdict is adopted immediately — no step-down hold, no hysteresis
  carry-over, no slew glide, pending budget holds count as served and the
  one-step-per-window ramp brake lifts (before this an edit was read within ~1 s
  but *felt* only after the 25 s hold + ramp). Measurement state — averaging
  windows, brands, futility latch, relief, learned model — is untouched: it
  encodes evidence, not settings (the snap correctly stops at a still-branded
  level).
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
  machine — all clocks, loads, voltages, not just the ~57 the app exposes), default
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
  the raw "now" temp (amber dashed line on the chart, "now …°" in the card header)
  and **power draw as horizontal reference lines in the curve chart** — the mirror
  of the vertical raw-temp line: dashed
  faint = instantaneous draw, solid brighter = sustained average (the budget
  strip's trace vocabulary), read against a right-hand watts scale (0 W at the
  bottom, `BudgetChart.NiceWatts` ladder top — made internal for this — printed as
  a chip on the 100% gridline's right end). MainWindow feeds the selected
  channel's 10-min history peak into `CurveEditor.UpdateLive` so the scale agrees
  with the budget strip and doesn't breathe with every sample. Labels `draw NN W` /
  `avg NN W` seat at the lines' right ends (avg above its line, draw below — the
  ceiling/aim rule; merged into one chip when the lines sit within a label height),
  drawn LAST on the card colour; the raw-temp "now …°" label dodges the scale chip.
  Hidden when the channel has no power sensor or the mode is Temperature (Watts is
  null).
- **Undo/redo covers preset switches, not just curve points** (2026-07-30, Kuba's
  ask — a preset click used to be unrecoverable: it overwrites every curve AND every
  behaviour knob, and it cleared the undo stack on the way through). One stack in
  MainWindow (`Edit`, Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z) now carries two entry kinds:
  `CurveEdit` (per-channel point lists, as before) and `TuningEdit`, a pair of
  whole-profile `TuningSnapshot`s. The snapshot type lives in Core next to
  `Profile.CaptureTuning()` / `ApplyTuning()`, and `AdoptTuning(preset)` is now
  `ApplyTuning(preset.CaptureTuning())` — so what a preset overwrites and what an
  undo restores are one list that cannot drift apart. It carries curve + power curve
  + averaging/hysteresis/hold/slew/floor per channel and the profile name; sensor and
  header assignments (the machine) and the learned thermal model (measured on it) are
  deliberately outside it. Notes: record equality compares the point Lists by
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

- **BLOCKER 2026-08-05: Windows Smart App Control flipped from evaluation to ON
  overnight** (`HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy\
  VerifiedAndReputablePolicyState = 1`; CodeIntegrity 3033/3077 events from 01:11)
  and blocks EVERY unsigned locally built binary — the bin\Debug exe and DLLs, the
  installed `%LOCALAPPDATA%\Programs\FanCurves\FanCurves.exe`, `--sim` runs, all of
  it; SAC has no per-file exclusions by design. The dev loop and the app itself
  cannot run until SAC is turned off (Windows Security → App & browser control →
  Smart App Control — user-only GUI action, NOT re-enableable without an OS
  reinstall). Caught mid-deploy, so the app is DOWN and fans are on BIOS control
  until then; restart with `schtasks /Run /TN FanCurves` once SAC is off.

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
  controller input/output (raw/avg/trend temp, draw + sustained avg, out/target %,
  why-chip reason, headroom, demand, ceiling/aim, learned base/R/mass), daily
  rotation, 7 days kept. The `rpm` column is only `ControlIds[0]`; the trailing
  **`rpm_per_header`** column (`ChannelStatus.Rpms`, added 2026-07-29) carries
  EVERY assigned header's rpm `|`-joined in `ControlIds` order — a channel driving
  six headers can have one fan stalled or spinning up far slower than its
  siblings, and the single-header column made that invisible (it also read 0 for a
  whole day once because `ControlIds[0]` happened to be an empty header). The
  header line is written when the day's file is created, so a schema change needs
  the day's CSV deleted, not just a restart.
  Also written: `behavior.txt` (4 MB cap → `behavior-old.txt`) with
  CHANGES only: fan ON/OFF, target ladder steps, reason transitions (pure
  None↔Ramp flips suppressed — the target line already implies the ramp),
  driving/released flips, `·· <event>` markers (`App.Telemetry?.Event`, used for
  the learned-model reset), and a full settings line whenever any tuning knob,
  curve point or the control mode changes (snapshot diffed every tick — catches
  presets/sliders/edits with no per-handler wiring). Dev flows write
  `telemetry-sim-*.csv` / `behavior-sim.txt` (same rule as sensors.sim.txt).
  UTF-8 **with BOM** — Windows PowerShell 5.1 reads BOM-less UTF-8 as ANSI and
  mangles °/·/∞. Writers buffer (CSV flushed every 5 s, behavior per event);
  first failed write disables logging for the session — it must never take the
  engine down. Switchable since 2026-07-29: `Profile.TelemetryLoggingEnabled`
  (default true, dev-panel REVIEW LOGGING above BACKEND) gates the `engine.Ticked`
  hook in `App`; the toggle writes a `·· review logging on/off` marker and calls
  the new `TelemetryLog.Flush()` when switching off, so the ≤5 s of buffered CSV
  rows are not stranded until process exit.
- **Installed & verified on real hardware** (the X870 Steel Legend / 9950X3D
  build): exe at `%LOCALAPPDATA%\Programs\FanCurves\FanCurves.exe`, autostart
  task registered, PawnIO 2.2.0 (silent flags are `-install -silent`, dash-style —
  `/S` fails with exit 87). LhmBackend sees the NCT6686D: 8 controllable headers +
  all board temps; CPU temp from `/amdcpu/0/temperature/2` (Tctl/Tdie). The
  "Pump Fan" header (`/lpc/nct6686d/0/control/1`, physically the second NH-D15
  fan) is manually assigned to the CPU cooler channel.
