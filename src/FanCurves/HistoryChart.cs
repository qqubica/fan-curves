using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace FanCurves;

/// <summary>One engine tick's worth of a channel's thermal state.</summary>
public readonly record struct HistorySample(double? RawTemp, double EffectiveTemp, double OutputPercent);

/// <summary>Fixed ring of the last <see cref="Capacity"/> samples (~10 min at the engine's jittered ~1 s tick).</summary>
public class ChannelHistory
{
    public const int Capacity = 600;
    private readonly HistorySample[] _buf = new HistorySample[Capacity];
    private int _next, _count;

    public int Count => _count;

    /// <summary>0 = oldest retained sample, Count-1 = newest.</summary>
    public HistorySample this[int i] => _buf[(_next - _count + i + Capacity) % Capacity];

    public void Add(HistorySample s)
    {
        _buf[_next] = s;
        _next = (_next + 1) % Capacity;
        if (_count < Capacity) _count++;
    }
}

/// <summary>
/// Rolling strip under the curve editor (developer mode only): the last 10 minutes of the
/// selected channel's rolling-average temperature, raw temperature and commanded fan %.
/// The trace grows in from the right (right edge = now). Hovering shows a crosshair with a
/// time · avg · now · % readout chip; only the newest sample carries amber (live-data-only,
/// like the curve's operating dot).
/// </summary>
public class HistoryChart : FrameworkElement
{
    private const double TempMin = 15, TempMax = 100;   // same span as CurveEditor
    private const double SecondsPerSample = 1;          // engine tick
    // Left/right match CurveEditor's padding so the two plots align vertically.
    private static readonly Thickness Pad = new(42, 22, 18, 20);

    private static readonly Color Amber = Color.FromRgb(0xFF, 0x9E, 0x5E);
    private static readonly Color CardBg = Color.FromRgb(0x11, 0x11, 0x16);

    public ChannelHistory? History { get; set; }

    private double? _hoverX;

    public void Refresh() => InvalidateVisual();

    private Rect Plot => new(
        Pad.Left, Pad.Top,
        Math.Max(10, ActualWidth - Pad.Left - Pad.Right),
        Math.Max(10, ActualHeight - Pad.Top - Pad.Bottom));

    private double PixelsPerSample => Plot.Width / (ChannelHistory.Capacity - 1);

    private double XForIndex(int i, int count) => Plot.Right - (count - 1 - i) * PixelsPerSample;
    private double YForPct(double p) => Plot.Bottom - Math.Clamp(p, 0, 100) / 100.0 * Plot.Height;
    private double YForTemp(double t) =>
        Plot.Bottom - (Math.Clamp(t, TempMin, TempMax) - TempMin) / (TempMax - TempMin) * Plot.Height;

    protected override void OnRender(DrawingContext dc)
    {
        // Transparent fill keeps hit-testing (hover) alive across the whole strip.
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

        // Hairline seam against the curve chart above + a letter-spaced strip title.
        dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x12, 0xff, 0xff, 0xff)), 1),
            new Point(r.Left, 0.5), new Point(r.Right, 0.5));
        var title = new FormattedText(Tracked.Space("HISTORY"), CultureInfo.InvariantCulture,
            FlowDirection.LeftToRight,
            new Typeface(new FontFamily("Segoe UI Variable Text, Segoe UI"),
                FontStyles.Normal, FontWeights.SemiBold, FontStretches.Normal),
            9, labelBrush, 1.25);
        dc.DrawText(title, new Point(r.Left, 7));

        // Legend, top-right: swatches for the three traces (kept monochrome, pens
        // mirror the actual trace styles so the mapping is readable at a glance).
        double lx = r.Right;
        void LegendItem(string text, Pen pen)
        {
            var t = Label(text, new SolidColorBrush(Color.FromArgb(0xa6, 0xff, 0xff, 0xff)), 10);
            lx -= t.Width;
            double cy = 6 + t.Height / 2;
            dc.DrawText(t, new Point(lx, 6));
            lx -= 22;
            dc.DrawLine(pen, new Point(lx, cy), new Point(lx + 17, cy));
            lx -= 18;
        }
        LegendItem("fan %", new Pen(new SolidColorBrush(Color.FromArgb(0x8c, 0xff, 0xff, 0xff)), 1.5));
        LegendItem("now temp", new Pen(new SolidColorBrush(Color.FromArgb(0x66, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) });
        LegendItem("avg temp", new Pen(new SolidColorBrush(Color.FromArgb(0xf0, 0xff, 0xff, 0xff)), 2)
            { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round });

        // Sparse horizontal grid on the % scale, labels on the left axis like the curve chart.
        for (double pct = 0; pct <= 100; pct += 50)
        {
            var y = YForPct(pct);
            dc.DrawLine(pct == 0 ? axisPen : gridPen, new Point(r.Left, y), new Point(r.Right, y));
            var t = Label(pct == 100 ? "100%" : $"{pct:0}");
            dc.DrawText(t, new Point(r.Left - t.Width - 10, y - t.Height / 2));
        }

        // Quiet temperature references inside the right edge (second scale, no gridlines).
        var tempRefBrush = new SolidColorBrush(Color.FromArgb(0x33, 0xff, 0xff, 0xff));
        foreach (double temp in new[] { 40.0, 60.0, 80.0 })
        {
            var t = Label($"{temp:0}°", tempRefBrush, 9.5);
            dc.DrawText(t, new Point(r.Right - t.Width - 4, YForTemp(temp) - t.Height / 2));
        }

        // Time axis: right edge is now, ticks every 5 minutes into the past.
        for (int s = 0; s <= 600; s += 300)
        {
            double x = r.Right - s / SecondsPerSample * PixelsPerSample;
            var t = Label(s == 0 ? "now" : Inv($"−{s / 60}:00"));
            dc.DrawText(t, new Point(
                Math.Clamp(x - t.Width / 2, r.Left, r.Right - t.Width), r.Bottom + 5));
        }

        var h = History;
        if (h == null || h.Count == 0) return;
        int count = h.Count;

        // Fan % trace: soft under-fill + a quiet line (same treatment as the curve's band fill).
        var pctGeo = new StreamGeometry();
        using (var ctx = pctGeo.Open())
        {
            ctx.BeginFigure(new Point(XForIndex(0, count), r.Bottom), true, true);
            for (int i = 0; i < count; i++)
                ctx.LineTo(new Point(XForIndex(i, count), YForPct(h[i].OutputPercent)), true, false);
            ctx.LineTo(new Point(r.Right, r.Bottom), false, false);
        }
        pctGeo.Freeze();
        dc.DrawGeometry(new LinearGradientBrush(
            Color.FromArgb(0x12, 0xff, 0xff, 0xff), Color.FromArgb(0x02, 0xff, 0xff, 0xff),
            new Point(0, 0), new Point(0, 1)), null, pctGeo);
        DrawTrace(dc, count, i => YForPct(h[i].OutputPercent),
            new Pen(new SolidColorBrush(Color.FromArgb(0x66, 0xff, 0xff, 0xff)), 1.5));

        // Raw ("now") temp: faint dashed reference under the average.
        DrawTrace(dc, count, i => h[i].RawTemp is double raw ? YForTemp(raw) : double.NaN,
            new Pen(new SolidColorBrush(Color.FromArgb(0x40, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) });

        // Rolling-average temp: the bright trace (it is what drives the steps).
        DrawTrace(dc, count, i => double.IsNaN(h[i].EffectiveTemp) ? double.NaN : YForTemp(h[i].EffectiveTemp),
            new Pen(new SolidColorBrush(Color.FromArgb(0xf0, 0xff, 0xff, 0xff)), 2)
            { LineJoin = PenLineJoin.Round, StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round });

        // Fan turn-ON events (commanded % leaves 0): tick at the baseline, and the
        // stopped time (previous turn-OFF → this turn-ON) written on a span between
        // them — makes the idle-kick / zero-snap cycling cadence readable at a glance.
        var onMarks = new List<int>();
        var offMarks = new List<int>();
        var offBefore = new List<int>();   // per on-mark: where the fan stopped, or -1 if before the window
        int lastOff = -1;
        for (int i = 1; i < count; i++)
        {
            if (h[i - 1].OutputPercent > 0 && h[i].OutputPercent <= 0) { offMarks.Add(i); lastOff = i; }
            else if (h[i - 1].OutputPercent <= 0 && h[i].OutputPercent > 0) { onMarks.Add(i); offBefore.Add(lastOff); }
        }
        if (onMarks.Count > 0 || offMarks.Count > 0)
        {
            var markPen = new Pen(new SolidColorBrush(Color.FromArgb(0x8c, 0xff, 0xff, 0xff)), 1);
            var offPen = new Pen(new SolidColorBrush(Color.FromArgb(0x46, 0xff, 0xff, 0xff)), 1);
            var spanPen = new Pen(new SolidColorBrush(Color.FromArgb(0x30, 0xff, 0xff, 0xff)), 1);
            var seatBrush = new SolidColorBrush(CardBg);
            foreach (int i in onMarks)
            {
                double x = XForIndex(i, count);
                dc.DrawLine(markPen, new Point(x, r.Bottom), new Point(x, r.Bottom - 6));
            }
            foreach (int i in offMarks)
            {
                double x = XForIndex(i, count);
                dc.DrawLine(offPen, new Point(x, r.Bottom), new Point(x, r.Bottom - 6));
            }
            // Stopped-time spans: one per completed stop (off → on), plus a live one that
            // counts up while the newest sample is still stopped. Labels seat on the card
            // background so they stay readable even when the gap is only a few pixels;
            // a label is only skipped when it would overlap the previous span's label.
            var spans = new List<(int Off, int End, bool Live)>();
            for (int k = 0; k < onMarks.Count; k++)
                if (offBefore[k] >= 0)   // stop that began before the visible window has no measurable start
                    spans.Add((offBefore[k], onMarks[k], false));
            if (h[count - 1].OutputPercent <= 0 && lastOff >= 0 &&
                (onMarks.Count == 0 || lastOff > onMarks[^1]))
                spans.Add((lastOff, count - 1, true));

            double spanY = r.Bottom - 14;
            double lastLabelRight = double.NegativeInfinity;
            foreach (var (off, end, live) in spans)
            {
                double x1 = XForIndex(off, count), x2 = XForIndex(end, count);
                if (x2 - x1 >= 8)
                {
                    dc.DrawLine(spanPen, new Point(x1 + 3, spanY), new Point(x2 - 3, spanY));
                    dc.DrawLine(spanPen, new Point(x1 + 3, spanY - 3), new Point(x1 + 3, spanY + 3));
                    if (!live)   // an ongoing stop has no end tick — the right edge is "now"
                        dc.DrawLine(spanPen, new Point(x2 - 3, spanY - 3), new Point(x2 - 3, spanY + 3));
                }
                int sec = (int)Math.Round((end - off) * SecondsPerSample);
                var t = Label(Inv($"{sec / 60}:{sec % 60:00}"),
                    new SolidColorBrush(Color.FromArgb(0xa6, 0xff, 0xff, 0xff)), 10);
                double tx = Math.Clamp((x1 + x2) / 2 - t.Width / 2, r.Left, r.Right - t.Width);
                if (tx - 3 <= lastLabelRight + 4) continue;   // would collide with the previous label
                var tp = new Point(tx, spanY - t.Height / 2);
                dc.DrawRectangle(seatBrush, null, new Rect(tp.X - 3, tp.Y, t.Width + 6, t.Height));
                dc.DrawText(t, tp);
                lastLabelRight = tp.X + t.Width + 3;
            }
        }

        // The newest sample is live thermal state — the one place amber is allowed here.
        var newest = h[count - 1];
        double nx = XForIndex(count - 1, count);
        var amberBrush = new SolidColorBrush(Amber);
        if (!double.IsNaN(newest.EffectiveTemp))
            dc.DrawEllipse(amberBrush, null, new Point(nx, YForTemp(newest.EffectiveTemp)), 2.6, 2.6);
        dc.DrawEllipse(amberBrush, null, new Point(nx, YForPct(newest.OutputPercent)), 2.2, 2.2);

        // Hover "ticket": crosshair at the nearest sample with a time · temp · % chip.
        if (_hoverX is double hx && hx >= XForIndex(0, count) - 4 && hx <= r.Right + 4)
        {
            int idx = count - 1 - (int)Math.Round((r.Right - Math.Clamp(hx, r.Left, r.Right)) / PixelsPerSample);
            idx = Math.Clamp(idx, 0, count - 1);
            var s = h[idx];
            double x = XForIndex(idx, count);

            dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x30, 0xff, 0xff, 0xff)), 1),
                new Point(x, r.Top), new Point(x, r.Bottom));

            var seat = new SolidColorBrush(CardBg);
            void Marker(double y)
            {
                dc.DrawEllipse(seat, null, new Point(x, y), 4.4, 4.4);
                dc.DrawEllipse(Brushes.White, null, new Point(x, y), 3, 3);
            }
            Marker(YForPct(s.OutputPercent));
            if (!double.IsNaN(s.EffectiveTemp)) Marker(YForTemp(s.EffectiveTemp));

            int ago = (int)Math.Round((count - 1 - idx) * SecondsPerSample);
            string when = ago == 0 ? "now" : Inv($"−{ago / 60}:{ago % 60:00}");
            string avgStr = double.IsNaN(s.EffectiveTemp) ? "—" : Inv($"{s.EffectiveTemp:0.0}°");
            string rawStr = s.RawTemp is double rw ? Inv($"{rw:0.0}°") : "—";
            var chip = Label(Inv($"{when} · avg {avgStr} · now {rawStr} · {s.OutputPercent:0}%"),
                new SolidColorBrush(Color.FromArgb(0xf2, 0xff, 0xff, 0xff)));
            var pos = new Point(Math.Clamp(x - chip.Width / 2, r.Left, r.Right - chip.Width), r.Top + 3);
            dc.DrawRoundedRectangle(new SolidColorBrush(CardBg),
                new Pen(new SolidColorBrush(Color.FromArgb(0x26, 0xff, 0xff, 0xff)), 1),
                new Rect(pos.X - 6, pos.Y - 3, chip.Width + 12, chip.Height + 6), 6, 6);
            dc.DrawText(chip, pos);
        }
    }

    /// <summary>Polyline over the samples; NaN breaks the trace (e.g. missing sensor).</summary>
    private void DrawTrace(DrawingContext dc, int count, Func<int, double> yFor, Pen pen)
    {
        Point? prev = null;
        for (int i = 0; i < count; i++)
        {
            double y = yFor(i);
            if (double.IsNaN(y)) { prev = null; continue; }
            var p = new Point(XForIndex(i, count), y);
            if (prev is Point q) dc.DrawLine(pen, q, p);
            prev = p;
        }
        // A single retained sample still shows as a dot instead of nothing.
        if (count == 1 && !double.IsNaN(yFor(0)))
            dc.DrawEllipse(pen.Brush, null, new Point(XForIndex(0, count), yFor(0)), 1.6, 1.6);
    }

    private static string Inv(FormattableString f) => FormattableString.Invariant(f);

    protected override void OnMouseMove(MouseEventArgs e)
    {
        _hoverX = e.GetPosition(this).X;
        InvalidateVisual();
    }

    protected override void OnMouseLeave(MouseEventArgs e)
    {
        _hoverX = null;
        InvalidateVisual();
    }
}
