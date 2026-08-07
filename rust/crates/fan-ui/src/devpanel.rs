//! Developer mode: the WPF dev panel ported to egui.
//!
//! Same organising idea as the original — a single narrow column of titled
//! groups where **the master checkbox IS the group header**, explanations live
//! in tooltips, and a group whose switch is off dims but STAYS EDITABLE
//! (settings must be reachable with the checkbox off).
//!
//! Every widget edits the UI's draft profile; `App` pushes the draft to the
//! daemon whenever it changed, and the engine's settings fingerprint
//! instant-applies it on the next tick — the same path a WPF slider takes.

use eframe::egui::{
    Align, Checkbox, Color32, CornerRadius, FontId, Frame, Layout, Margin, Response, RichText,
    ScrollArea, Slider, Stroke, Ui,
};

use fan_core::Profile;

use crate::client::Inventory;
use crate::{AMBER, CARD, DIM, FAINT, HAIRLINE, TEXT};

pub const PANEL_WIDTH: f32 = 300.0;

/// Averaging window slider: a notch index, not seconds — 5 s steps up to
/// 2 min, then 30 s steps to 5 min (port of `MainWindow.AvgNotchToSeconds`),
/// so the useful low end is not squeezed into the first tenth of the track.
pub fn avg_notch_to_seconds(notch: i32) -> f64 {
    if notch <= 24 {
        (notch * 5) as f64 // 0 … 120 s
    } else {
        (120 + (notch - 24) * 30) as f64 // 150 … 300 s
    }
}

pub fn avg_seconds_to_notch(secs: f64) -> i32 {
    if secs <= 120.0 {
        (secs / 5.0).round() as i32
    } else {
        24 + ((secs - 120.0) / 30.0).round() as i32
    }
}

const AVG_MAX_NOTCH: i32 = 30; // 5 min

/// Sensor-history notches in MINUTES: off · 5/10/15/30 min · 1/2/4/8/12/24 h.
const HISTORY_MINUTES: [f64; 11] =
    [0.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 240.0, 480.0, 720.0, 1440.0];

/// Port of `MainWindow.FormatAvg`: "45 s" under a minute, "2 min" on the
/// minute, "1 min 30 s" otherwise. Used for every duration knob so the panel
/// reads the same as the WPF one.
fn format_duration(secs: f64) -> String {
    let s = secs.round() as i64;
    if s < 60 {
        format!("{s} s")
    } else if s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

fn history_label(minutes: f64, sensor_count: usize) -> String {
    if minutes <= 0.0 {
        return "off".into();
    }
    let dur = if minutes < 60.0 {
        format!("{minutes:.0} min")
    } else {
        format!("{:.0} h", minutes / 60.0)
    };
    // The library appends a value roughly every 4 s, 16 B per entry.
    let mb = sensor_count as f64 * (minutes * 60.0 / 4.0) * 16.0 / (1024.0 * 1024.0);
    if mb < 1.0 {
        format!("{dur} ≈ <1 MB")
    } else {
        format!("{dur} ≈ {mb:.0} MB")
    }
}

fn micro(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(FontId::proportional(9.5)).color(FAINT));
}

/// Label on the left, value hard right, on ONE line. Must be inside a
/// `horizontal` scope: a right-to-left layout in a vertical Ui claims all the
/// remaining height and stretches the whole group.
fn label_value_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        micro(ui, label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).font(FontId::monospace(10.5)).color(TEXT));
        });
    });
}

/// Letter-spaced micro-label, the panel's header style ("S A F E T Y  F L O O R").
pub fn tracked(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for (i, c) in text.chars().enumerate() {
        if i > 0 {
            out.push(if c == ' ' { ' ' } else { '\u{2009}' });
        }
        out.push(c);
    }
    out
}

/// A titled group. `switch` makes the header a master checkbox; the body dims
/// to 45 % when the switch is off but stays interactive.
///
/// No frame of its own: the whole panel is ONE card (as in the WPF app), and
/// per-group boxes would read as eleven stacked panels instead of one.
fn group<R>(
    ui: &mut Ui,
    title: &str,
    tooltip: &str,
    switch: Option<&mut bool>,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let enabled = switch.as_ref().map(|s| **s).unwrap_or(true);
    let header = RichText::new(tracked(title))
        .font(FontId::proportional(9.5))
        .color(if enabled { TEXT } else { DIM });
    match switch {
        Some(flag) => {
            let r = ui.add(Checkbox::new(flag, header));
            if !tooltip.is_empty() {
                r.on_hover_text(tooltip);
            }
        }
        None => {
            ui.label(header);
        }
    }
    ui.add_space(5.0);
    let out = ui
        .scope(|ui| {
            // Dim, don't disable — settings stay reachable with the switch off.
            if !enabled {
                ui.set_opacity(0.45);
            }
            body(ui)
        })
        .inner;
    ui.add_space(14.0);
    out
}

/// How a knob's value is written out.
enum Fmt {
    /// Whole number plus a unit, e.g. "25 %".
    Unit(&'static str),
    /// One optional decimal plus a unit, e.g. "1.5 °C", "9 %/s".
    Decimal(&'static str),
    /// "45 s" / "2 min" / "1 min 30 s" — the WPF FormatAvg.
    Duration,
}

fn render(value: f64, fmt: &Fmt) -> String {
    match fmt {
        Fmt::Unit(u) => format!("{value:.0}{u}"),
        Fmt::Decimal(u) => {
            if (value - value.round()).abs() < 0.05 {
                format!("{value:.0}{u}")
            } else {
                format!("{value:.1}{u}")
            }
        }
        Fmt::Duration => format_duration(value),
    }
}

/// Slider row: label + value on one line, slider under it. `step` snaps like
/// the WPF sliders' TickFrequency + IsSnapToTickEnabled.
fn knob(
    ui: &mut Ui,
    label: &str,
    tooltip: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    step: f64,
    fmt: Fmt,
) -> bool {
    label_value_row(ui, label, &render(*value, &fmt));
    let before = *value;
    let r = ui.add(Slider::new(value, range).step_by(step).show_value(false));
    ui.add_space(3.0);
    if !tooltip.is_empty() {
        r.on_hover_text(tooltip);
    }
    (*value - before).abs() > f64::EPSILON
}

/// What a frame of the panel changed. `tuning` means the edit belongs to the
/// profile's tuning (curve/response/floor) and therefore renames it "Custom" —
/// app-level switches deliberately do not (presets don't touch them either).
#[derive(Default)]
pub struct Edited {
    pub any: bool,
    pub tuning: bool,
}

impl Edited {
    fn app(&mut self, changed: bool) {
        self.any |= changed;
    }
    fn tuning(&mut self, changed: bool) {
        self.any |= changed;
        self.tuning |= changed;
    }
}

/// Draw the panel. Returns what changed (the caller pushes to the daemon).
pub fn draw(
    ui: &mut Ui,
    profile: &mut Profile,
    channel: usize,
    inventory: &Inventory,
    daemon_version: &str,
) -> Edited {
    let mut edited = Edited::default();
    let changed = &mut edited;
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // Fit INSIDE the card: the caller fixed the card's content width, and
        // any wider set_width here silently widens the whole card — Frame
        // sizes itself to its child and ScrollArea does not clip the cross
        // axis, which is how the panel's right border ended up off-window.
        ui.set_width(ui.available_width());
        // Full-width slider tracks — egui's default is a fixed ~100 px stub.
        ui.spacing_mut().slider_width = ui.available_width();

        // ---- CHANNEL RESPONSE — the one group with no switch: following the
        // curve at all has no on/off, and its knobs can be flattened one by one.
        if let Some(ch) = profile.channels.get_mut(channel) {
            let title = format!("CHANNEL RESPONSE · {}", ch.name.to_uppercase());
            let c = group(ui, &title, "", None, |ui| {
                let mut c = false;

                let mut notch = avg_seconds_to_notch(ch.averaging_seconds);
                label_value_row(ui, "Averaging window", &format_duration(avg_notch_to_seconds(notch)));
                let r = ui.add(Slider::new(&mut notch, 0..=AVG_MAX_NOTCH).show_value(false));
                ui.add_space(3.0);
                r.on_hover_text(
                    "The curve is driven by the ROLLING AVERAGE over this window, not the live \
                     temperature - the longer it is, the less a short spike can move the fans.",
                );
                let secs = avg_notch_to_seconds(notch);
                if (secs - ch.averaging_seconds).abs() > f64::EPSILON {
                    ch.averaging_seconds = secs;
                    c = true;
                }

                c |= knob(ui, "Step-down hysteresis",
                    "A step down only fires once (average + this) still maps to the lower step - no flapping at a band edge.",
                    &mut ch.hysteresis_c, 0.0..=8.0, 0.5, Fmt::Decimal(" \u{b0}C"));
                c |= knob(ui, "Step-down hold",
                    "The lower band must hold this long before the fan is allowed to step down.",
                    &mut ch.step_down_hold_seconds, 0.0..=60.0, 5.0, Fmt::Unit(" s"));
                c |= knob(ui, "Ramp up",
                    "Fastest the commanded % may RISE - this is what makes a ramp audible-smooth instead of stepped.",
                    &mut ch.slew_up_percent_per_sec, 0.5..=20.0, 0.5, Fmt::Decimal(" %/s"));
                c |= knob(ui, "Ramp down",
                    "Fastest the commanded % may FALL.",
                    &mut ch.slew_down_percent_per_sec, 0.5..=10.0, 0.5, Fmt::Decimal(" %/s"));
                c
            });
            changed.tuning(c);
        }

        // ---- SAFETY FLOOR (the switch is app-level; the value is tuning)
        let mut floor_on = profile.safety_floor_enabled;
        let floor_value_changed = group(ui, "SAFETY FLOOR", "", Some(&mut floor_on), |ui| {
            let mut c = false;
            if let Some(ch) = profile.channels.get_mut(channel) {
                c = knob(ui, "Never below",
                    "The engine never writes less than this for the selected channel. With the group off the fans may go all the way to a stop, and the channel becomes trial-stoppable.",
                    &mut ch.min_percent, 0.0..=60.0, 5.0, Fmt::Unit(" %"));
            }
            c
        });
        changed.tuning(floor_value_changed);
        changed.app(floor_on != profile.safety_floor_enabled);
        profile.safety_floor_enabled = floor_on;

        // ---- STOPPED-FAN KICK
        let mut kick_on = profile.idle_kick_enabled;
        let kick_changed = group(ui, "STOPPED-FAN KICK", "", Some(&mut kick_on), |ui| {
            let mut c = false;
            c |= knob(ui, "Kick after stopped for",
                "How long a driven channel must sit at 0 % before it is briefly spun up. All channels.",
                &mut profile.idle_kick_stopped_seconds, 10.0..=300.0, 10.0, Fmt::Duration);
            c |= knob(ui, "Kick speed", "PWM % written while the kick runs.",
                &mut profile.idle_kick_percent, 5.0..=100.0, 5.0, Fmt::Unit(" %"));
            c |= knob(ui, "Kick duration", "How long each kick lasts.",
                &mut profile.idle_kick_seconds, 5.0..=60.0, 5.0, Fmt::Unit(" s"));
            c
        });
        changed.app(kick_changed || kick_on != profile.idle_kick_enabled);
        profile.idle_kick_enabled = kick_on;

        // ---- STOP INSTEAD OF SLOW (zero snap)
        let mut snap_on = profile.zero_snap_enabled;
        let snap_changed = group(ui, "STOP INSTEAD OF SLOW", "", Some(&mut snap_on), |ui| {
            knob(ui, "Stop below",
                "Any curve target above 0 % but under this runs the fan at 0 % instead - meaningful speed or fully stopped, never a slow crawl. All channels.",
                &mut profile.zero_snap_percent, 1.0..=50.0, 1.0, Fmt::Unit(" %"))
        });
        changed.app(snap_changed || snap_on != profile.zero_snap_enabled);
        profile.zero_snap_enabled = snap_on;

        // ---- TRIAL STOPS (stop probe)
        let mut probe_on = profile.stop_probe_enabled;
        let probe_changed = group(ui, "TRIAL STOPS", "", Some(&mut probe_on), |ui| {
            let mut c = false;
            c |= knob(ui, "Steady running before trial",
                "After a fan has run this long with a stable temperature, it is stopped as a trial - if the temperature climbs, curve control resumes at once. All channels.",
                &mut profile.stop_probe_run_seconds, 10.0..=300.0, 10.0, Fmt::Duration);
            c |= knob(ui, "Trial stop length",
                "A stop that dies within this (plus one steady period) counts as failed and arms the retry backoff.",
                &mut profile.stop_probe_seconds, 5.0..=60.0, 5.0, Fmt::Duration);
            c |= knob(ui, "Stable band \u{b7} rise to resume",
                "The temperature must stay inside this band to earn a trial; a rise beyond it while stopped resumes the fan.",
                &mut profile.stop_probe_stable_range_c, 0.5..=5.0, 0.5, Fmt::Decimal(" \u{b0}C"));
            c |= knob(ui, "Retry after failed trial",
                "Wait this long after a failed trial, so a loaded fan never settles into an on/off cycle.",
                &mut profile.stop_probe_retry_seconds, 60.0..=900.0, 30.0, Fmt::Duration);
            c |= knob(ui, "No trials above",
                "Raw-temperature ceiling: a channel hotter than this is never trial-stopped, and a running trial resumes the moment the temperature crosses it.",
                &mut profile.stop_probe_max_temp_c, 50.0..=90.0, 1.0, Fmt::Unit(" \u{b0}C"));
            c
        });
        changed.app(probe_changed || probe_on != profile.stop_probe_enabled);
        profile.stop_probe_enabled = probe_on;

        // ---- INSTANT APPLY (bare header checkbox)
        let mut instant = profile.instant_apply_enabled;
        if ui
            .add(Checkbox::new(&mut instant, RichText::new(tracked("INSTANT APPLY")).font(FontId::proportional(9.5)).color(TEXT)))
            .on_hover_text(
                "A setting you change here is adopted on the very next tick: no step-down hold to \
                 wait out, no slew glide - the fan jumps to what the new setting asks for. Off, the \
                 edit is read at once but only FELT after the normal holds and ramps.",
            )
            .changed()
        {
            profile.instant_apply_enabled = instant;
            changed.app(true);
        }
        ui.add_space(10.0);

        // ---- SOURCES
        group(ui, "SOURCES", "", None, |ui| {
            micro(ui, "Temperature sensors \u{2014} max wins");
            ui.add_space(3.0);
            let mut sensor_rows = 0;
            for s in inventory.sensors.iter().filter(|s| !s.is_rpm) {
                sensor_rows += 1;
                let mut on = profile.channels.get(channel)
                    .is_some_and(|c| c.sensor_ids.iter().any(|i| *i == s.id));
                ui.horizontal_top(|ui| {
                    if ui.add(Checkbox::without_text(&mut on)).changed() {
                        if let Some(ch) = profile.channels.get_mut(channel) {
                            if on {
                                ch.sensor_ids.push(s.id.clone());
                            } else {
                                ch.sensor_ids.retain(|i| *i != s.id);
                            }
                            // Assignment describes the machine, not the tuning,
                            // so it must NOT rename the profile "Custom".
                            changed.app(true);
                        }
                    }
                    ui.label(RichText::new(s.value.map_or("     \u{2014}".into(), |v| format!("{v:>5.1}\u{b0}")))
                        .font(FontId::monospace(10.5))
                        .color(if s.value.is_some() { TEXT } else { FAINT }));
                    ui.label(RichText::new(&s.name).font(FontId::monospace(10.5)).color(DIM));
                });
            }
            if sensor_rows == 0 {
                micro(ui, "No temperature sensors found.");
            }

            ui.add_space(9.0);
            micro(ui, "Fan headers driven");
            ui.add_space(3.0);
            if inventory.controls.is_empty() {
                micro(ui, "No controllable headers found.");
            }
            for c in &inventory.controls {
                let mut on = profile.channels.get(channel)
                    .is_some_and(|ch| ch.control_ids.iter().any(|i| *i == c.id));
                let owner = profile.channels.iter()
                    .position(|ch| ch.control_ids.iter().any(|i| *i == c.id));
                let elsewhere = owner.filter(|i| *i != channel);
                ui.horizontal_top(|ui| {
                    if ui.add(Checkbox::without_text(&mut on)).changed() {
                        // A header obeys ONE PWM value: assigning it takes it
                        // away from whichever channel held it.
                        profile.assign_control(channel, &c.id, on);
                        changed.app(true);
                    }
                    ui.label(RichText::new(c.value.map_or("      \u{2014}".into(), |v| format!("{v:>5.0} r")))
                        .font(FontId::monospace(10.5))
                        .color(if c.value.is_some() { TEXT } else { FAINT }));
                    ui.label(RichText::new(&c.name)
                        .font(FontId::monospace(10.5))
                        .color(if elsewhere.is_some() { FAINT } else { DIM }));
                });
                if let Some(i) = elsewhere {
                    if let Some(other) = profile.channels.get(i) {
                        ui.label(RichText::new(format!("         driven by \u{201c}{}\u{201d}", other.name))
                            .font(FontId::proportional(9.0))
                            .color(FAINT));
                    }
                }
            }
        });

        // ---- app plumbing
        let mut logging = profile.telemetry_logging_enabled;
        if ui
            .add(Checkbox::new(&mut logging, RichText::new(tracked("REVIEW LOGGING")).font(FontId::proportional(9.5)).color(TEXT)))
            .on_hover_text(
                "The review log: one CSV row per channel per tick (every controller input and \
                 output, daily file, 7 days kept) plus behavior.txt, which records only the \
                 changes - fan on/off, target steps, reason transitions and a full settings line \
                 whenever a knob moves. Off writes nothing.",
            )
            .changed()
        {
            profile.telemetry_logging_enabled = logging;
            changed.app(true);
        }
        let mut priority = profile.high_priority_enabled;
        if ui
            .add(Checkbox::new(&mut priority, RichText::new(tracked("HIGH PROCESS PRIORITY")).font(FontId::proportional(9.5)).color(TEXT)))
            .on_hover_text(
                "Run the daemon at High priority so a fully loaded CPU cannot starve the engine \
                 tick. Costs a few % of one core.",
            )
            .changed()
        {
            profile.high_priority_enabled = priority;
            changed.app(true);
        }
        ui.add_space(10.0);

        // ---- SENSOR HISTORY
        group(ui, "SENSOR HISTORY", "", None, |ui| {
            let current_minutes = profile.sensor_history_hours * 60.0;
            let mut idx = HISTORY_MINUTES
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 - current_minutes).abs().total_cmp(&(b.1 - current_minutes).abs())
                })
                .map(|(i, _)| i as i32)
                .unwrap_or(0);
            let shown = HISTORY_MINUTES[idx.clamp(0, HISTORY_MINUTES.len() as i32 - 1) as usize];
            label_value_row(ui, "Keep sensor readings for",
                &history_label(shown, inventory.sensors.len()));
            let r = ui.add(Slider::new(&mut idx, 0..=(HISTORY_MINUTES.len() as i32 - 1)).show_value(false));
            r.on_hover_text(
                "The hardware layer keeps a rolling history of every value it reads, for every \
                 sensor it tracks internally - not just the ones listed above. This app never \
                 reads it; it only costs RAM. Off (the default) stops the growth.",
            );
            let minutes = HISTORY_MINUTES[idx.clamp(0, HISTORY_MINUTES.len() as i32 - 1) as usize];
            let hours = minutes / 60.0;
            if (hours - profile.sensor_history_hours).abs() > f64::EPSILON {
                profile.sensor_history_hours = hours;
                changed.app(true);
            }
        });

        // ---- BACKEND (info only)
        group(ui, "BACKEND", "", None, |ui| {
            let info = |ui: &mut Ui, text: String, color: Color32| {
                ui.label(RichText::new(text).font(FontId::monospace(9.5)).color(color));
            };
            info(ui, inventory.backend.clone(), DIM);
            info(ui, format!("daemon v{daemon_version}"), FAINT);
            info(ui, inventory.config_path.clone(), FAINT);
            if inventory.read_only {
                info(ui, "profile is READ-ONLY - edits apply live, nothing is saved".into(), AMBER);
            }
            if inventory.simulated {
                info(ui, "simulation - no real fans are being driven".into(), AMBER);
            }
        });
    });
    edited
}
