namespace FanCurves.Core;

public record struct CurvePoint(double TempC, double Percent);

/// <summary>A watts→percent step of the power curve (PowerCurve control mode).</summary>
public record struct PowerPoint(double Watts, double Percent);

/// <summary>
/// Piecewise-linear fan curve. Points are kept sorted by temperature;
/// output is clamped to the first/last point outside the defined range.
/// The axis bounds default to the temperature range; a power curve reuses the
/// same staircase with watts on the X axis (see FromPower).
/// </summary>
public class FanCurve
{
    private readonly double _axisMin, _axisMax;
    private readonly List<CurvePoint> _points = new();

    public IReadOnlyList<CurvePoint> Points => _points;

    public FanCurve(IEnumerable<CurvePoint> points, double axisMin = 15, double axisMax = 100)
    {
        _axisMin = axisMin;
        _axisMax = axisMax;
        _points.AddRange(points);
        Normalize();
    }

    /// <summary>The watts staircase for PowerCurve mode — X is watts, unclamped in practice.</summary>
    public static FanCurve FromPower(IEnumerable<PowerPoint> points) =>
        new(points.Select(p => new CurvePoint(p.Watts, p.Percent)), 0, 100000);

    public void ReplacePoints(IEnumerable<CurvePoint> points)
    {
        _points.Clear();
        _points.AddRange(points);
        Normalize();
    }

    public void MovePoint(int index, double tempC, double percent)
    {
        if (index < 0 || index >= _points.Count) return;
        // Clamp temperature between neighbours so the curve stays a function of temp.
        double minT = index > 0 ? _points[index - 1].TempC + 1 : _axisMin;
        double maxT = index < _points.Count - 1 ? _points[index + 1].TempC - 1 : _axisMax;
        _points[index] = new CurvePoint(
            Math.Clamp(tempC, minT, maxT),
            Math.Clamp(percent, 0, 100));
    }

    private void Normalize()
    {
        _points.Sort((a, b) => a.TempC.CompareTo(b.TempC));
        for (int i = 0; i < _points.Count; i++)
            _points[i] = new CurvePoint(
                Math.Clamp(_points[i].TempC, _axisMin, _axisMax),
                Math.Clamp(_points[i].Percent, 0, 100));
    }

    /// <summary>
    /// Staircase evaluation: each point starts a band — at or above its temperature
    /// the fan runs at that point's level, flat, until the next point's temperature.
    /// </summary>
    public double Evaluate(double tempC)
    {
        if (_points.Count == 0) return 50;
        double level = _points[0].Percent;
        foreach (var p in _points)
        {
            if (tempC >= p.TempC) level = p.Percent;
            else break;
        }
        return level;
    }
}
