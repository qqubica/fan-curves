//! The app mark: three fan blades and a hub, drawn — never an asset file, the
//! same choice the WPF app made for one glyph.
//!
//! Two consumers, one geometry:
//!   * [`draw_glyph`] paints the title-bar mark (the WPF 20×20 canvas: blade
//!     ellipses centred at (10, 4.8) with radii 2.5 × 4.2, rotated 0/120/240°
//!     about the centre, plus a hub of radius 2.1, all at white α 0xD9).
//!   * [`icon_data`] rasterises the 32×32 window/taskbar icon: near-black disc,
//!     hairline white ring, the same blades and a radius-3 hub — pixel-for-pixel
//!     the design `TrayIcon.DrawIcon` produces.

use eframe::egui::{Color32, Painter, Pos2, Shape, Stroke, Vec2};

/// Points around one blade: an ellipse of `radius`, centred `offset` above the
/// hub, rotated `angle` radians about the hub.
fn blade(center: Pos2, offset: f32, radius: Vec2, angle: f32, steps: usize) -> Vec<Pos2> {
    (0..steps)
        .map(|i| {
            let t = std::f32::consts::TAU * i as f32 / steps as f32;
            // Ellipse point in blade space, then lifted onto the hub axis…
            let (ex, ey) = (radius.x * t.cos(), radius.y * t.sin() - offset);
            // …then rotated about the hub.
            let (s, c) = angle.sin_cos();
            Pos2::new(center.x + ex * c - ey * s, center.y + ex * s + ey * c)
        })
        .collect()
}

/// Title-bar mark, scaled to `size` (the WPF canvas is 20 px).
pub fn draw_glyph(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let k = size / 20.0; // WPF units → pixels
    for i in 0..3 {
        let angle = std::f32::consts::TAU * i as f32 / 3.0;
        let pts = blade(center, 5.2 * k, Vec2::new(2.5 * k, 4.2 * k), angle, 24);
        painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    }
    painter.circle_filled(center, 2.1 * k, color);
}

/// The 32×32 window/taskbar icon. The pixels come from `fan_core::icon`, so
/// the UI and the tray launcher cannot drift apart.
pub fn icon_data() -> eframe::egui::IconData {
    eframe::egui::IconData {
        rgba: fan_core::icon::rgba(),
        width: fan_core::icon::SIZE as u32,
        height: fan_core::icon::SIZE as u32,
    }
}
