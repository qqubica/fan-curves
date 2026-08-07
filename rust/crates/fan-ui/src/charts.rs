//! Custom-drawn charts — the egui `Painter` translation of the WPF
//! `OnRender`/`DrawingContext` code. Same visual grammar: card surface with a
//! hairline, sparse horizontal grid, glow-under white staircase, and amber
//! reserved for live thermal state (operating dot, raw-temp dashed vertical).

use eframe::egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::client::HistorySample;
use crate::{Snapshot, AMBER, CARD, DIM, FAINT, HAIRLINE, TEXT};

const TEMP_MIN: f64 = 15.0;
const TEMP_MAX: f64 = 100.0;
// Left/right padding is identical on both charts ON PURPOSE, so the curve and
// the history strip line up vertically (the WPF layout's intent).
const PAD_L: f32 = 42.0;
const PAD_R: f32 = 18.0;
const PAD_T: f32 = 16.0;
const PAD_B: f32 = 30.0;
const STRIP_PAD_T: f32 = 22.0;
const STRIP_PAD_B: f32 = 20.0;

/// Curve-editing limits, matching the WPF editor.
const HIT_RADIUS: f32 = 11.0;
const MAX_POINTS: usize = 12;
const MIN_POINTS: usize = 2;
const MIN_BAND_C: f64 = 1.0;

/// What an interaction did to the point list this frame.
#[derive(Default)]
pub struct CurveEdit {
    /// Points changed — redraw, and the caller should push to the daemon.
    pub changed: bool,
    /// A complete gesture finished (drag released, point added or removed):
    /// the moment to record ONE undo entry, so a drag is not 50 of them.
    pub committed: bool,
}

/// Amber axis readout, seated on a card-coloured backing so it stays legible
/// where it crosses a gridline.
fn chip(painter: &Painter, at: Pos2, anchor: Align2, text: &str) {
    let font = eframe::egui::FontId::monospace(10.5);
    let galley = painter.layout_no_wrap(text.to_string(), font, AMBER);
    let size = galley.size();
    let min = match anchor {
        Align2::CENTER_TOP => Pos2::new(at.x - size.x / 2.0, at.y),
        Align2::RIGHT_CENTER => Pos2::new(at.x - size.x, at.y - size.y / 2.0),
        _ => at,
    };
    painter.rect_filled(
        Rect::from_min_size(min, size).expand2(eframe::egui::vec2(4.0, 1.0)),
        CornerRadius::same(3),
        CARD,
    );
    painter.galley(min, galley, AMBER);
}

fn card(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, CornerRadius::same(6), CARD);
    painter.rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(1.0, HAIRLINE),
        eframe::egui::StrokeKind::Inside,
    );
}

fn plot_area(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.min.x + PAD_L, rect.min.y + PAD_T),
        Pos2::new(rect.max.x - PAD_R, rect.max.y - PAD_B),
    )
}

fn strip_plot_area(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.min.x + PAD_L, rect.min.y + STRIP_PAD_T),
        Pos2::new(rect.max.x - PAD_R, rect.max.y - STRIP_PAD_B),
    )
}

/// Handle mouse editing of the curve. Runs before drawing so the same frame
/// shows the result. `drag` carries the point index across frames.
#[allow(clippy::too_many_arguments)]
pub fn edit_curve(
    ui: &eframe::egui::Ui,
    response: &eframe::egui::Response,
    rect: Rect,
    points: &mut Vec<fan_core::CurvePoint>,
    drag: &mut Option<usize>,
) -> CurveEdit {
    use fan_core::CurvePoint;

    let mut out = CurveEdit::default();
    let plot = plot_area(rect);
    if plot.width() < 60.0 || plot.height() < 40.0 {
        return out;
    }
    let to_temp = |x: f32| {
        TEMP_MIN + ((x - plot.min.x) / plot.width()).clamp(0.0, 1.0) as f64 * (TEMP_MAX - TEMP_MIN)
    };
    let to_pct = |y: f32| (1.0 - ((y - plot.min.y) / plot.height()).clamp(0.0, 1.0) as f64) * 100.0;
    let screen = |p: &CurvePoint| -> Pos2 {
        Pos2::new(
            plot.min.x + ((p.temp_c - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)) as f32 * plot.width(),
            plot.max.y - (p.percent / 100.0) as f32 * plot.height(),
        )
    };
    let Some(pos) = ui.input(|i| i.pointer.interact_pos()) else {
        return out;
    };
    let hit = points.iter().position(|p| screen(p).distance(pos) <= HIT_RADIUS);

    // Right-click removes — never below two points.
    if response.secondary_clicked() {
        if let Some(i) = hit {
            if points.len() > MIN_POINTS {
                points.remove(i);
                *drag = None;
                out.changed = true;
                out.committed = true;
                return out;
            }
        }
    }

    // Double-click on empty space adds, if there is room and the new point
    // would not sit inside an existing band.
    if response.double_clicked() && hit.is_none() && points.len() < MAX_POINTS {
        let t = to_temp(pos.x).round();
        let p = to_pct(pos.y).round();
        if points.iter().all(|e| (e.temp_c - t).abs() >= MIN_BAND_C) {
            points.push(CurvePoint::new(t, p));
            points.sort_by(|a, b| a.temp_c.total_cmp(&b.temp_c));
            out.changed = true;
            out.committed = true;
            return out;
        }
    }

    if response.drag_started() {
        *drag = hit;
    }
    if response.dragged() {
        if let Some(i) = *drag {
            // Ctrl+Z can shrink the list mid-drag.
            if i >= points.len() {
                *drag = None;
                return out;
            }
            // Snap to whole degrees / whole percent, and keep the point
            // between its neighbours so the curve stays a function of temp.
            let min_t = if i > 0 { points[i - 1].temp_c + MIN_BAND_C } else { TEMP_MIN };
            let max_t = if i + 1 < points.len() { points[i + 1].temp_c - MIN_BAND_C } else { TEMP_MAX };
            if min_t <= max_t {
                let t = to_temp(pos.x).round().clamp(min_t, max_t);
                let p = to_pct(pos.y).round().clamp(0.0, 100.0);
                if points[i] != CurvePoint::new(t, p) {
                    points[i] = CurvePoint::new(t, p);
                    out.changed = true;
                }
            }
        }
    }
    // One drag = one undo entry: commit only on release.
    if response.drag_stopped() && drag.take().is_some() {
        out.committed = true;
    }
    out
}

/// The chart draws NO card of its own — the whole column (header, curve,
/// strip, hint) lives inside one card, as in the WPF layout.
pub fn draw_curve_chart(painter: &Painter, rect: Rect, snap: &Snapshot) {
    let plot = plot_area(rect);
    if plot.width() < 60.0 || plot.height() < 40.0 {
        return; // render-robustness rule: bail below a minimum plot size
    }

    // No chart title: the card header (channel segments) already names it, and
    // a title here collides with the 100 % axis label.
    let x = |t: f64| plot.min.x + ((t - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)) as f32 * plot.width();
    let y = |p: f64| plot.max.y - (p / 100.0) as f32 * plot.height();

    // Sparse horizontal grid + right-edge % labels.
    for pct in [0.0, 25.0, 50.0, 75.0, 100.0] {
        let yy = y(pct);
        painter.line_segment(
            [Pos2::new(plot.min.x, yy), Pos2::new(plot.max.x, yy)],
            Stroke::new(1.0, Color32::from_white_alpha(10)),
        );
        crate::micro(painter, Pos2::new(plot.min.x - 6.0, yy), Align2::RIGHT_CENTER, &format!("{pct:.0}"), FAINT);
    }
    for temp in [20.0, 40.0, 60.0, 80.0, 100.0] {
        crate::micro(painter, Pos2::new(x(temp), plot.max.y + 4.0), Align2::CENTER_TOP, &format!("{temp:.0}°"), FAINT);
    }

    // Staircase: flat band from each point to the next, vertical riser between.
    if !snap.curve.is_empty() {
        let mut pts: Vec<Pos2> = Vec::with_capacity(snap.curve.len() * 2 + 2);
        let first = snap.curve[0];
        pts.push(Pos2::new(x(TEMP_MIN), y(first.percent)));
        for (i, p) in snap.curve.iter().enumerate() {
            pts.push(Pos2::new(x(p.temp_c), y(p.percent)));
            let next_t = snap.curve.get(i + 1).map_or(TEMP_MAX, |n| n.temp_c);
            pts.push(Pos2::new(x(next_t), y(p.percent)));
        }
        // Glow under, crisp line over.
        painter.add(Shape::line(pts.clone(), Stroke::new(4.0, Color32::from_white_alpha(26))));
        painter.add(Shape::line(pts, Stroke::new(1.6, TEXT)));
        for p in &snap.curve {
            painter.circle_filled(Pos2::new(x(p.temp_c), y(p.percent)), 3.0, TEXT);
        }
    }

    // Why-chip: notification-style, top-left INSIDE the plot, explaining why
    // the commanded % differs from what the curve asks for. Hidden when they
    // agree — the chip is only there when something needs explaining.
    if let Some(ch) = snap.channels.get(snap.selected) {
        let text = why_text(ch, snap);
        if !text.is_empty() {
            let font = eframe::egui::FontId::monospace(11.0);
            let galley = painter.layout_no_wrap(text, font, Color32::from_rgba_premultiplied(166, 166, 166, 166));
            let pad = eframe::egui::vec2(9.0, 4.0);
            let at = Pos2::new(plot.min.x + 6.0, plot.min.y + 8.0);
            let bg = Rect::from_min_size(at, galley.size() + pad * 2.0);
            painter.rect_filled(bg, CornerRadius::same(8), Color32::from_rgb(0x13, 0x13, 0x18));
            painter.rect_stroke(
                bg,
                CornerRadius::same(8),
                Stroke::new(1.0, Color32::from_white_alpha(26)),
                eframe::egui::StrokeKind::Inside,
            );
            painter.galley(at + pad, galley, TEXT);
        }
    }

    // Live overlay — amber only.
    if let Some(ch) = snap.channels.get(snap.selected) {
        if let Some(raw) = ch.raw_temp {
            let xx = x(raw.clamp(TEMP_MIN, TEMP_MAX));
            painter.add(Shape::dashed_line(
                &[Pos2::new(xx, plot.min.y), Pos2::new(xx, plot.max.y)],
                Stroke::new(1.0, AMBER.gamma_multiply(0.55)),
                4.0,
                4.0,
            ));
            crate::micro(painter, Pos2::new(xx, plot.min.y - 4.0), Align2::CENTER_BOTTOM, &format!("now {raw:.1}°"), AMBER);
        }
        if !ch.effective_temp.is_nan() {
            let t = ch.effective_temp.clamp(TEMP_MIN, TEMP_MAX);
            let p = ch.output_percent.clamp(0.0, 100.0);
            let dot = Pos2::new(x(t), y(p));
            // Hairlines to both axes with amber readout chips — the operating
            // point tells you the temperature AND the duty at a glance.
            let hair = Stroke::new(1.0, AMBER.gamma_multiply(0.30));
            painter.line_segment([dot, Pos2::new(dot.x, plot.max.y)], hair);
            painter.line_segment([dot, Pos2::new(plot.min.x, dot.y)], hair);
            chip(painter, Pos2::new(dot.x, plot.max.y + 4.0), Align2::CENTER_TOP, &format!("{t:.1}°"));
            chip(painter, Pos2::new(plot.min.x - 10.0, dot.y), Align2::RIGHT_CENTER, &format!("{p:.0}%"));
            painter.circle_filled(dot, 4.2, AMBER);
            painter.circle_filled(dot, 1.8, Color32::WHITE);
        }
    }
}

/// Why-chip wording — the WPF phrasings, which keep the numbers that make the
/// chip worth reading (the slew rate, the hysteresis margin, the threshold).
fn why_text(s: &fan_core::ChannelStatus, snap: &Snapshot) -> String {
    use fan_core::OutputReason as R;
    match s.reason {
        R::RampUp => format!(
            "ramping up to {:.0}% · {} %/s",
            s.target_percent,
            trim(snap.slew_up)
        ),
        R::RampDown => format!(
            "ramping down to {:.0}% · {} %/s",
            s.target_percent,
            trim(snap.slew_down)
        ),
        R::StepDownHold => format!(
            "holding {:.0}% · steps down to {:.0}% in {:.0} s",
            s.output_percent,
            s.reason_level,
            s.reason_seconds.ceil()
        ),
        R::Hysteresis => format!(
            "holding {:.0}% · avg not yet {}° below the step",
            s.output_percent,
            trim(snap.hysteresis_c)
        ),
        R::ZeroSnap => format!(
            "stopped · curve's {:.0}% is under the {:.0}% stop threshold",
            s.reason_level, snap.zero_snap_percent
        ),
        R::MinFloor => format!(
            "safety floor {:.0}% · curve asks {:.0}%",
            s.output_percent, s.reason_level
        ),
        R::IdleKick => "idle kick · spinning the stopped fan briefly".into(),
        R::StopProbe => "trial stop · resumes the moment the temp rises".into(),
        R::None => String::new(),
    }
}

/// "9" not "9.0", but "1.5" stays "1.5" — the C# "0.#" format.
fn trim(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// The dim bar between a fan turn-OFF and the next turn-ON, labelled `m:ss`.
/// Returns the label's right edge so the next one can skip when it would
/// overlap. `completed` draws the closing cap; a live stop stays open.
#[allow(clippy::too_many_arguments)]
fn draw_stopped_span(
    painter: &Painter,
    plot: Rect,
    x1: f32,
    x2: f32,
    seconds: f64,
    completed: bool,
    prev_label_right: f32,
) -> f32 {
    let y = plot.max.y - 14.0;
    let pen = Stroke::new(1.0, Color32::from_white_alpha(48));
    if x2 - x1 >= 8.0 {
        painter.line_segment([Pos2::new(x1 + 3.0, y), Pos2::new(x2 - 3.0, y)], pen);
    }
    painter.line_segment([Pos2::new(x1, y - 3.0), Pos2::new(x1, y + 3.0)], pen);
    if completed {
        painter.line_segment([Pos2::new(x2, y - 3.0), Pos2::new(x2, y + 3.0)], pen);
    }
    let s = seconds.max(0.0) as i64;
    let text = format!("{}:{:02}", s / 60, s % 60);
    let font = eframe::egui::FontId::monospace(10.0);
    let galley = painter.layout_no_wrap(text, font, Color32::from_white_alpha(166));
    let w = galley.size().x;
    let left = ((x1 + x2) / 2.0 - w / 2.0).clamp(plot.min.x, plot.max.x - w);
    // Skip a label that would collide with the previous span's.
    if left < prev_label_right + 4.0 {
        return prev_label_right;
    }
    let at = Pos2::new(left, y - galley.size().y / 2.0);
    painter.rect_filled(
        Rect::from_min_size(at, galley.size()).expand2(eframe::egui::vec2(3.0, 0.0)),
        CornerRadius::same(2),
        CARD,
    );
    painter.galley(at, galley, Color32::from_white_alpha(166));
    left + w
}

/// Wall clock as `HH:mm:ss`, from unix seconds plus the local offset captured
/// at startup (std has no timezone database).
fn clock(unix: f64, offset_secs: i64) -> String {
    let s = (unix as i64 + offset_secs).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// "now" for the freshest sample, else `−m:ss` / `−h:mm:ss`.
fn ago_text(age: f64, newest: bool) -> String {
    if newest && age <= 1.0 {
        return "now".into();
    }
    let s = age.max(0.0) as i64;
    if s < 3600 {
        format!("−{}:{:02}", s / 60, s % 60)
    } else {
        format!("−{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }
}

/// Rounded chip with a card backing — the strip's hover readout. Tries each
/// wording and draws the first that fits the plot; falls back to the last.
fn hover_chip(painter: &Painter, plot: Rect, x: f32, options: &[String]) {
    let font = eframe::egui::FontId::monospace(10.5);
    let mut chosen = painter.layout_no_wrap(
        options.last().cloned().unwrap_or_default(),
        font.clone(),
        TEXT,
    );
    for o in options {
        let g = painter.layout_no_wrap(o.clone(), font.clone(), TEXT);
        if g.size().x + 12.0 <= plot.width() {
            chosen = g;
            break;
        }
    }
    let size = chosen.size();
    let pad = eframe::egui::vec2(6.0, 3.0);
    let left = (x - size.x / 2.0).clamp(plot.min.x, plot.max.x - size.x - pad.x * 2.0);
    let at = Pos2::new(left, plot.min.y + 3.0);
    painter.rect_filled(
        Rect::from_min_size(at, size + pad * 2.0),
        CornerRadius::same(6),
        CARD,
    );
    painter.rect_stroke(
        Rect::from_min_size(at, size + pad * 2.0),
        CornerRadius::same(6),
        Stroke::new(1.0, Color32::from_white_alpha(38)),
        eframe::egui::StrokeKind::Inside,
    );
    painter.galley(at + pad, chosen, TEXT);
}

pub fn draw_history_strip(painter: &Painter, rect: Rect, snap: &Snapshot) {
    // Seam hairline instead of a card edge: the strip shares the chart's card.
    painter.line_segment(
        [Pos2::new(rect.min.x + PAD_L, rect.min.y), Pos2::new(rect.max.x - PAD_R, rect.min.y)],
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
    let plot = strip_plot_area(rect);
    if plot.width() < 60.0 || plot.height() < 30.0 {
        return;
    }
    crate::micro(painter, Pos2::new(plot.min.x, rect.min.y + 7.0), Align2::LEFT_TOP, "H I S T O R Y", FAINT);
    // Right edge names the window: "now" while live, else where it ends.
    let edge = if snap.is_live {
        "now".to_string()
    } else {
        snap.history.last().map_or(String::new(), |s| clock(s.wall, snap.utc_offset_secs))
    };
    crate::micro(painter, Pos2::new(plot.max.x, rect.max.y - 4.0), Align2::RIGHT_BOTTOM, &edge, FAINT);

    // Fan-% grid: 0 / 50 / 100 only (the curve chart carries the finer one).
    for pct in [0.0, 50.0, 100.0] {
        let yy = plot.max.y - (pct / 100.0) as f32 * plot.height();
        painter.line_segment(
            [Pos2::new(plot.min.x, yy), Pos2::new(plot.max.x, yy)],
            Stroke::new(1.0, Color32::from_white_alpha(if pct == 0.0 { 26 } else { 13 })),
        );
        let label = if pct == 100.0 { "100%".to_string() } else { format!("{pct:.0}") };
        crate::micro(painter, Pos2::new(plot.min.x - 10.0, yy), Align2::RIGHT_CENTER, &label, FAINT);
    }
    let _ = card; // the card is drawn by the caller now

    // X is INDEX-based, not time-based: the newest sample owns the right edge
    // and a partly filled window grows in from the right. (A jittered tick
    // makes a time mapping drift, and index anchoring is what scrollback will
    // need.) Temps share the curve chart's 15–100 °C span so the two align.
    const WINDOW: usize = 600;
    let visible: Vec<&HistorySample> = snap.history.iter().rev().take(WINDOW).rev().collect();
    let count = visible.len();
    let px_per_sample = plot.width() / (WINDOW - 1) as f32;
    let x = |i: usize| plot.max.x - (count.saturating_sub(1) - i) as f32 * px_per_sample;
    let y_temp = |c: f64| {
        plot.max.y
            - (((c - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)) as f32).clamp(0.0, 1.0) * plot.height()
    };
    let y_pct = |p: f64| plot.max.y - ((p / 100.0) as f32).clamp(0.0, 1.0) * plot.height();
    // Temperature reference labels only, no lines (the grid belongs to fan %).
    for t in [40.0, 60.0, 80.0] {
        crate::micro(
            painter,
            Pos2::new(plot.max.x - 4.0, y_temp(t)),
            Align2::RIGHT_CENTER,
            &format!("{t:.0}°"),
            Color32::from_white_alpha(51),
        );
    }

    if count >= 2 {
        // Fan % under-fill + dim trace. The staircase outline is CONCAVE, and
        // `convex_polygon` fills a concave outline with criss-crossing
        // triangles (visible smears) — so the fill is a hand-built triangle
        // strip down to the baseline instead.
        let line: Vec<Pos2> =
            visible.iter().enumerate().map(|(i, s)| Pos2::new(x(i), y_pct(s.out))).collect();
        let fill_color = Color32::from_white_alpha(8);
        let mut mesh = eframe::egui::Mesh::default();
        for (i, p) in line.iter().enumerate() {
            mesh.colored_vertex(*p, fill_color);
            mesh.colored_vertex(Pos2::new(p.x, plot.max.y), fill_color);
            if i > 0 {
                let b = (2 * i) as u32;
                mesh.add_triangle(b - 2, b - 1, b);
                mesh.add_triangle(b - 1, b + 1, b);
            }
        }
        painter.add(Shape::mesh(mesh));
        painter.add(Shape::line(line, Stroke::new(1.5, Color32::from_white_alpha(102))));

        // Raw "now" temp, faint — a missing reading BREAKS the line rather
        // than bridging over it, which is how a dead sensor should read.
        let mut run: Vec<Pos2> = Vec::new();
        for (i, s) in visible.iter().enumerate() {
            match s.raw {
                Some(r) if !r.is_nan() => run.push(Pos2::new(x(i), y_temp(r))),
                _ => {
                    if run.len() >= 2 {
                        painter.add(Shape::line(std::mem::take(&mut run), Stroke::new(1.0, FAINT)));
                    } else {
                        run.clear();
                    }
                }
            }
        }
        if run.len() >= 2 {
            painter.add(Shape::line(run, Stroke::new(1.0, FAINT)));
        }

        // Rolling average — the bright trace. NaN (no reading) BREAKS the
        // line like the raw trace: a NaN vertex poisons the tessellator and
        // smears the whole strip.
        let mut avg_run: Vec<Pos2> = Vec::new();
        for (i, s) in visible.iter().enumerate() {
            if s.avg.is_nan() {
                if avg_run.len() >= 2 {
                    painter.add(Shape::line(std::mem::take(&mut avg_run), Stroke::new(2.0, TEXT)));
                } else {
                    avg_run.clear();
                }
            } else {
                avg_run.push(Pos2::new(x(i), y_temp(s.avg)));
            }
        }
        if avg_run.len() >= 2 {
            painter.add(Shape::line(avg_run, Stroke::new(2.0, TEXT)));
        }

        // Fan ON/OFF baseline ticks (ON brighter), and the stopped-time span
        // between an OFF and the next ON — how long the fan actually rested.
        let mut last_off: Option<usize> = None;
        let mut last_label_right = f32::NEG_INFINITY;
        for i in 1..count {
            let (prev, cur) = (visible[i - 1].out, visible[i].out);
            let on = prev <= 0.01 && cur > 0.01;
            let off = prev > 0.01 && cur <= 0.01;
            if on || off {
                let xx = x(i);
                painter.line_segment(
                    [Pos2::new(xx, plot.max.y), Pos2::new(xx, plot.max.y - 6.0)],
                    Stroke::new(1.0, Color32::from_white_alpha(if on { 140 } else { 70 })),
                );
            }
            if off {
                last_off = Some(i);
            }
            if on {
                if let Some(o) = last_off.take() {
                    last_label_right =
                        draw_stopped_span(painter, plot, x(o), x(i), visible[i].wall - visible[o].wall, true, last_label_right);
                }
            }
        }
        // An ongoing stop counts up live against the right edge.
        if let Some(o) = last_off {
            if visible[count - 1].out <= 0.01 {
                draw_stopped_span(
                    painter,
                    plot,
                    x(o),
                    x(count - 1),
                    visible[count - 1].wall - visible[o].wall,
                    false,
                    last_label_right,
                );
            }
        }

        // Live edge — the only amber in the strip, and gone while scrolled.
        let last = count - 1;
        if snap.is_live {
            if !visible[last].avg.is_nan() {
                painter.circle_filled(Pos2::new(x(last), y_temp(visible[last].avg)), 2.6, AMBER);
            }
            painter.circle_filled(Pos2::new(x(last), y_pct(visible[last].out)), 2.2, AMBER);
        }

        // Hover: crosshair, markers on both traces, and the readout chip.
        if let Some(px) = snap.hover_x {
            if px >= plot.min.x - 4.0 && px <= plot.max.x + 4.0 {
                let i = (((px - plot.max.x) / px_per_sample) + (count - 1) as f32)
                    .round()
                    .clamp(0.0, (count - 1) as f32) as usize;
                let s = visible[i];
                let xx = x(i);
                painter.line_segment(
                    [Pos2::new(xx, plot.min.y), Pos2::new(xx, plot.max.y)],
                    Stroke::new(1.0, Color32::from_white_alpha(48)),
                );
                let mark = |p: Pos2| {
                    painter.circle_filled(p, 4.4, CARD);
                    painter.circle_filled(p, 3.0, TEXT);
                };
                mark(Pos2::new(xx, y_pct(s.out)));
                if !s.avg.is_nan() {
                    mark(Pos2::new(xx, y_temp(s.avg)));
                }
                let a = |v: f64| if v.is_nan() { "—".to_string() } else { format!("{v:.1}°") };
                let clock_s = clock(s.wall, snap.utc_offset_secs);
                let ago = ago_text(visible[count - 1].wall - s.wall, i == count - 1);
                let raw = s.raw.map_or("—".to_string(), |r| format!("{r:.1}°"));
                hover_chip(
                    painter,
                    plot,
                    xx,
                    &[
                        format!("{clock_s} · {ago} · avg {} · now {raw} · {:.0}%", a(s.avg), s.out),
                        format!("{clock_s} · {ago}\navg {} · now {raw} · {:.0}%", a(s.avg), s.out),
                    ],
                );
            }
        }
    } else {
        crate::micro(painter, plot.center(), Align2::CENTER_CENTER, "collecting…", FAINT);
    }

    // Legend, right-aligned in the title row.
    let legend = Pos2::new(rect.max.x - 14.0, rect.min.y + 8.0);
    crate::micro(painter, legend, Align2::RIGHT_TOP, "avg — · now ·· · fan %", FAINT);
    let _ = Vec2::ZERO;
}
