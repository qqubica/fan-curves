//! The app mark as pixels — three fan blades and a hub, drawn rather than
//! shipped as an asset (the same choice the WPF app made for one glyph).
//!
//! Lives in core because two processes need it: the UI window/taskbar icon and
//! the tray launcher. The geometry is `TrayIcon.DrawIcon`'s verbatim — a
//! near-black disc, a hairline white ring, three blade ellipses centred 8 above
//! the hub at radii 2.5 × 5 and rotated 0/120/240°, and a radius-3 hub.

/// Icon edge in pixels.
pub const SIZE: usize = 32;
/// Supersampling factor for smooth edges.
const SS: usize = 4;

/// Straight (non-premultiplied) RGBA8, `SIZE * SIZE * 4` bytes.
pub fn rgba() -> Vec<u8> {
    let mut out = vec![0u8; SIZE * SIZE * 4];
    let c = (16.0f32, 16.0f32);
    let (bg_r, ring_outer, ring_inner, hub_r) = (15.0f32, 15.0f32, 13.0f32, 3.0f32);
    let blade_offset = 8.0f32;
    let (blade_rx, blade_ry) = (2.5f32, 5.0f32);

    let inside_blade = |x: f32, y: f32| -> bool {
        (0..3).any(|i| {
            let a = -std::f32::consts::TAU * i as f32 / 3.0;
            let (dx, dy) = (x - c.0, y - c.1);
            let (s, co) = a.sin_cos();
            // Rotate the sample into blade space, then a plain ellipse test.
            let (rx, ry) = (dx * co - dy * s, dx * s + dy * co);
            let ey = ry + blade_offset;
            (rx / blade_rx).powi(2) + (ey / blade_ry).powi(2) <= 1.0
        })
    };

    for py in 0..SIZE {
        for px in 0..SIZE {
            let (mut ar, mut ag, mut ab, mut aa) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let d = ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt();
                    if d > bg_r {
                        continue; // outside the disc → transparent
                    }
                    let (mut r, mut g, mut b) = (8.0f32, 8.0f32, 10.0f32);
                    if d >= ring_inner && d <= ring_outer {
                        let a = 80.0 / 255.0;
                        r = r * (1.0 - a) + 255.0 * a;
                        g = g * (1.0 - a) + 255.0 * a;
                        b = b * (1.0 - a) + 255.0 * a;
                    }
                    if d <= hub_r || inside_blade(x, y) {
                        r = 255.0;
                        g = 255.0;
                        b = 255.0;
                    }
                    ar += r;
                    ag += g;
                    ab += b;
                    aa += 255.0;
                }
            }
            let n = (SS * SS) as f32;
            let i = (py * SIZE + px) * 4;
            let covered = (aa / 255.0).max(1e-6);
            out[i] = (ar / covered).round().clamp(0.0, 255.0) as u8;
            out[i + 1] = (ag / covered).round().clamp(0.0, 255.0) as u8;
            out[i + 2] = (ab / covered).round().clamp(0.0, 255.0) as u8;
            out[i + 3] = (aa / n).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_with_a_white_hub() {
        let px = rgba();
        assert_eq!(px.len(), SIZE * SIZE * 4);
        let at = |x: usize, y: usize| {
            let i = (y * SIZE + x) * 4;
            (px[i], px[i + 1], px[i + 2], px[i + 3])
        };
        assert_eq!(at(0, 0).3, 0, "corner is outside the disc");
        assert_eq!(at(16, 16), (255, 255, 255, 255), "hub is opaque white");
        let (r, _, _, a) = at(16, 22);
        assert_eq!(a, 255);
        assert!(r < 60, "expected the dark disc, got r={r}");
    }
}
