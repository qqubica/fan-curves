use serde::{Deserialize, Serialize};

/// One staircase step. Serialized with the C# field names so the same
/// `profile.json` loads in both implementations.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CurvePoint {
    pub temp_c: f64,
    pub percent: f64,
}

impl CurvePoint {
    pub fn new(temp_c: f64, percent: f64) -> Self {
        Self { temp_c, percent }
    }
}

/// Staircase fan curve. Points are kept sorted by temperature; output is
/// clamped to the first/last point outside the defined range.
#[derive(Debug, Clone)]
pub struct FanCurve {
    points: Vec<CurvePoint>,
}

const AXIS_MIN: f64 = 15.0;
const AXIS_MAX: f64 = 100.0;

impl FanCurve {
    pub fn new(points: &[CurvePoint]) -> Self {
        let mut c = Self { points: points.to_vec() };
        c.normalize();
        c
    }

    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    pub fn replace_points(&mut self, points: &[CurvePoint]) {
        self.points = points.to_vec();
        self.normalize();
    }

    pub fn move_point(&mut self, index: usize, temp_c: f64, percent: f64) {
        if index >= self.points.len() {
            return;
        }
        // Clamp temperature between neighbours so the curve stays a function of temp.
        let min_t = if index > 0 { self.points[index - 1].temp_c + 1.0 } else { AXIS_MIN };
        let max_t = if index < self.points.len() - 1 {
            self.points[index + 1].temp_c - 1.0
        } else {
            AXIS_MAX
        };
        self.points[index] = CurvePoint::new(temp_c.clamp(min_t, max_t), percent.clamp(0.0, 100.0));
    }

    fn normalize(&mut self) {
        self.points.sort_by(|a, b| a.temp_c.total_cmp(&b.temp_c));
        for p in &mut self.points {
            p.temp_c = p.temp_c.clamp(AXIS_MIN, AXIS_MAX);
            p.percent = p.percent.clamp(0.0, 100.0);
        }
    }

    /// Staircase evaluation: each point starts a band — at or above its
    /// temperature the fan runs at that point's level, flat, until the next
    /// point's temperature.
    pub fn evaluate(&self, temp_c: f64) -> f64 {
        if self.points.is_empty() {
            return 50.0;
        }
        let mut level = self.points[0].percent;
        for p in &self.points {
            if temp_c >= p.temp_c {
                level = p.percent;
            } else {
                break;
            }
        }
        level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet_cpu() -> FanCurve {
        FanCurve::new(&[
            CurvePoint::new(20.0, 0.0),
            CurvePoint::new(50.0, 10.0),
            CurvePoint::new(55.0, 20.0),
            CurvePoint::new(62.0, 40.0),
        ])
    }

    #[test]
    fn staircase_bands_are_flat_and_left_closed() {
        let c = quiet_cpu();
        assert_eq!(c.evaluate(19.0), 0.0); // below first point: first point's level
        assert_eq!(c.evaluate(20.0), 0.0);
        assert_eq!(c.evaluate(49.999), 0.0);
        assert_eq!(c.evaluate(50.0), 10.0); // at the threshold the band opens
        assert_eq!(c.evaluate(54.9), 10.0);
        assert_eq!(c.evaluate(55.0), 20.0);
        assert_eq!(c.evaluate(61.99), 20.0);
        assert_eq!(c.evaluate(62.0), 40.0);
        assert_eq!(c.evaluate(99.0), 40.0); // clamped to the last band
    }

    #[test]
    fn empty_curve_returns_50() {
        assert_eq!(FanCurve::new(&[]).evaluate(70.0), 50.0);
    }

    #[test]
    fn normalize_sorts_and_clamps() {
        let c = FanCurve::new(&[
            CurvePoint::new(80.0, 120.0),
            CurvePoint::new(5.0, -3.0), // temp clamps to axis min 15, percent to 0
        ]);
        assert_eq!(c.points()[0], CurvePoint::new(15.0, 0.0));
        assert_eq!(c.points()[1], CurvePoint::new(80.0, 100.0));
    }

    #[test]
    fn move_point_clamps_between_neighbours() {
        let mut c = quiet_cpu();
        c.move_point(1, 90.0, 50.0); // can't pass its right neighbour (55) minus 1
        assert_eq!(c.points()[1], CurvePoint::new(54.0, 50.0));
        c.move_point(1, 10.0, 50.0); // nor its left neighbour (20) plus 1
        assert_eq!(c.points()[1], CurvePoint::new(21.0, 50.0));
    }
}
