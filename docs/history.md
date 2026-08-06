# fan-curves — development history & harness logs

Dated narratives, default-change history and scratchpad-harness recaps extracted
from `CLAUDE.md` on 2026-07-29. The standing rules, contracts and gotchas these
events produced live in `CLAUDE.md`; this file records how and when they came to be.

## Layout

### History strip / budget strip evolution

- **History strip** added 2026-07-22 (to make the idle-kick / zero-snap cycling
  cadence readable); demoted to **Developer mode only** later the same day —
  collapsed in simple mode, the curve chart takes the freed space.
- Stopped-time spans: originally measured start-to-start; corrected to
  turn-OFF → next turn-ON after Kuba's correction later that day. An earlier
  gap-width guard silently dropped stop labels under ~1 min at dev-mode width —
  fixed after Kuba's report the same day (the rule became: skip a label only when
  it would overlap the previous span's label).
- **Budget ceiling** dotted reference added 2026-07-25; **sustained aim** dotted
  line added 2026-07-27 (its label seated BELOW the line so it never collides
  with the ceiling's when the margins put them a few pixels apart).
- **Budget strip** (`BudgetChart.cs`) added 2026-07-25. Since 2026-07-27 the
  headroom it plots is measured to the sustained aim, so it drains under a
  sustained fans-off load instead of pinning at ∞ near any under-ceiling
  equilibrium.
- **Headroom axis linear → logarithmic** (2026-07-26, after Kuba's "headroom is
  not decreasing while the avg temp rises"): the old linear axis capped at 3× the
  lead drew "3 hours", "20 min and falling" and "∞" as the same flat line at the
  top — a slow warm-up's drain from hours to minutes was invisible. Replaced with
  the 10 s → 30 min log scale.
- An under-fill below the headroom trace was tried and rejected — a healthy
  buffer pins the trace to the top of the scale and the fill looked like a solid
  block flooding the strip.
- Dev-window growth: 1010×660 → 1320×830 (to fit the second strip) → 1568×830 on
  2026-07-27 (for the two-column dev panel).
- **CLEAR text-button** added 2026-07-27 (Kuba's ask).
- Strip rendering verified by a scratchpad WPF harness that renders the strips to
  PNG from scripted histories (fuse span, hover chip, empty state, 420 px-wide
  window) — it pokes `StripChart._hoverX` by reflection, because `SetCursorPos`
  cannot hover a window that another window covers, and a 4 s `--sim` capture
  never reaches those states.
- **Scroll-back + wall-clock timestamps** added 2026-08-04 (Kuba's asks: "make
  the history graph scrollable, don't keep everything in memory" and "display
  time of the measurement, not only time from now"). `ChannelHistory` became
  two-tier (600-sample RAM ring + ~24 h binary spill file in `%TEMP%\FanCurves`,
  DeleteOnClose), both strips pan as one timeline through a shared
  `HistoryViewport` (wheel / drag / double-click / LIVE button), samples carry
  `DateTime Time`, and the axis + hover chips print wall-clock times. Records
  were doubles (89 B) for a few hours, then quantized to 26 B integers the same
  night (Kuba's ask, "use type that uses less memory") — tenths resolution,
  below anything the strips can display; only the disk is lossy, the ring keeps
  exact doubles. Storage
  and scroll math proved by a 25-check console harness referencing the built
  UI DLL (including a 175k-sample compaction run); interaction verified by the
  UIAutomation + SendInput harness against an 11-minute `--sim --dev` run.
- **Lightweight/fast pass** (2026-08-04, same night — Kuba: "optimize the whole
  way this app so it's very lightweight and fast"). Visible-window baseline was
  4.55 CPU-s/min (~7.6% of one core, LHM hardware polling included). Three
  changes (rules now in CLAUDE.md's render/tick performance entry): all ~83
  per-render `new Pen/SolidColorBrush/Typeface/DashStyle` sites across the four
  charts replaced with frozen statics built by the new `Paint.cs`; per-tick UI
  painting skipped entirely while the window is hidden in the tray or minimized
  (histories/tray/model-save keep running; `RefreshLiveUi()` catches up on
  reopen — before this the hidden app rebuilt hero/chips/charts/dev readouts
  every second for nobody); the title-bar spin animation capped from 60 to
  20 fps and the tray tooltip deduped. Charts verified pixel-identical by the
  WPF render harness (both strips), storage harness ALL PASS unchanged.

### Render-robustness incident (2026-07-27)

Kuba's resize crash: a mid-resize arrange pass handed `CurveEditor` a near-zero
size and its axis-chip `Math.Clamp` threw on an inverted range (min 16 > max
13.8 — ArgumentException), killing the process — which left the Super I/O frozen
at the last written PWM with nothing watching the die while a load pushed it
through 80°. Result: the render-robustness rules in `CLAUDE.md` (minimum-size
render bail, `Math.Max(lo, hi)` clamp guards, `App.DispatcherUnhandledException`
swallowing UI faults).

### Why-chip

Added 2026-07-22; classification verified by a scratchpad console harness
(ramp/hold/hysteresis/snap/floor scenarios) the same day.

### Dev-panel rework journal

- 2026-07-27 (Kuba's ask): the panel became two columns, 520 px wide; the
  CONTROL MODE switch moved to the top of the panel (Kuba's placement).
- 2026-07-28 (Kuba: "grouped in more clear way. Add checkboxes to enable/disable
  all the features"): settings regrouped into titled feature groups whose master
  checkbox is the group header; CHANNEL RESPONSE renamed from BEHAVIOUR; HARD
  OVERRIDE gathered the fuse's trigger + release knobs from POWER CONTROL and
  BUDGET INTERNALS; relief and the power floor got their first enable switches
  (`Profile.ReliefEnabled` / `Profile.PowerFloorEnabled`).
- 2026-07-29 (Kuba: "are there any more functions missing the on/off switch?"):
  switch inventory completed — the futility probe, the Auto floor guard, instant
  apply, review logging and High process priority each got a switch, and
  `LEARNED MODEL`'s existing checkbox became the group header like the rest.
  Same day SAFETY FLOOR became its own group — the floor slider moved out of
  CHANNEL RESPONSE, relabelled "Never below".
- Label collision: the ramp-lead label was shortened from "Ramp when headroom
  under", which had collided with its "1 min 30 s" value.
- The 45 %-dim-but-editable reachability rule dates to 2026-07-21 (Kuba:
  settings must be reachable with the checkbox off).
- Sensor-row alignment/font/wrapping (2026-07-24): rows lead with live temp/rpm
  since that day; the value column was right-aligned through 2026-07-24 and
  switched to left-aligned (Kuba wanted the checkbox-to-value gap smaller); the
  whole row went to Cascadia Mono 11.5 after Kuba's "use uniform font" the same
  day; later that day long names got wrapping and the lists' horizontal
  scrollers were removed (the shared CheckBox template's old horizontal
  StackPanel measured content at infinite width and silently disabled wrapping —
  its root became a Grid with a star content column).

### Design-language origin

The "precision instrument" language is the 2026-07-20 redesign; it grew out of
the speed-pill lookbook after "looks like 2012" feedback. Exactly-three window
sizes was Kuba's choice the same day. The dev-mode raw-temp dashed line and its
"now …°" label joined the amber set on 2026-07-27 — Kuba asked to
colour-differentiate the temp and wattage reference lines, so temperature became
the amber quantity and the power lines stayed monochrome.

## Behaviour contract

### Default profile

Kuba's hand-tuned settings were promoted to default first on 2026-07-21 and
re-promoted from his live profile on 2026-07-22.

### Stopped-fan kick

Default-on at 60 s / 20% / 20 s through 2026-07-21; **default false since
2026-07-22** (Kuba runs with the kick off). The editable-while-disabled rule is
Kuba's 2026-07-21 correction.

### Safety floor switch

Added 2026-07-29, Kuba's ask — "the same way other features have the turn on
turn off option". Verified by a scratchpad harness driving the real engine
against a fake backend: floor holds 30% cool → off stops the fan → curve's own
60% unaffected → back on restores → probe gated on/off with the switch.

### Zero snap

Added 2026-07-22; the threshold default was 30% and dropped to 20% later that
day. The "default curves" referenced by the no-band-snapped note are the
2026-07-22 re-promoted defaults.

### Stop probe

Added 2026-07-22. The continuous in-trial comparison is a superset of Kuba's
"recheck every 30 s" ask. State-machine scenarios verified by a scratchpad
console harness on 2026-07-22 (probe timing, in-trial rise + backoff, late rise
without backoff, unstable temps never probe). The 2026-07-22 defaults
re-promotion changed: stable band 2 → 3.5 °C, trial length 10 → 30 s, retry
backoff 300 → 60 s. "No trials above" (`StopProbeMaxTempC`) added 2026-07-27
(Kuba's ask), verified by a scratchpad harness the same day.

**Demand gate** (2026-07-27, from Kuba's chess-engine report): a CPU clamping
its own temperature reads perfectly "stable" at 180 W, and because the clamped
die can never "rise", an ungated trial held the fan at 0% at 178 W indefinitely
while the CPU throttled; harness C6 documents the latch.

### Thermal-budget control

Added 2026-07-24 ("control with power, not temperature").

**Headroom redefined 2026-07-27** (Kuba: "headroom is almost always at 100% no
matter what the temperature or the fan speed; it should almost only go down — up
without fans only when hot with no load"): the old pair (`E/surplus` +
slope-to-CEILING) read ∞ at any equilibrium under the ceiling — on this cooler,
nearly every real state — and even recovered to ∞ mid-warm-up as the climb
decelerated toward a hotter-than-wanted equilibrium with the fans still parked.
Replaced by the aim-referenced `TauSeconds` (guarded line, model + measured
prongs); headroom now recovers only when the fans step up or the load ends. The
dev-panel readout line was renamed from `lead` to `headroom` the same day.

**Prong gating redesigned later on 2026-07-27** (after Kuba's "temperature
rising and rising through 80° and the fans not kicking in"): the first cut gated
BOTH prongs on burst-quietness, which silenced the whole predictive layer under
a real game — a live draw never stops fluctuating (peak−avg ≈ 55 W for the
entire session), the gate never opened, headroom sat pinned at ∞ and only
reactive StepUpHold caught the climb — late, above the aim (harness S7
reproduces this with a realistic swinging load). The falling-trend veto
(slope < −0.01) was added later that day: the still-hot minute-average of a
just-ended load otherwise computed "seconds to the aim" while the die plunged
1°/s through it and fired a pointless ramp mid-decay — harness C7. The slope
prong's contamination is harness S3: an 8 s spike's die jump reads as a rush
toward the aim for as long as the fit can see it. Learning's long quiet
horizon: the 250 W spike train poisoned R(0) to ~0.78 through 30 s-"stable"
windows by pairing the still-elevated trend with trough power — a poisoned R is
what turns the ungated model prong into a false accuser; with honest learning
the margin alone keeps spike trains silent (150 W and 250 W verified: fans never
move, min tau 632 s). The wall-jump cost: a 188 W step is spike-indistinguishable
early on, fires at ~68° trend and peaks 80° die — the fuse stays the hard stop.

**One ramp step per slope window** (2026-07-26, from Kuba's "fans ramped
instantly to 100%"): the backward-looking measured slope re-firing every tick
used to climb the whole ladder in seconds. The gate-open brand exists because an
optimistically-wrong model otherwise argues the fan straight back down after
every predictive step — slow on/off hunt, harness S5.

**Step-down snap** (Kuba later on 2026-07-27: "after a step-down timer, snap to
the fan curve"): the old one-level-per-hold walk kept the fans dawdling ABOVE
the staircase for a hold per level after a load ended. Harness C10: 90→0 in one
snap, no intermediate levels, never below a branded level, slew still glides the
output. Draw-dimension hysteresis rationale: a constant load equilibrating
exactly AT the aim hunted 20↔40% across the ±HysteresisC temperature band, and a
game's minute-scale swells re-crossed it constantly (harness S7); stacking the
5/4 draw margin WITH the temperature band blocked the final step to silence at
warm idle (harness S5b).

**Upward aim enforcement** (2026-07-26, from Kuba's report "headroom dips
briefly, recovers to max, fans never kick in"): a settled equilibrium that a
frozen or unlearned model believed under the aim used to park a 90 W game at
~84° die with fans stopped forever, and a 188 W load at 86.1° — above its own
ceiling — showing "buffer 0.0 kJ · headroom ∞". Sustained forgiveness replaced
instant forgiveness on 2026-07-27 — instant forgiveness re-armed the descent on
every lull of a fluctuating load (harness S7).

**Model-poisoning incident (2026-07-26)**: found on Kuba's machine as base
52° / R flat ~0.08–0.10 on BOTH channels (the model claimed ~200 W dissipation
at 44 W draw → every energy-side prediction blind; partly legacy of the
pre-2026-07-25 bug where `--sim` runs saved sim-learned models into the real
profile). His profile was healed in place (learned values zeroed; backup
`profile.json.bak-2026-07-26` in `%AppData%\FanCurves\`). His margins —
`SteadyTargetMarginC` 20 (sustained aim 70°) and `RampLeadSeconds` 90 ("fans
should kick in much earlier"; ~70° die should mean fans, stated twice) — were
set the same day.

**Knob exposure**: 2026-07-25, Kuba: "add all the new parameters to be shown in
developer mode" — the former `private const` values in `PowerBudgetController`
became app-level profile settings with BUDGET INTERNALS sliders. 2026-07-27,
Kuba: "add separate line with slider options that are used in new algorithm" —
the `headroom: lead · ±band` readout line. That line was first committed as
concatenated interpolated strings (`Inv($"…" + $"…")`) and the tip didn't build
until the control-mode commit folded it back into a single `$"…\n…"` — origin of
the one-interpolated-string gotcha.

**Harness log**:

- **First harness (2026-07-24)**: burst immunity, predictive ramp, fuse with
  corrupt model, power step-down, C/R learning convergence — all verified.
- **Second harness (2026-07-25, the newly exposed knobs)**: a bigger ceiling
  margin ramps earlier and peaks cooler (first fan at 156/131/87 s for
  4/10/20 °C), a tighter aim settles higher (81/90/100 %), inverted margins
  clamp instead of misbehaving, extreme windows stay finite — but long
  trend/slope windows are the risky end: at 120 s/120 s the ramp waits until
  205 s and the die reaches 89.9 °, i.e. it only just stays under the fuse.
  (The OverrideTempC-below-90 gotcha was found in this harness's scenario 2.)
- **Third harness (2026-07-26, upward enforcement; sim-plant replica,
  deterministic)**: 90 W sustained now steps 0→20% ~8 min in and settles at ~71°
  die (was: parked at 84.2° with fans off forever); 188 W converges to 100%
  holding ~79.7° (the aim); 8 s/150 W spikes still never move the fans;
  learning-off behaves the same as learning-on thanks to the model-free prong +
  the brand; after the load ends the brand is forgiven and the fan winds back
  to 0.
- **Fourth harness (2026-07-27, the aim-referenced headroom; 10 scenarios by the
  end of the day)**, deterministic sim-plant replica, Kuba's margins (aim 70 /
  ceiling 86 / lead 90) plus a preset-defaults scenario: settled idle 44 W reads
  ∞ with fans off; a constant 90 W step gets fans at +70 s (trend 64°) and
  settles 65° at 40% with tau recovering ONLY on the fan steps; a 44→90 W creep
  over 8 min shows a 242 s visible countdown before firing at 66°; 150 W and
  250 W spike trains never move the fans (min tau 632 s, never near the
  trigger); 188 W reaches 100% in ~1 min peaking 80.1° die, no fuse; a frozen
  optimistic-corner model still gets fans via the measured prong and holds a
  stable 40% thanks to the ramp brand; preset defaults fire at 78.9° and settle
  70.7°; **S7, the report repro** — a realistic game (95 W mean swinging
  111↔167 W, bursts to ~155, Kuba's learned values, model reset mid-rise) —
  fires predictively at +58 s / trend 67° and then holds one steady 50% for ten
  minutes straight through the swings; S5b (90 W onto a warm 63° idle) fires at
  +56 s / 68.2° and the warm idle itself unwinds to fully-stopped fans.

**Control-mode default**: Power for its first day (2026-07-27), then Auto by
Kuba's call the same day.

### Futility edge + experiment latch

2026-07-27, Kuba's chess-engine report: fans at 100% "and I don't think it is
doing that much because the radiator is barely hot". His 9950X3D read 85.3° at
81, 90, 100% AND fans-off alike at ~180 W. Harness C1 caught the futility march
resuming at 81→90 exactly through the learning asymmetry the experiment latch
fixes (LearnSteady teaches only the visited anchor). The boost-clamp
observation — draw rose ~7 W per step all session at constant temp — motivated
counting fan-into-watts as futile. Net effect on his curve: 81% at avg 85.3°.
The `FutilityProbeEnabled` switch is from 2026-07-29; harness check: clamped die
at 85.3°/180 W peaks at 90% and settles back to 81% with the probe on, marches
to 100% with it off.

### Downward relief + power floor

Same night (2026-07-27), Kuba: "try lowering the speed; if the parameters stay
the same lower it, if they start rising raise the RPM" + "only if the sustained
load is below 190 W, add a slider" + "step-down hold applies to the first step
only, then instantly snap" — the downward-relief contract. Later that night,
Kuba: "200 sustained draw requires 80% fan, 100 requires 30%" — the power floor.
Both master checkboxes (`ReliefEnabled` / `PowerFloorEnabled`) date to
2026-07-28. With the power floor, the chess repro bottoms out ON the line (~64%
at 168 W — matching his original "lower it to 60%" better than the ladder's 50).

**Seventh harness (2026-07-27)**: C1 chess repro from his actual learned model +
margins in Auto (floor climb → one probe 81→90 → back, 25 s above 81%, then
relief settles ON the power-floor line at 64%, line never undercut); C2 90 W on
a fan-effective plant still fires and settles 40%/64°; C3 188 W from seeds on a
plant where 81% genuinely can't hold the preset aim still climbs to 90% (a
helping step's verdict clears — trend falls); C4 spike train silent; C5
floor-guard 40 W creep still one-steady-ON; C6 the ungated stop probe latching
0% at full load below the 78° probe ceiling (why the demand gate exists); C7
load-end during relief — no fan bump, unwinds to silence; C8 clamp shifting
85.3→88 mid-relief — floor restored in 17 s, no fuse; C9 draw over the relief
cap — floor never undercut; C10 post-load descent = one target snap +
power-floor glide, monotonic 90→0, fans off 94 s after load end.

### Power curve (Curve mode)

2026-07-27, Kuba: "can we create a curve for power consumption?" — the
direct-driver variant was chosen via AskUserQuestion. Verified by the **eighth
scratchpad console harness**: constant 180 W settles 65% steady; ±7 W swing
across a step edge never flaps; 8 s/250 W spike train at idle stays silent; fuse
latches instantly at raw 90° and releases clean; hot-but-low-draw lets the temp
floor win; empty power curve is tick-identical to the pure temp path; load-end
winds down to stopped in ~69 s. Editor render verified by a WPF harness PNG on
both axes.

### Control-mode switch + floor guard

Control-mode switch added 2026-07-27 (Kuba: "switch between temperature-based
and power-based mode" + "automatic option that considers both outputs"); default
was Power for its first day, Auto since Kuba's call later that day. The
inside-the-controller floor design is backed by harness A2's floor-blind
control: the outside-max design taught R(0) 0.55→0.35 by pairing 50 %-cooled
temps with the budget's parked 0%.

**Fifth harness (2026-07-27)**: A1 constant 90 W in Auto: budget target never
below the floor, predictive fire still at +69 s; A2 learning honesty + the
poisoned control; A3a sparse 150 W spikes: nothing moves; A3b dense spike train
whose 90 s average legitimately crosses the 57° step: the staircase's own 20 %
runs, power side silent at ∞ headroom — pure Temperature mode would do the same;
A4 `FloorPercent = 0` is tick-identical to the pre-change controller, so Power
mode is untouched. Plus a WPF reflection harness that clicks the switch (profile
follows, segments render; the `--screenshot` flow can't scroll the dev panel
that far down).

**Floor guard** (later on 2026-07-27, Kuba: "the power mode should take into
consideration that the heat buffer is being drove down by the temperature — add
this calculation to auto mode"): his telemetry that night showed the limit
cycle — ~37 W of desktop draw settles the die at 57–58°, exactly on the 57°
first step, so the floor cycled the fans on/off every 2–4 min all evening while
the budget read "headroom ∞ · demand 0" (model equilibrium 51° — optimistic, and
aim-referenced headroom only watches 70°). A model arm for the guard was tried
and cut the same day: one transient of ridge-inflated R(0)≈1.0 turned the 57°
line's small clearance into a false fire during a spike, and a model-eq floor
term in demand/step-down LATCHED a pessimistic model's fan on at true idle —
harness F4. The `FloorGuardEnabled` switch is from 2026-07-29.

**Sixth harness** (plant tuned to the telemetry: passive die eq ≈ 60° at 38 W):
old Auto 9 ON/OFF cycles per hour → guarded Auto one ON then steady 20 %, die
54.9°; true idle 26 W stays silent; draw-drop to 27 W stops the fan for good;
sparse 150 W spikes never move it; 90 W load parity with A1; a settled 40 W
creep fires the measured prong at +33 s so the budget glides in before the floor
ever rises (model-free — works from unlearned seeds); pure Power mode still
tolerates the same creep (58° < the 70° aim).

### Instant apply

2026-07-29, Kuba: "after a change is done in the developer settings, apply it
instantly". Motivating session: Kuba's 01:21–01:28 curve-editing session, where
the target followed a curve edit 27 s late. Verified by a scratchpad harness
(15 checks): filter snap down/up mid-hold and mid-slew, budget snap with output
jump, fingerprint stability against per-tick learned-value writes; harness C3:
the snap stopped at a still-branded level, correctly. The `InstantApplyEnabled`
switch is from the same day's switch inventory.

### Process priority High

Added 2026-07-29. A 32-core chess run starved the normal-priority UI thread
until Windows ghosted the window (blank → reappear), which read as "the app
closed and opened again"; the process never actually restarted (same PID across
the session — events.txt showed no startup line). Separate finding from that
night's logs: the 2026-07-28 14:10 reboot was an OS-level hard crash —
Kernel-Power 41, "restarted without clean shutdown" — not the app.

### `--sim` ReadOnly

Bug found 2026-07-25: every `--sim` run rewrote the real `profile.json` —
AutoAssign pruned the hardware IDs the sim backend doesn't know and saved
`sim/*` in their place. Auto-assignable headers heal on the next real launch,
but manual assignments do not — Kuba's Pump Fan header (never auto-assigned by
design) was silently lost that way. Fixed with `Profile.ReadOnly`.

## Status / open items

### Install verification (2026-07-23)

Installed & verified on real hardware 2026-07-23 (the X870 Steel Legend /
9950X3D build): v0.1.0 exe at `%LOCALAPPDATA%\Programs\FanCurves\FanCurves.exe`,
autostart task registered, PawnIO 2.2.0 installed. Control worked — the Quiet
preset stopped the CPU fans at idle within the first minute. Kuba manually
assigned the "Pump Fan" header (physically the second NH-D15 fan) to the CPU
cooler channel, as on the previous test setup.

### Chassis headers switched to DC — measured sweep (2026-07-30 ~02:00)

Kuba set **all four chassis headers to DC (voltage) mode** in the BIOS to test the
2026-07-29 hypothesis that the dead-at-low-duty header was one header left in DC.
Rebooted at 01:56; the case channel drove `control/2,/4,/5,/3` (four fans, all four
tachs alive). The header SET changed at 01:14:49 that night (was six headers with
fans on two), so only the same night's own before/after is comparable — the
2026-07-29 numbers refer to different headers.

Both eras' duty sweeps are hand-made: Kuba dragged the case curve points while
instant-apply snapped the output, so the telemetry is a break-away ladder.

Steady holds (duty unchanged ≥15 s), rpm per header:

| duty | PWM (01:14–01:47) | DC (01:56–02:06) |
|------|-------------------|------------------|
| ≤25% | all four stopped (25% held 5 min, all dead) | all four stopped |
| 26%  | —                 | stopped; `/5` crept to ~334 rpm once after 47 s, then died |
| 27%  | `/2 /4 /5` ~350 rpm, `/3` dead | stalls (part of a 42→32→27→26 glide) |
| 28%  | —                 | **all four ~570–630 rpm** |
| 30%  | —                 | commanded 22 s from a stop → never started |
| 35%  | —                 | all four ~890–950 |
| 37%  | all four ~960–1020 | — |
| 42–43% | all four, `/3` lagging 581 vs ~1120 | all four ~1109–1126, `/1` 592 mid-ramp |
| 45–50% | all four ~1150–1230 | all four ~1290–1390 |
| 100% | all four ~2320–2470 | — |

Break-away lag from a dead stop (time the duty had been held):

- PWM: 27% → `/2 /4 /5` at 14–15 s; `/3` first turned at 43% (9 s); 100% → 2–3 s.
- DC: 28% → 12–13 s; 35% → 3–5 s; 42% → staggered 1 / 9 / 12 s; 50% → 0–2 s.

So the long break-away lag is a **low-duty property in both modes** (14 s at 27% PWM
≈ 13 s at 28% DC), not a DC artifact. Spin-up to steady rpm at a fixed duty takes
~20 s in DC (298 → ~1300 rpm at 50%) — a fan crawling off stall, board-side voltage
ramp, or both; not distinguishable from this data.

Sustain failures in DC (fan already spinning, duty lowered): 50→14% died in 1 s,
28→24% in 1 s, and a 42→32→27→26% glide shed all four at 26% (`/5` lingered ~380 rpm
to 02:04:34).

CPU headers were NOT switched and are unchanged: at 20%, `CPU_FAN1` 331 rpm in 100%
of 271 steady samples, Pump Fan 0 rpm in 100%; at 40% both run (693/731).

Verdict: DC **cured the odd-header-out** (no header is the exception any more) and
lowered the all-four floor from 37% to 28%, but it **cost the quiet operating point**
— PWM held ~350 rpm at 27% on three headers, DC's slowest sustainable speed is
~600 rpm at 28%, because DC cannot run a P14 below its stall voltage while PWM
pulses full 12 V. Whether PWM's `/3` would have started somewhere in 30–37% was
never tested (the sweep jumped 27 → 43), so "43% floor in PWM" is a lower bound on
the untested range, not a measurement.

### Two low-duty header faults — full diagnosis (2026-07-29) and resolution (2026-07-30)

Moved here from CLAUDE.md when the faults were resolved; CLAUDE.md keeps the
condensed verdict and the enduring rules.

Both faults were diagnosed 2026-07-29 from the telemetry's `rpm_per_header`
column, and both reproduced under BIOS control — never an app bug.

(1) **The `CPU_FAN2/WP_3A` "Pump Fan" header could not hold a fan at the curve's
20% first step — proven header-side, not fan-side.** At a constant 20% the CPU
Fan header held its fan rock steady while the Pump Fan header alternated
0 rpm ↔ ~370 rpm every 10–60 s; both headers got the SAME duty from one channel.
**Swap test 2026-07-29 16:40** (the two NF-A15 exchanged headers): the rpm
signatures traded (CPU_FAN1 355→330, Pump 345→370) while the dropouts STAYED on
the Pump header — post-swap, at duty>0, CPU_FAN1 read zero in 1% of samples and
the Pump header in 64%. Both fans are good. Steady and matched from 40% up
(711/700) and at 100% (1544/1564) — a low-duty limit of that header. **The
header was healthy the same morning** (share of 20%-duty samples reading 0 rpm,
per hour: h04 0% and h05 0% over 3556 samples, then h14 68%, h15 98%, h16 67%,
h17 74%) and only went flaky from 14:12 on — after the BIOS session in which
every header was set to PWM output — so a setting on that header was a likelier
cause than a dying board. When it did turn it turned at the normal ~355–380 rpm,
so the duty reaching the fan was right; it was the drive that lapsed. (Fallback
fix if it had been the board: Noctua Y-cable, both NH-D15 fans off CPU_FAN1 —
~0.25 A, fine on a 1 A header; costs the second tach.)

(2) **`System Fan #1` (`/lpc/nct6686d/0/control/2`) was dead below ~40% duty.**
At 5% two case fans turned ~423 rpm and the third read 0; at 20% it was
705/706/0; only from 53% did all three run (1454/1435/1368), matching at 100%
(2366/2352/2398).

**Both faults were SUSTAIN failures, not start-up failures** (established from
ramp-down episodes in the telemetry — the break-away test for free): a fan
already spinning STALLED within one second of the duty reaching ~20%, while
identical fans on sibling headers held ~700 rpm there. Case channel
04:10:49→51, 53%→23%→20%: `1142|1116|…|1225` → `778|744|…|0`. CPU channel
14:09:14→15, 23%→20%: Pump `696` → `0`; same at 14:46:12, 16:23:55, 18:00:03.
Conversely the healthy headers **started these same fans from a dead stop at
5%** (17:36:56 0%→5%: two fans broke away within 2 s and settled at ~420) — so
no fan on this board needs a 20%+ "minimum duty", stiction was not the
mechanism, and no BIOS minimum-duty / PWM-gate value could fix a header that
cannot hold a turning fan. (A BIOS minimum-duty setting only governs the BIOS's
own control loop anyway — LHM writes the duty register directly, bypassing it
entirely while the app drives.) The remaining candidate was the header's drive
MODE: in DC mode "20%" is ~2.4 V, below stall — exactly the observed behaviour
and exactly the ~40–53% (≈5–6 V) break-away. The DC-mode sweep above confirmed
it: every header delivers exactly what its configured mode predicts, and DC
made all four chassis headers behave identically — which a failing header
cannot be argued into. Likely trigger: ASRock's **Auto** drive mode re-probing
3-pin vs 4-pin at each boot and guessing DC (explains the header measuring
perfect at 04:00 and flaky from 14:12 after a BIOS session, the culprit
appearing to move between sessions, and the Pump header surviving a "set
everything to PWM" pass — the water-pump header has its own menu entry).

**Resolution (2026-07-30, verified over the following morning).** After the DC
sweep, every header was set explicitly to PWM in the BIOS (~02:45; app session
from 02:51:49). Telemetry 03:00–11:53 (63 112 rows): Pump Fan header **0%
dropouts in 28 336 samples at 20% duty** (~371 rpm beside CPU_FAN1's ~334; the
previous afternoon it read 0 rpm in 64–98% of same-duty samples), and **all
four chassis headers statistically identical at every duty** — same stop/spin
pattern and rpm at 5/6/10% (~424 rpm when turning), no odd header out. Also
confirmed on PWM: an already-spinning P14 sustains rotation down to ~5% duty
(the 5%-duty samples split all-four-spinning vs all-four-stopped, i.e. history
dependence, not header variance), while from a dead stop nothing starts below
break-away (~27%).

## 2026-08-06 — power/budget control removed; the app is temperature-only again

Kuba's ask: "Remove all modes except for the temperature mode." Everything the
power side had grown since 2026-07-25 was deleted wholesale rather than hidden —
the modes ceased to exist, and `ResponseFilter` on the temperature staircase is
once again the only control path (plus the courtesy features it always had:
safety floor, zero snap, stop probe, idle kick, instant apply).

Removed (code lives in git history before this date):

- **`PowerBudget.cs`** — `ThermalModel` (learned mass / R anchors / base, with
  the learning guardrails) and `PowerBudgetController` (thermal-budget control:
  heatsink credit, aim-referenced headroom with model + slope prongs, StepUpHold
  branding with sustained forgiveness, the futility probe + experiment latch,
  downward relief, the power floor line, the Auto floor guard, and the
  hard-override fuse).
- **Power-curve ("Curve") mode** — the watts staircase (`PowerPoint`,
  `ChannelConfig.PowerPoints`, `FanCurve.FromPower` + the axis-generic bounds),
  its `ResponseFilter` instance in the watts dimension, the per-channel
  `FuseState` in the engine, and `CurveEditor`'s power axis + `CURVE °C/W`
  toggle.
- **The CONTROL MODE switch** (`ControlMode` enum, the `PowerControlEnabled`
  JSON bridge) and every power profile setting: averaging/hysteresis/lead,
  override + release, ceiling/aim margins, trend/slope/now windows, learning,
  futility, floor guard, relief, power floor.
- **UI**: the dev panel's right column (POWER CONTROL, AUTO FLOOR GUARD,
  FUTILITY PROBE, DOWNWARD RELIEF, POWER FLOOR, HARD OVERRIDE, BUDGET
  INTERNALS, LEARNED MODEL + reset button) — the panel is single-column 300 px
  again and the dev window shrank 1568×830 → 1336×830; the budget strip
  (`BudgetChart.cs`) — the history strip is the only strip, and the freed row
  goes to the curve editor; the ceiling/aim reference lines in the history
  strip; the power reference lines + right-hand watts scale in the curve chart;
  the power-sensor SOURCES list; the why-chip reasons StepUpHold / BudgetHold /
  BudgetRamp / HardOverride / PowerCurve.
- **Telemetry**: the CSV dropped trend_c, watts, watts_avg, budget_kj,
  headroom_s, demand_pct, ceiling_c, aim_c, slope_cps, base_c, r_cpw, mass_jpc
  (12 of 24 columns); the settings line dropped the power tokens; the day's
  real CSV was deleted so the header matches (documented schema-change rule).
  The history spill record shrank 26 → 10 bytes (~0.9 MB/channel/day).
- **Backends**: power sensors are no longer enumerated (LhmBackend's
  `SensorType.Power` case, the sim's `sim/cpu-pwr`/`sim/gpu-pwr`); AutoAssign
  no longer wires package power. The sim keeps its honest thermal plant — only
  the sensors went.
- The stop probe's power-demand gate went with `DemandLevel`; its protection
  against trial-stopping a die-limited load falls to the "No trials above"
  temperature ceiling (default 78°, well under a clamped die), which is exactly
  what Temperature mode relied on before this change too.

One deliberate tuning consequence: the Quiet preset's CPU staircase used to cap
at 50% ("sustained-load speed comes from the power side in Auto"). With the
power side gone that cap would be the ceiling forever, so the preset grafts the
original v0.1.0 temperature-only top steps above Kuba's hand-tuned low/mid
bands: 20:0 · 50:10 · 55:20 · 62:40 (his) + 70:50 · 76:65 · 84:81 · 88:90 ·
92:100 (v0.1.0). Kuba's live profile.json was not touched — at removal time it
held a hand-made high-duty test curve (90% from ~21–24° avg, floors/snap/probe
off), presumably part of the chassis-header EC investigation.

Saved profiles keep loading: only fields were deleted, none renamed, so the
orphaned power values are ignored and dropped on the next save.

## Rust port — phase 1: core + parity (2026-08-06)

Kuba's decision after a resource-usage review: port the app to Rust, with the
goal of minimal footprint and a universal (Windows + Linux) build. Target
architecture from that discussion: a small headless engine daemon per OS plus
an on-demand egui UI, native hardware backends (PawnIO + NCT6686D on Windows,
hwmon/sysfs on Linux) replacing LibreHardwareMonitorLib eventually. The
same-day temperature-only removal shrank the port surface considerably — the
thermal-budget controller no longer needed porting.

Phase 1 landed the `rust/` cargo workspace: `fan-core` (curve, filter, kick,
probe, profile, engine, backend trait, sim plant — one module per C# file) and
`fan-daemon` (jittered tick loop, auto-assign port, BIOS handback on drop).
Parity with C# is enforced by `rust/parity-harness`, which drives the REAL
FanCurves.Core through deterministic scenarios (~5,500 ticks: spikes, band
descents, zero-snap bands, apply-now jumps, probe fail/backoff/max-temp,
kick cycles) and writes golden CSVs that `cargo test` replays through the port
— matching at 1e-9, in practice bit-identical. First run: 29 unit + 6 golden
tests green; the daemon loaded the real profile.json unchanged.

Toolchain installed the same day (rustup 1.29 + VS Build Tools 17.14 via
winget). Smart App Control had been switched off earlier that morning, which
unblocked local builds generally.

## Rust port — phase 2: telemetry + IPC (2026-08-06)

The daemon became a complete headless engine. TelemetryLog ported with the
C# schema, number formats ("0.#"/"0.0"/"0" trims) and behavior vocabulary kept
identical — verified live when the Rust daemon appended rows to the same
telemetry-sim CSV a C# --sim run had created that morning, header untouched.
Local-time offset is captured once at startup (std has no timezone database;
the `time` crate's lookup is only safe pre-threads), so a DST flip mid-run
shifts log timestamps until restart — accepted, documented.

IPC: local socket `fan-curves-daemon.sock` (named pipe on Windows) speaking
line-delimited JSON — ping/status/profile/set_profile/preset/apply/pause/
shutdown. Binding the socket doubles as the single-instance check, and
`shutdown` is the daemon's exit.signal. `--send` makes the binary its own
client. Verified end-to-end: preset adoption over IPC instant-applied within a
tick (Quiet → Performance, CPU 0% → 45% at the same temp), pause released to
BIOS, shutdown exited cleanly with the "headers to BIOS" marker flushed.
Read-only guard confirmed: preset via IPC on the real profile answered
"saved":false. Footprint with IPC + telemetry live: 5.6 MB working set / 1.0 MB
private. Service/autostart wiring deferred to the hardware-backend phase.
