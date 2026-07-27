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
///
/// Two axes share the one editor: the temperature staircase (°C → %, the default)
/// and the power staircase (W → %, PowerCurve mode's ladder) — switched by
/// PowerAxis. All editing logic is axis-generic; only the bounds, snapping and
/// overlays differ. On the power axis the live overlays are the sustained draw
/// (operating dot, white — amber stays reserved for thermal state) and the
/// instantaneous draw (dashed vertical).
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
    private bool _powerAxis;

    /// <summary>When true the curve is an illustration only (simple mode).</summary>
    public bool IsReadOnly { get; set; } = true;

    /// <summary>Developer mode: also overlay the raw (unaveraged) temperature.</summary>
    public bool ShowRaw { get; set; }

    /// <summary>False = temperature staircase (°C), true = power staircase (W).</summary>
    public bool PowerAxis
    {
        get => _powerAxis;
        set
        {
            if (_powerAxis == value) return;
            _powerAxis = value;
            _dragIndex = -1;
            _hoverPos = null;
            InvalidateVisual();
        }
    }

    private double? _liveRaw, _liveEffective, _liveOutput;
    private double? _liveWatts, _liveWattsAvg;
    private double _wattsPeak; // recent-history peak — keeps the right-hand watts scale steady

    public event Action? CurveChanged;

    public ChannelConfig? Channel
    {
        get => _channel;
        set { _channel = value; InvalidateVisual(); }
    }

    public void UpdateLive(double? raw, double? effective, double? output,
        double? watts = null, double? wattsAvg = null, double wattsPeak = 0)
    {
        _liveRaw = raw;
        _liveEffective = effective;
        _liveOutput = output;
        _liveWatts = watts;
        _liveWattsAvg = wattsAvg;
        _wattsPeak = wattsPeak;
        InvalidateVisual();
    }

    // ---- axis abstraction ----

    private double XMin => _powerAxis ? 0 : TempMin;
    private double XMax => _powerAxis ? PowerMax() : TempMax;
    /// <summary>Edits snap to whole °C on the temp axis, 5 W steps on the power axis.</summary>
    private double SnapStep => _powerAxis ? 5 : 1;
    /// <summary>Neighbouring points keep at least one snap step between them.</summary>
    private double MinGap => SnapStep;

    /// <summary>Power axis top: at least 300 W, and always headroom above the topmost
    /// point so it stays draggable to the right.</summary>
    private double PowerMax()
    {
        double top = 250;
        if (_channel != null)
            foreach (var p in _channel.PowerPoints) top = Math.Max(top, p.Watts);
        return Math.Ceiling((top + 50) / 50) * 50;
    }

    private int PtCount => _channel == null ? 0
        : _powerAxis ? _channel.PowerPoints.Count : _channel.Points.Count;

    private (double X, double Pct) Pt(int i) => _powerAxis
        ? (_channel!.PowerPoints[i].Watts, _channel.PowerPoints[i].Percent)
        : (_channel!.Points[i].TempC, _channel.Points[i].Percent);

    private void SetPt(int i, double x, double pct)
    {
        if (_powerAxis) _channel!.PowerPoints[i] = new PowerPoint(x, pct);
        else _channel!.Points[i] = new CurvePoint(x, pct);
    }

    private void AddPt(double x, double pct)
    {
        if (_powerAxis)
        {
            _channel!.PowerPoints.Add(new PowerPoint(x, pct));
            _channel.PowerPoints.Sort((a, b) => a.Watts.CompareTo(b.Watts));
        }
        else
        {
            _channel!.Points.Add(new CurvePoint(x, pct));
            _channel.Points.Sort((a, b) => a.TempC.CompareTo(b.TempC));
        }
    }

    private void RemovePt(int i)
    {
        if (_powerAxis) _channel!.PowerPoints.RemoveAt(i);
        else _channel!.Points.RemoveAt(i);
    }

    private string FormatX(double x) => _powerAxis
        ? Inv($"{x:0} W")
        : Inv($"{x:0.#}°");

    private static string Inv(FormattableString f) => FormattableString.Invariant(f);

    private Rect Plot => new(
        Pad.Left, Pad.Top,
        Math.Max(10, ActualWidth - Pad.Left - Pad.Right),
        Math.Max(10, ActualHeight - Pad.Top - Pad.Bottom));

    private Point ToScreen(double x, double pct)
    {
        var r = Plot;
        return new Point(
            r.Left + (x - XMin) / (XMax - XMin) * r.Width,
            r.Bottom - pct / 100.0 * r.Height);
    }

    private (double x, double pct) FromScreen(Point p)
    {
        var r = Plot;
        return (
            XMin + Math.Clamp((p.X - r.Left) / r.Width, 0, 1) * (XMax - XMin),
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

        // Sparse horizontal grid; X positions are labels only — no vertical cage.
        for (double pct = 0; pct <= 100; pct += 25)
        {
            var y = ToScreen(XMin, pct).Y;
            dc.DrawLine(pct == 0 ? axisPen : gridPen, new Point(r.Left, y), new Point(r.Right, y));
            var t = Label(pct == 100 ? "100%" : $"{pct:0}");
            dc.DrawText(t, new Point(r.Left - t.Width - 10, y - t.Height / 2));
        }
        if (_powerAxis)
        {
            double step = XMax <= 500 ? 100 : 200;
            for (double gw = step; gw <= XMax + 0.1; gw += step)
            {
                var x = ToScreen(gw, 0).X;
                var t = Label(gw + step > XMax + 0.1 ? Inv($"{gw:0} W") : Inv($"{gw:0}"));
                // The last label carries the unit and would otherwise clip at the edge.
                dc.DrawText(t, new Point(
                    Math.Min(x - t.Width / 2, ActualWidth - t.Width - 2), r.Bottom + 8));
            }
        }
        else
        {
            for (double temp = 20; temp <= 100; temp += 20)
            {
                var x = ToScreen(temp, 0).X;
                var t = Label($"{temp:0}°");
                dc.DrawText(t, new Point(x - t.Width / 2, r.Bottom + 8));
            }
        }

        if (_channel == null || PtCount == 0) return;
        var pts = Enumerable.Range(0, PtCount).Select(Pt).OrderBy(p => p.X).ToList();

        // Staircase: each point opens a flat band that runs until the next point's X.
        var stairs = new List<Point> { ToScreen(XMin, pts[0].Pct) };
        for (int i = 0; i < pts.Count; i++)
        {
            double bandEnd = i < pts.Count - 1 ? pts[i + 1].X : XMax;
            stairs.Add(ToScreen(pts[i].X, pts[i].Pct)); // vertical jump lands here
            stairs.Add(ToScreen(bandEnd, pts[i].Pct));  // flat band
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

        // Power draw on the TEMP axis, developer mode (power-controlled channels
        // only): the mirror of the raw-temp line — HORIZONTAL reference lines read
        // against a watts scale on the RIGHT (0 W at the bottom, a NiceWatts ladder
        // top shared with the budget strip; the recent-history peak keeps the scale
        // from breathing with every sample). Lines draw here, under the knobs and
        // the operating dot; their labels draw LAST, seated on the card colour, so
        // the staircase and crosshairs never run through them. On the POWER axis the
        // watts are the X dimension itself, so these lines don't apply there.
        FormattedText? scaleMark = null, drawLabel = null, avgLabel = null;
        double scaleLeft = r.Right, yDraw = 0, yAvg = 0;
        if (!_powerAxis && ShowRaw && _liveWatts is double w)
        {
            double wa = _liveWattsAvg ?? w;
            double scale = BudgetChart.NiceWatts(Math.Max(25, Math.Max(_wattsPeak, Math.Max(w, wa))));
            double YOf(double watts) => r.Bottom - Math.Clamp(watts / scale, 0, 1) * r.Height;
            yDraw = YOf(w);
            yAvg = YOf(wa);

            // Instantaneous draw mirrors the raw-temp line (faint, dashed); the
            // sustained average is the brighter solid one — same vocabulary as the
            // budget strip's traces.
            var drawPen = new Pen(new SolidColorBrush(Color.FromArgb(0x40, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) };
            var avgPen = new Pen(new SolidColorBrush(Color.FromArgb(0x59, 0xff, 0xff, 0xff)), 1);
            dc.DrawLine(drawPen, new Point(r.Left, yDraw), new Point(r.Right, yDraw));
            dc.DrawLine(avgPen, new Point(r.Left, yAvg), new Point(r.Right, yAvg));

            var quiet = new SolidColorBrush(Color.FromArgb(0x73, 0xff, 0xff, 0xff));
            drawLabel = Label(Inv($"draw {w:0} W"), quiet);
            avgLabel = Label(Inv($"avg {wa:0} W"), quiet);
            scaleMark = Label(Inv($"{scale:0} W"));
            scaleLeft = Math.Max(r.Left, r.Right - scaleMark.Width - 4);
        }

        // Raw (unaveraged) temp, developer mode, temp axis: quiet dashed reference
        // line. Amber, like every other live-thermal readout (dot, crosshair) —
        // temperature is the amber quantity, power stays monochrome, so the two
        // families of reference lines never read as each other.
        if (!_powerAxis && ShowRaw && _liveRaw is double raw)
        {
            var pen = new Pen(new SolidColorBrush(Color.FromArgb(0x59, Amber.R, Amber.G, Amber.B)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) };
            var x = ToScreen(Math.Clamp(raw, TempMin, TempMax), 0).X;
            dc.DrawLine(pen, new Point(x, r.Top), new Point(x, r.Bottom));
            var t = Label($"now {raw.ToString("0.0", CultureInfo.InvariantCulture)}°",
                new SolidColorBrush(Color.FromArgb(0x99, Amber.R, Amber.G, Amber.B)));
            double maxX = scaleMark == null ? r.Right - t.Width : scaleLeft - t.Width - 8;
            dc.DrawText(t, new Point(Math.Min(x + 6, Math.Max(r.Left, maxX)), r.Top + 2));
        }

        // Power axis: the instantaneous draw is the dashed vertical reference (the
        // raw-temp line's role), monochrome by the amber-is-thermal rule.
        if (_powerAxis && ShowRaw && _liveWatts is double wNowLine)
        {
            var pen = new Pen(new SolidColorBrush(Color.FromArgb(0x40, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) };
            var x = ToScreen(Math.Clamp(wNowLine, XMin, XMax), 0).X;
            dc.DrawLine(pen, new Point(x, r.Top), new Point(x, r.Bottom));
            var t = Label(Inv($"now {wNowLine:0} W"),
                new SolidColorBrush(Color.FromArgb(0x73, 0xff, 0xff, 0xff)));
            dc.DrawText(t, new Point(
                Math.Min(x + 6, Math.Max(r.Left, r.Right - t.Width)), r.Top + 2));
        }

        // Curve points: soft halo under a crisp knob with a dark seat.
        var halo = new SolidColorBrush(Color.FromArgb(0x26, 0xff, 0xff, 0xff));
        var seat = new SolidColorBrush(CardBg);
        for (int i = 0; i < PtCount; i++)
        {
            var (px, ppct) = Pt(i);
            var c = ToScreen(px, ppct);
            dc.DrawEllipse(halo, null, c, 8.5, 8.5);
            dc.DrawEllipse(seat, null, c, 5.5, 5.5);
            dc.DrawEllipse(Brushes.White, null, c, 4, 4);
        }

        // The operating point — where the driving average meets the commanded output.
        // Temp axis: rolling-average temp, amber (live thermal state). Power axis:
        // sustained draw average, white (power stays monochrome).
        double? liveX = _powerAxis ? _liveWattsAvg : _liveEffective;
        if (liveX is double eff && !double.IsNaN(eff) && _liveOutput is double outPct)
        {
            var p = ToScreen(Math.Clamp(eff, XMin, XMax), Math.Clamp(outPct, 0, 100));
            Color dotC = _powerAxis ? Color.FromRgb(0xff, 0xff, 0xff) : Amber;
            byte hairA = _powerAxis ? (byte)0x38 : (byte)0x4d;
            var hairPen = new Pen(new SolidColorBrush(Color.FromArgb(hairA, dotC.R, dotC.G, dotC.B)), 1);
            dc.DrawLine(hairPen, new Point(p.X, r.Bottom), p);
            dc.DrawLine(hairPen, new Point(r.Left, p.Y), p);

            var glow = new RadialGradientBrush(
                Color.FromArgb(_powerAxis ? (byte)0x33 : (byte)0x55, dotC.R, dotC.G, dotC.B),
                Color.FromArgb(0x00, dotC.R, dotC.G, dotC.B));
            dc.DrawEllipse(glow, null, p, 16, 16);
            dc.DrawEllipse(new SolidColorBrush(dotC), null, p, 4.2, 4.2);
            dc.DrawEllipse(_powerAxis ? seat : Brushes.White, null, p, 1.8, 1.8);

            var chipBrush = new SolidColorBrush(_powerAxis
                ? Color.FromArgb(0xd9, 0xff, 0xff, 0xff) : Amber);
            var chipBg = new SolidColorBrush(CardBg);

            // X readout on the bottom axis…
            var tx = Label(_powerAxis
                ? Inv($"{eff:0} W")
                : $"{eff.ToString("0.0", CultureInfo.InvariantCulture)}°", chipBrush);
            var txPos = new Point(
                Math.Clamp(p.X - tx.Width / 2, r.Left, Math.Max(r.Left, r.Right - tx.Width)), r.Bottom + 8);
            dc.DrawRectangle(chipBg, null,
                new Rect(txPos.X - 4, txPos.Y - 1, tx.Width + 8, tx.Height + 2));
            dc.DrawText(tx, txPos);

            // …and the fan % on the y-axis.
            var ty = Label($"{outPct.ToString("0", CultureInfo.InvariantCulture)}%", chipBrush);
            var tyPos = new Point(r.Left - ty.Width - 10,
                Math.Clamp(p.Y - ty.Height / 2, r.Top, Math.Max(r.Top, r.Bottom - ty.Height)));
            dc.DrawRectangle(chipBg, null,
                new Rect(tyPos.X - 4, tyPos.Y - 1, ty.Width + 8, ty.Height + 2));
            dc.DrawText(ty, tyPos);
        }

        // While dragging a point, echo its exact position: white hairline crosshair
        // with X/% chips over the axis labels (amber stays live-data-only).
        if (_dragIndex >= 0 && _dragIndex < PtCount)
        {
            var (dx, dpct) = Pt(_dragIndex);
            DrawWhiteCrosshair(dc, r, Label, dx, dpct, lineAlpha: 0x30, textAlpha: 0xf2);
        }
        // Plain hover (developer mode, not dragging): same crosshair, slightly dimmer,
        // reading out the X/% under the cursor.
        else if (_hoverPos is Point hp && !IsReadOnly)
        {
            var (hx, hpct) = FromScreen(hp);
            DrawWhiteCrosshair(dc, r, Label, SnapX(hx), Math.Round(hpct),
                lineAlpha: 0x20, textAlpha: 0xb3);
        }

        if (scaleMark != null && drawLabel != null && avgLabel != null)
        {
            var chipBg = new SolidColorBrush(CardBg);
            void Chip(FormattedText t, Point pos)
            {
                dc.DrawRectangle(chipBg, null,
                    new Rect(pos.X - 4, pos.Y - 1, t.Width + 8, t.Height + 2));
                dc.DrawText(t, pos);
            }

            // Top of the watts scale, centred on the 100% gridline like the left
            // axis labels — everything below it reads on this 0..top range.
            Chip(scaleMark, new Point(scaleLeft, r.Top - scaleMark.Height / 2));

            // avg seats ABOVE its line, draw BELOW (the ceiling/aim rule from the
            // history strip); when the two would still collide — draw and avg a few
            // watts apart — they merge into one chip under the lower line.
            double LabX(FormattedText t) => Math.Max(r.Left, r.Right - t.Width - 4);
            double top = r.Top + scaleMark.Height + 2;
            double bottom = Math.Max(top, r.Bottom - avgLabel.Height);
            var avgPos = new Point(LabX(avgLabel),
                Math.Clamp(yAvg - avgLabel.Height - 2, top, bottom));
            var drawPos = new Point(LabX(drawLabel),
                Math.Clamp(yDraw + 3, top, bottom));
            bool collide = Math.Abs(avgPos.Y - drawPos.Y) < avgLabel.Height + 2;
            if (collide)
            {
                double wNow = _liveWatts!.Value, wAvg = _liveWattsAvg ?? wNow;
                var both = Label(Inv($"draw {wNow:0} · avg {wAvg:0} W"),
                    new SolidColorBrush(Color.FromArgb(0x73, 0xff, 0xff, 0xff)));
                Chip(both, new Point(LabX(both),
                    Math.Clamp(Math.Max(yDraw, yAvg) + 3, top, bottom)));
            }
            else
            {
                Chip(avgLabel, avgPos);
                Chip(drawLabel, drawPos);
            }
        }
    }

    private void DrawWhiteCrosshair(DrawingContext dc, Rect r,
        Func<string, Brush?, double, FormattedText> label,
        double x, double pct, byte lineAlpha, byte textAlpha)
    {
        var p = ToScreen(x, pct);
        var hairPen = new Pen(new SolidColorBrush(Color.FromArgb(lineAlpha, 0xff, 0xff, 0xff)), 1);
        dc.DrawLine(hairPen, new Point(p.X, r.Bottom), p);
        dc.DrawLine(hairPen, new Point(r.Left, p.Y), p);

        var whiteBrush = new SolidColorBrush(Color.FromArgb(textAlpha, 0xff, 0xff, 0xff));
        var chipBg = new SolidColorBrush(CardBg);

        var tx = label(FormatX(x), whiteBrush, 10.5);
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

    private double SnapX(double x) => Math.Round(x / SnapStep) * SnapStep;

    private int HitTestPoint(Point pos)
    {
        for (int i = 0; i < PtCount; i++)
        {
            var (x, pct) = Pt(i);
            if ((ToScreen(x, pct) - pos).Length <= 11) return i;
        }
        return -1;
    }

    protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
    {
        if (_channel == null || IsReadOnly) return;
        var pos = e.GetPosition(this);
        if (e.ClickCount == 2)
        {
            var (rawX, rawPct) = FromScreen(pos);
            double x = SnapX(rawX), pct = Math.Round(rawPct);
            // Keep every band at least one snap step wide — the drag clamp assumes it.
            if (PtCount < 12 && HitTestPoint(pos) < 0
                && Enumerable.Range(0, PtCount).All(i => Math.Abs(Pt(i).X - x) >= MinGap))
            {
                AddPt(x, pct);
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
        if (_dragIndex < 0 || _channel == null || _dragIndex >= PtCount)
        {
            var hover = !IsReadOnly && Plot.Contains(pos) ? pos : (Point?)null;
            if (hover != _hoverPos) { _hoverPos = hover; InvalidateVisual(); }
            return;
        }
        _hoverPos = null;
        var (x, pct) = FromScreen(pos);
        double minX = _dragIndex > 0 ? Pt(_dragIndex - 1).X + MinGap : XMin;
        double maxX = _dragIndex < PtCount - 1 ? Pt(_dragIndex + 1).X - MinGap : XMax;
        if (minX > maxX) return; // neighbours from an old profile can sit under a gap apart
        SetPt(_dragIndex, Math.Clamp(SnapX(x), minX, maxX), Math.Round(pct));
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
        if (_channel == null || IsReadOnly || PtCount <= 2) return;
        int i = HitTestPoint(e.GetPosition(this));
        if (i >= 0)
        {
            RemovePt(i);
            CurveChanged?.Invoke();
            InvalidateVisual();
        }
    }
}
