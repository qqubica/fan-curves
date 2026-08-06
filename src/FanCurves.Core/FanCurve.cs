namespace FanCurves.Core;

public record struct CurvePoint(double TempC, double Percent);

/// <summary>
/// Piecewise-linear fan curve. Points are kept sorted by temperature;
/// output is clamped to the first/last point outside the defined range.
/// </summary>
public class FanCurve
{
    private const double AxisMin = 15, AxisMax = 100;
    private readonly List<CurvePoint> _points = new();

    public IReadOnlyList<CurvePoint> Points => _points;

    public FanCurve(IEnumerable<CurvePoint> points)
    {
        _points.AddRange(points);
        Normalize();
    }

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
        double minT = index > 0 ? _points[index - 1].TempC + 1 : AxisMin;
        double maxT = index < _points.Count - 1 ? _points[index + 1].TempC - 1 : AxisMax;
        _points[index] = new CurvePoint(
            Math.Clamp(tempC, minT, maxT),
            Math.Clamp(percent, 0, 100));
    }

    private void Normalize()
    {
        _points.Sort((a, b) => a.TempC.CompareTo(b.TempC));
        for (int i = 0; i < _points.Count; i++)
            _points[i] = new CurvePoint(
                Math.Clamp(_points[i].TempC, AxisMin, AxisMax),
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
