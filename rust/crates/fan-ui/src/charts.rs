//! Custom-drawn charts — the egui `Painter` translation of the WPF
//! `OnRender`/`DrawingContext` code. Same visual grammar: card surface with a
//! hairline, sparse horizontal grid, glow-under white staircase, and amber
//! reserved for live thermal state (operating dot, raw-temp dashed vertical).

use eframe::egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::client::HistorySample;
use crate::{Snapshot, AMBER, CARD, DIM, FAINT, HAIRLINE, TEXT};

const TEMP_MIN: f64 = 15.0;
const TEMP_MAX: f64 = 100.0;
const PAD_L: f32 = 40.0;
const PAD_R: f32 = 16.0;
const PAD_T: f32 = 26.0;
const PAD_B: f32 = 24.0;

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

pub fn draw_curve_chart(painter: &Painter, rect: Rect, snap: &Snapshot) {
    card(painter, rect);
    let plot = plot_area(rect);
    if plot.width() < 60.0 || plot.height() < 40.0 {
        return; // render-robustness rule: bail below a minimum plot size
    }

    crate::micro(painter, Pos2::new(rect.min.x + 14.0, rect.min.y + 8.0), Align2::LEFT_TOP, "CURVE", FAINT);

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
            let dot = Pos2::new(
                x(ch.effective_temp.clamp(TEMP_MIN, TEMP_MAX)),
                y(ch.output_percent.clamp(0.0, 100.0)),
            );
            painter.circle_filled(dot, 5.0, AMBER);
            painter.circle_stroke(dot, 8.0, Stroke::new(1.0, AMBER.gamma_multiply(0.35)));
        }
    }
}

pub fn draw_history_strip(painter: &Painter, rect: Rect, snap: &Snapshot) {
    card(painter, rect);
    let plot = plot_area(rect);
    if plot.width() < 60.0 || plot.height() < 30.0 {
        return;
    }
    crate::micro(painter, Pos2::new(rect.min.x + 14.0, rect.min.y + 8.0), Align2::LEFT_TOP, "HISTORY · 10 MIN", FAINT);

    let now = snap.history.last().map_or(0.0, |s| s.t);
    let t0 = now - 600.0;
    let x = |t: f64| plot.min.x + (((t - t0) / 600.0) as f32).clamp(0.0, 1.0) * plot.width();
    let y_temp = |c: f64| plot.max.y - (((c - 20.0) / 80.0) as f32).clamp(0.0, 1.0) * plot.height();
    let y_pct = |p: f64| plot.max.y - ((p / 100.0) as f32).clamp(0.0, 1.0) * plot.height();

    let visible: Vec<&HistorySample> = snap.history.iter().filter(|s| s.t >= t0).collect();
    if visible.len() >= 2 {
        // Fan % under-fill + dim trace.
        let mut fill: Vec<Pos2> = visible.iter().map(|s| Pos2::new(x(s.t), y_pct(s.out))).collect();
        let line = fill.clone();
        fill.push(Pos2::new(x(now), plot.max.y));
        fill.push(Pos2::new(x(visible[0].t), plot.max.y));
        painter.add(Shape::convex_polygon(
            // Not convex in general — egui still fills simple ribbons acceptably,
            // and the WPF strip uses the same under-fill idea. Kept subtle.
            fill,
            Color32::from_white_alpha(8),
            Stroke::NONE,
        ));
        painter.add(Shape::line(line, Stroke::new(1.0, DIM)));

        // Raw temp, faint.
        let raw: Vec<Pos2> = visible
            .iter()
            .filter_map(|s| s.raw.map(|r| Pos2::new(x(s.t), y_temp(r))))
            .collect();
        if raw.len() >= 2 {
            painter.add(Shape::line(raw, Stroke::new(1.0, FAINT)));
        }

        // Average — the bright trace.
        let avg: Vec<Pos2> = visible.iter().map(|s| Pos2::new(x(s.t), y_temp(s.avg))).collect();
        painter.add(Shape::line(avg, Stroke::new(1.4, TEXT)));

        // Live-edge dot in amber.
        if let Some(last) = visible.last() {
            painter.circle_filled(Pos2::new(x(last.t), y_temp(last.avg)), 2.5, AMBER);
        }
    } else {
        crate::micro(painter, plot.center(), Align2::CENTER_CENTER, "collecting…", FAINT);
    }

    // Legend, right-aligned in the title row.
    let legend = Pos2::new(rect.max.x - 14.0, rect.min.y + 8.0);
    crate::micro(painter, legend, Align2::RIGHT_TOP, "avg — · fan % ▁ · now ·", FAINT);
    let _ = Vec2::ZERO;
}
