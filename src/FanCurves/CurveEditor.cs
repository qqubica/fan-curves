using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using FanCurves.Core;

namespace FanCurves;

/// <summary>
/// Interactive fan-curve chart: drag points, double-click the line to add one,
/// right-click a point to remove it. Overlays live raw temp, effective (filtered)
/// temp and the current output the engine is commanding.
/// </summary>
public class CurveEditor : FrameworkElement
{
    private const double TempMin = 15, TempMax = 100;
    private static readonly Thickness Pad = new(42, 16, 18, 30);

    // The one accent in the app: warm amber, reserved for live thermal state.
    private static readonly Color Amber = Color.FromRgb(0xFF, 0x9E, 0x5E);
    private static readonly Color CardBg = Color.FromRgb(0x11, 0x11, 0x16);

    private ChannelConfig? _channel;
    private int _dragIndex = -1;
    private Point? _hoverPos;

    /// <summary>When true the curve is an illustration only (simple mode).</summary>
    public bool IsReadOnly { get; set; } = true;

    /// <summary>Developer mode: also overlay the raw (unaveraged) temperature.</summary>
    public bool ShowRaw { get; set; }

    private double? _liveRaw, _liveEffective, _liveOutput;
    private double? _liveWatts, _liveWattsAvg;

    public event Action? CurveChanged;

    public ChannelConfig? Channel
    {
        get => _channel;
        set { _channel = value; InvalidateVisual(); }
    }

    public void UpdateLive(double? raw, double? effective, double? output,
        double? watts = null, double? wattsAvg = null)
    {
        _liveRaw = raw;
        _liveEffective = effective;
        _liveOutput = output;
        _liveWatts = watts;
        _liveWattsAvg = wattsAvg;
        InvalidateVisual();
    }

    private Rect Plot => new(
        Pad.Left, Pad.Top,
        Math.Max(10, ActualWidth - Pad.Left - Pad.Right),
        Math.Max(10, ActualHeight - Pad.Top - Pad.Bottom));

    private Point ToScreen(double temp, double pct)
    {
        var r = Plot;
        return new Point(
            r.Left + (temp - TempMin) / (TempMax - TempMin) * r.Width,
            r.Bottom - pct / 100.0 * r.Height);
    }

    private (double temp, double pct) FromScreen(Point p)
    {
        var r = Plot;
        return (
            TempMin + Math.Clamp((p.X - r.Left) / r.Width, 0, 1) * (TempMax - TempMin),
            Math.Clamp((r.Bottom - p.Y) / r.Height, 0, 1) * 100);
    }

    protected override void OnRender(DrawingContext dc)
    {
        // Mid-resize arrange passes can hand this control a near-zero size; drawing
        // into it is meaningless and the label clamps below would see inverted ranges
        // (min > max threw and took the whole app down — crash 2026-07-27).
        if (ActualWidth - Pad.Left - Pad.Right < 60 || ActualHeight - Pad.Top - Pad.Bottom < 40)
            return;
        // The card behind provides the surface; a transparent fill keeps hit-testing alive.
        dc.DrawRectangle(Brushes.Transparent, null, new Rect(0, 0, ActualWidth, ActualHeight));
        var r = Plot;

        var gridPen = new Pen(new SolidColorBrush(Color.FromArgb(0x0d, 0xff, 0xff, 0xff)), 1);
        var axisPen = new Pen(new SolidColorBrush(Color.FromArgb(0x1a, 0xff, 0xff, 0xff)), 1);
        var labelBrush = new SolidColorBrush(Color.FromArgb(0x59, 0xff, 0xff, 0xff));
        var tf = new Typeface(new FontFamily("Cascadia Mono, Consolas"),
            FontStyles.Normal, FontWeights.Normal, FontStretches.Normal);

        FormattedText Label(string s, Brush? brush = null, double size = 10.5) => new(
            s, CultureInfo.InvariantCulture, FlowDirection.LeftToRight, tf, size,
            brush ?? labelBrush, 1.25);

        // Sparse horizontal grid; temperatures are labels only — no vertical cage.
        for (double pct = 0; pct <= 100; pct += 25)
        {
            var y = ToScreen(TempMin, pct).Y;
            dc.DrawLine(pct == 0 ? axisPen : gridPen, new Point(r.Left, y), new Point(r.Right, y));
            var t = Label(pct == 100 ? "100%" : $"{pct:0}");
            dc.DrawText(t, new Point(r.Left - t.Width - 10, y - t.Height / 2));
        }
        for (double temp = 20; temp <= 100; temp += 20)
        {
            var x = ToScreen(temp, 0).X;
            var t = Label($"{temp:0}°");
            dc.DrawText(t, new Point(x - t.Width / 2, r.Bottom + 8));
        }

        if (_channel == null || _channel.Points.Count == 0) return;
        var pts = _channel.Points.OrderBy(p => p.TempC).ToList();

        // Staircase: each point opens a flat band that runs until the next point's temp.
        var stairs = new List<Point> { ToScreen(TempMin, pts[0].Percent) };
        for (int i = 0; i < pts.Count; i++)
        {
            double bandEnd = i < pts.Count - 1 ? pts[i + 1].TempC : TempMax;
            stairs.Add(ToScreen(pts[i].TempC, pts[i].Percent)); // vertical jump lands here
            stairs.Add(ToScreen(bandEnd, pts[i].Percent));      // flat band
        }

        var geo = new StreamGeometry();
        using (var ctx = geo.Open())
        {
            ctx.BeginFigure(new Point(r.Left, r.Bottom), true, true);
            foreach (var p in stairs) ctx.LineTo(p, false, false);
            ctx.LineTo(new Point(r.Right, r.Bottom), false, false);
        }
        geo.Freeze();
        var underFill = new LinearGradientBrush(
            Color.FromArgb(0x16, 0xff, 0xff, 0xff), Color.FromArgb(0x02, 0xff, 0xff, 0xff),
            new Point(0, 0), new Point(0, 1));
        dc.DrawGeometry(underFill, null, geo);

        // Soft glow pass under a crisp white line.
        var glowPen = new Pen(new SolidColorBrush(Color.FromArgb(0x24, 0xff, 0xff, 0xff)), 6.5)
        { LineJoin = PenLineJoin.Round, StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round };
        var curvePen = new Pen(new SolidColorBrush(Color.FromArgb(0xf0, 0xff, 0xff, 0xff)), 2.2)
        { LineJoin = PenLineJoin.Round, StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round };
        for (int i = 1; i < stairs.Count; i++)
            dc.DrawLine(glowPen, stairs[i - 1], stairs[i]);
        for (int i = 1; i < stairs.Count; i++)
            dc.DrawLine(curvePen, stairs[i - 1], stairs[i]);

        // Power draw readout, developer mode (power-controlled channels only) — the
        // text is measured here so the raw-temp label below can dodge it, but drawn
        // LAST, seated on the card colour, so the staircase/crosshairs never run
        // through it (same rule as the strip charts' reference labels).
        FormattedText? power = null;
        double powerLeft = r.Right;
        if (ShowRaw && _liveWatts is double w)
        {
            double wa = _liveWattsAvg ?? w;
            power = Label(string.Create(CultureInfo.InvariantCulture,
                    $"draw {w:0} W · avg {wa:0} W"),
                new SolidColorBrush(Color.FromArgb(0x73, 0xff, 0xff, 0xff)));
            powerLeft = Math.Max(r.Left, r.Right - power.Width - 4);
        }

        // Raw (unaveraged) temp, developer mode: quiet dashed reference line.
        if (ShowRaw && _liveRaw is double raw)
        {
            var pen = new Pen(new SolidColorBrush(Color.FromArgb(0x40, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) };
            var x = ToScreen(Math.Clamp(raw, TempMin, TempMax), 0).X;
            dc.DrawLine(pen, new Point(x, r.Top), new Point(x, r.Bottom));
            var t = Label($"now {raw.ToString("0.0", CultureInfo.InvariantCulture)}°",
                new SolidColorBrush(Color.FromArgb(0x73, 0xff, 0xff, 0xff)));
            double maxX = power == null ? r.Right - t.Width : powerLeft - t.Width - 8;
            dc.DrawText(t, new Point(Math.Min(x + 6, Math.Max(r.Left, maxX)), r.Top + 2));
        }

        // Curve points: soft halo under a crisp knob with a dark seat.
        var halo = new SolidColorBrush(Color.FromArgb(0x26, 0xff, 0xff, 0xff));
        var seat = new SolidColorBrush(CardBg);
        for (int i = 0; i < _channel.Points.Count; i++)
        {
            var c = ToScreen(_channel.Points[i].TempC, _channel.Points[i].Percent);
            dc.DrawEllipse(halo, null, c, 8.5, 8.5);
            dc.DrawEllipse(seat, null, c, 5.5, 5.5);
            dc.DrawEllipse(Brushes.White, null, c, 4, 4);
        }

        // The operating point — where the rolling average meets the commanded output.
        // Amber crosshair to both axes with readouts chipped over the axis labels.
        if (_liveEffective is double eff && !double.IsNaN(eff) && _liveOutput is double outPct)
        {
            var p = ToScreen(Math.Clamp(eff, TempMin, TempMax), Math.Clamp(outPct, 0, 100));
            var hairPen = new Pen(new SolidColorBrush(Color.FromArgb(0x4d, Amber.R, Amber.G, Amber.B)), 1);
            dc.DrawLine(hairPen, new Point(p.X, r.Bottom), p);
            dc.DrawLine(hairPen, new Point(r.Left, p.Y), p);

            var glow = new RadialGradientBrush(
                Color.FromArgb(0x55, Amber.R, Amber.G, Amber.B),
                Color.FromArgb(0x00, Amber.R, Amber.G, Amber.B));
            dc.DrawEllipse(glow, null, p, 16, 16);
            dc.DrawEllipse(new SolidColorBrush(Amber), null, p, 4.2, 4.2);
            dc.DrawEllipse(Brushes.White, null, p, 1.8, 1.8);

            var amberBrush = new SolidColorBrush(Amber);
            var chipBg = new SolidColorBrush(CardBg);

            // Temp readout on the x-axis…
            var tx = Label($"{eff.ToString("0.0", CultureInfo.InvariantCulture)}°", amberBrush);
            var txPos = new Point(
                Math.Clamp(p.X - tx.Width / 2, r.Left, Math.Max(r.Left, r.Right - tx.Width)), r.Bottom + 8);
            dc.DrawRectangle(chipBg, null,
                new Rect(txPos.X - 4, txPos.Y - 1, tx.Width + 8, tx.Height + 2));
            dc.DrawText(tx, txPos);

            // …and the fan % on the y-axis.
            var ty = Label($"{outPct.ToString("0", CultureInfo.InvariantCulture)}%", amberBrush);
            var tyPos = new Point(r.Left - ty.Width - 10,
                Math.Clamp(p.Y - ty.Height / 2, r.Top, Math.Max(r.Top, r.Bottom - ty.Height)));
            dc.DrawRectangle(chipBg, null,
                new Rect(tyPos.X - 4, tyPos.Y - 1, ty.Width + 8, ty.Height + 2));
            dc.DrawText(ty, tyPos);
        }

        // While dragging a point, echo its exact position: white hairline crosshair
        // with temp/% chips over the axis labels (amber stays live-data-only).
        if (_dragIndex >= 0 && _dragIndex < _channel.Points.Count)
        {
            var dp = _channel.Points[_dragIndex];
            DrawWhiteCrosshair(dc, r, Label, dp.TempC, dp.Percent, lineAlpha: 0x30, textAlpha: 0xf2);
        }
        // Plain hover (developer mode, not dragging): same crosshair, slightly dimmer,
        // reading out the temp/% under the cursor.
        else if (_hoverPos is Point hp && !IsReadOnly)
        {
            var (ht, hpct) = FromScreen(hp);
            DrawWhiteCrosshair(dc, r, Label, Math.Round(ht), Math.Round(hpct),
                lineAlpha: 0x20, textAlpha: 0xb3);
        }

        if (power != null)
        {
            var pos = new Point(powerLeft, r.Top + 2);
            dc.DrawRectangle(new SolidColorBrush(CardBg), null,
                new Rect(pos.X - 4, pos.Y - 1, power.Width + 8, power.Height + 2));
            dc.DrawText(power, pos);
        }
    }

    private void DrawWhiteCrosshair(DrawingContext dc, Rect r,
        Func<string, Brush?, double, FormattedText> label,
        double temp, double pct, byte lineAlpha, byte textAlpha)
    {
        var p = ToScreen(temp, pct);
        var hairPen = new Pen(new SolidColorBrush(Color.FromArgb(lineAlpha, 0xff, 0xff, 0xff)), 1);
        dc.DrawLine(hairPen, new Point(p.X, r.Bottom), p);
        dc.DrawLine(hairPen, new Point(r.Left, p.Y), p);

        var whiteBrush = new SolidColorBrush(Color.FromArgb(textAlpha, 0xff, 0xff, 0xff));
        var chipBg = new SolidColorBrush(CardBg);

        var tx = label($"{temp.ToString("0.#", CultureInfo.InvariantCulture)}°", whiteBrush, 10.5);
        var txPos = new Point(
            Math.Clamp(p.X - tx.Width / 2, r.Left, Math.Max(r.Left, r.Right - tx.Width)), r.Bottom + 8);
        dc.DrawRectangle(chipBg, null,
            new Rect(txPos.X - 4, txPos.Y - 1, tx.Width + 8, tx.Height + 2));
        dc.DrawText(tx, txPos);

        var ty = label($"{pct.ToString("0", CultureInfo.InvariantCulture)}%", whiteBrush, 10.5);
        var tyPos = new Point(r.Left - ty.Width - 10,
            Math.Clamp(p.Y - ty.Height / 2, r.Top, Math.Max(r.Top, r.Bottom - ty.Height)));
        dc.DrawRectangle(chipBg, null,
            new Rect(tyPos.X - 4, tyPos.Y - 1, ty.Width + 8, ty.Height + 2));
        dc.DrawText(ty, tyPos);
    }

    private int HitTestPoint(Point pos)
    {
        if (_channel == null) return -1;
        for (int i = 0; i < _channel.Points.Count; i++)
        {
            var p = _channel.Points[i];
            if ((ToScreen(p.TempC, p.Percent) - pos).Length <= 11) return i;
        }
        return -1;
    }

    protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
    {
        if (_channel == null || IsReadOnly) return;
        var pos = e.GetPosition(this);
        if (e.ClickCount == 2)
        {
            var (rawTemp, rawPct) = FromScreen(pos);
            double temp = Math.Round(rawTemp), pct = Math.Round(rawPct);
            // Keep every band at least 1 °C wide — also what the drag clamp assumes.
            if (_channel.Points.Count < 12 && HitTestPoint(pos) < 0
                && _channel.Points.All(p => Math.Abs(p.TempC - temp) >= 1))
            {
                _channel.Points.Add(new CurvePoint(temp, pct));
                _channel.Points.Sort((a, b) => a.TempC.CompareTo(b.TempC));
                CurveChanged?.Invoke();
                InvalidateVisual();
            }
            return;
        }
        _dragIndex = HitTestPoint(pos);
        if (_dragIndex >= 0) CaptureMouse();
    }

    protected override void OnMouseMove(MouseEventArgs e)
    {
        var pos = e.GetPosition(this);
        // Upper bound too: Ctrl+Z can shrink the list while a drag is in flight.
        if (_dragIndex < 0 || _channel == null || _dragIndex >= _channel.Points.Count)
        {
            var hover = !IsReadOnly && Plot.Contains(pos) ? pos : (Point?)null;
            if (hover != _hoverPos) { _hoverPos = hover; InvalidateVisual(); }
            return;
        }
        _hoverPos = null;
        var (temp, pct) = FromScreen(pos);
        var pts = _channel.Points;
        double minT = _dragIndex > 0 ? pts[_dragIndex - 1].TempC + 1 : TempMin;
        double maxT = _dragIndex < pts.Count - 1 ? pts[_dragIndex + 1].TempC - 1 : TempMax;
        if (minT > maxT) return; // neighbours from an old profile can sit <2 °C apart
        pts[_dragIndex] = new CurvePoint(Math.Clamp(Math.Round(temp), minT, maxT), Math.Round(pct));
        InvalidateVisual();
    }

    protected override void OnMouseLeftButtonUp(MouseButtonEventArgs e)
    {
        if (_dragIndex >= 0)
        {
            _dragIndex = -1;
            ReleaseMouseCapture();
            CurveChanged?.Invoke();
        }
    }

    protected override void OnMouseLeave(MouseEventArgs e)
    {
        if (_hoverPos != null) { _hoverPos = null; InvalidateVisual(); }
    }

    protected override void OnMouseRightButtonDown(MouseButtonEventArgs e)
    {
        if (_channel == null || IsReadOnly || _channel.Points.Count <= 2) return;
        int i = HitTestPoint(e.GetPosition(this));
        if (i >= 0)
        {
            _channel.Points.RemoveAt(i);
            CurveChanged?.Invoke();
            InvalidateVisual();
        }
    }
}
