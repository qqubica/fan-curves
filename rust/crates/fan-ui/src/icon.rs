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

/// The 32×32 window/taskbar icon as RGBA8, supersampled 4× for smooth edges.
pub fn icon_data() -> eframe::egui::IconData {
    const N: usize = 32;
    const SS: usize = 4; // supersampling factor
    let mut rgba = vec![0u8; N * N * 4];

    // Geometry in icon space, straight from TrayIcon.DrawIcon.
    let c = (16.0f32, 16.0f32);
    let bg_r = 15.0;
    let ring_outer = 15.0;
    let ring_inner = 13.0; // a 2 px stroke centred on r = 14
    let hub_r = 3.0;
    let blade_offset = 8.0; // blade centre (16, 8) → 8 above the hub
    let (blade_rx, blade_ry) = (2.5f32, 5.0f32);

    let inside_blade = |x: f32, y: f32| -> bool {
        (0..3).any(|i| {
            let a = -std::f32::consts::TAU * i as f32 / 3.0;
            let (dx, dy) = (x - c.0, y - c.1);
            let (s, co) = a.sin_cos();
            // Rotate the sample into blade space, then the ellipse test.
            let (rx, ry) = (dx * co - dy * s, dx * s + dy * co);
            let ey = ry + blade_offset;
            (rx / blade_rx).powi(2) + (ey / blade_ry).powi(2) <= 1.0
        })
    };

    for py in 0..N {
        for px in 0..N {
            let (mut acc_r, mut acc_g, mut acc_b, mut acc_a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let d = ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt();
                    if d > bg_r {
                        continue; // outside the disc: transparent
                    }
                    // Base: the near-black disc.
                    let (mut r, mut g, mut b) = (8.0f32, 8.0f32, 10.0f32);
                    // Ring, white at α 80/255, composited over the disc.
                    if d >= ring_inner && d <= ring_outer {
                        let a = 80.0 / 255.0;
                        r = r * (1.0 - a) + 255.0 * a;
                        g = g * (1.0 - a) + 255.0 * a;
                        b = b * (1.0 - a) + 255.0 * a;
                    }
                    // Blades and hub, opaque white.
                    if d <= hub_r || inside_blade(x, y) {
                        r = 255.0;
                        g = 255.0;
                        b = 255.0;
                    }
                    acc_r += r;
                    acc_g += g;
                    acc_b += b;
                    acc_a += 255.0;
                }
            }
            let n = (SS * SS) as f32;
            let i = (py * N + px) * 4;
            // Straight (non-premultiplied) RGBA: average colour, coverage alpha.
            let covered = (acc_a / 255.0).max(1e-6);
            rgba[i] = (acc_r / covered).round().clamp(0.0, 255.0) as u8;
            rgba[i + 1] = (acc_g / covered).round().clamp(0.0, 255.0) as u8;
            rgba[i + 2] = (acc_b / covered).round().clamp(0.0, 255.0) as u8;
            rgba[i + 3] = (acc_a / n).round().clamp(0.0, 255.0) as u8;
        }
    }

    eframe::egui::IconData { rgba, width: N as u32, height: N as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_a_disc_with_a_white_hub() {
        let icon = icon_data();
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| {
            let i = (y * 32 + x) * 4;
            (icon.rgba[i], icon.rgba[i + 1], icon.rgba[i + 2], icon.rgba[i + 3])
        };
        // Corner is outside the disc → fully transparent.
        assert_eq!(at(0, 0).3, 0);
        // Centre is the hub → opaque white.
        let (r, g, b, a) = at(16, 16);
        assert_eq!((r, g, b, a), (255, 255, 255, 255));
        // A point between hub and ring, away from any blade, stays near-black.
        let (r, _, _, a) = at(16, 22);
        assert_eq!(a, 255);
        assert!(r < 60, "expected the dark disc, got r={r}");
    }
}
