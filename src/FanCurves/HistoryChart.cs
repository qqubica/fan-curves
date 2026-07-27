using System.Windows;
using System.Windows.Media;

namespace FanCurves;

/// <summary>
/// One engine tick's worth of a channel's state. The temperature/output fields feed the
/// history strip; the rest is thermal-budget telemetry for the budget strip (defaults
/// apply on channels the budget controller does not drive).
/// </summary>
public readonly record struct HistorySample(
    double? RawTemp,
    double EffectiveTemp,
    double OutputPercent,
    double? Watts = null,           // instantaneous channel draw
    double? WattsAvg = null,        // sustained average — the demand signal
    double BudgetJoules = 0,        // energy credit left before the ceiling
    double TauSeconds = double.PositiveInfinity, // predicted headroom
    double DemandLevel = 0,         // ladder level the power average asks for
    double CeilingC = 0,            // budget ceiling in force
    double AimC = 0,                // sustained aim in force — headroom is measured to it
    bool Override = false);         // fuse latched (raw die at/over the override temp)

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

    /// <summary>Forget everything; the strips grow in from the right again.</summary>
    public void Clear() { _next = 0; _count = 0; }
}

/// <summary>
/// Rolling strip under the curve editor (developer mode only): the last 10 minutes of the
/// selected channel's rolling-average temperature, raw temperature and commanded fan %.
/// The trace grows in from the right (right edge = now). Hovering shows a crosshair with a
/// time · avg · now · % readout chip; only the newest sample carries amber (live-data-only,
/// like the curve's operating dot).
/// </summary>
public class HistoryChart : StripChart
{
    private const double TempMin = 15, TempMax = 100;   // same span as CurveEditor

    private double YForPct(double p) => Plot.Bottom - Math.Clamp(p, 0, 100) / 100.0 * Plot.Height;
    private double YForTemp(double t) =>
        Plot.Bottom - (Math.Clamp(t, TempMin, TempMax) - TempMin) / (TempMax - TempMin) * Plot.Height;

    protected override void OnRender(DrawingContext dc)
    {
        if (TooSmallToRender) return;
        // Transparent fill keeps hit-testing (hover) alive across the whole strip.
        dc.DrawRectangle(Brushes.Transparent, null, new Rect(0, 0, ActualWidth, ActualHeight));
        var r = Plot;

        var gridPen = new Pen(new SolidColorBrush(Color.FromArgb(0x0d, 0xff, 0xff, 0xff)), 1);
        var axisPen = new Pen(new SolidColorBrush(Color.FromArgb(0x1a, 0xff, 0xff, 0xff)), 1);

        DrawFrame(dc, "HISTORY");
        DrawLegend(dc,
            ("fan %", new Pen(new SolidColorBrush(Color.FromArgb(0x8c, 0xff, 0xff, 0xff)), 1.5)),
            ("now temp", new Pen(new SolidColorBrush(Color.FromArgb(0x66, 0xff, 0xff, 0xff)), 1)
                { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) }),
            ("avg temp", new Pen(new SolidColorBrush(Color.FromArgb(0xf0, 0xff, 0xff, 0xff)), 2)
                { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round }));

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

        var h = History;
        if (h == null || h.Count == 0) return;
        int count = h.Count;

        // Budget ceiling (power control only): the temperature the thermal credit is
        // measured against — the trace approaching it is what makes the fans step up.
        // (its label goes on last, over the traces)
        double ceiling = h[count - 1].CeilingC;
        if (ceiling > 0)
            dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x2e, 0xff, 0xff, 0xff)), 1)
                { DashStyle = new DashStyle(new double[] { 1, 4 }, 0) },
                new Point(r.Left, YForTemp(ceiling)), new Point(r.Right, YForTemp(ceiling)));

        // Sustained aim (power control only): the line the controller actually defends —
        // headroom counts down to it and the fans engage before the trend settles past it.
        // Dimmer than the ceiling; skipped when the margins make the two coincide.
        double aim = h[count - 1].AimC;
        if (aim > 0 && aim < ceiling - 0.5)
            dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x20, 0xff, 0xff, 0xff)), 1)
                { DashStyle = new DashStyle(new double[] { 1, 4 }, 0) },
                new Point(r.Left, YForTemp(aim)), new Point(r.Right, YForTemp(aim)));

        // Fan % trace: soft under-fill + a quiet line (same treatment as the curve's band fill).
        DrawUnderFill(dc, count, i => YForPct(h[i].OutputPercent), new LinearGradientBrush(
            Color.FromArgb(0x12, 0xff, 0xff, 0xff), Color.FromArgb(0x02, 0xff, 0xff, 0xff),
            new Point(0, 0), new Point(0, 1)));
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
                double tx = Math.Clamp((x1 + x2) / 2 - t.Width / 2, r.Left, Math.Max(r.Left, r.Right - t.Width));
                if (tx - 3 <= lastLabelRight + 4) continue;   // would collide with the previous label
                var tp = new Point(tx, spanY - t.Height / 2);
                dc.DrawRectangle(seatBrush, null, new Rect(tp.X - 3, tp.Y, t.Width + 6, t.Height));
                dc.DrawText(t, tp);
                lastLabelRight = tp.X + t.Width + 3;
            }
        }

        if (ceiling > 0)
        {
            var t = Label(Inv($"ceiling {ceiling:0}°"), tempRefBrush, 9.5);
            DrawSeatedText(dc, t, new Point(r.Left + 4, YForTemp(ceiling) - t.Height - 1));
        }
        if (aim > 0 && aim < ceiling - 0.5)
        {
            // Below its line (the ceiling label sits above its own), so the two never
            // collide even when the margins put the lines a few pixels apart.
            var t = Label(Inv($"aim {aim:0}°"), tempRefBrush, 9.5);
            DrawSeatedText(dc, t, new Point(r.Left + 4, YForTemp(aim) + 2));
        }

        // The newest sample is live thermal state — the one place amber is allowed here.
        var newest = h[count - 1];
        double nx = XForIndex(count - 1, count);
        var amberBrush = new SolidColorBrush(Amber);
        if (!double.IsNaN(newest.EffectiveTemp))
            dc.DrawEllipse(amberBrush, null, new Point(nx, YForTemp(newest.EffectiveTemp)), 2.6, 2.6);
        dc.DrawEllipse(amberBrush, null, new Point(nx, YForPct(newest.OutputPercent)), 2.2, 2.2);

        // Hover "ticket": crosshair at the nearest sample with a time · temp · % chip.
        if (HoverIndex(count) is int idx)
        {
            var s = h[idx];
            double x = XForIndex(idx, count);
            DrawCrosshair(dc, x);
            DrawMarker(dc, new Point(x, YForPct(s.OutputPercent)));
            if (!double.IsNaN(s.EffectiveTemp)) DrawMarker(dc, new Point(x, YForTemp(s.EffectiveTemp)));

            string avgStr = double.IsNaN(s.EffectiveTemp) ? "—" : Inv($"{s.EffectiveTemp:0.0}°");
            string rawStr = s.RawTemp is double rw ? Inv($"{rw:0.0}°") : "—";
            DrawChip(dc, x,
                Inv($"{Ago(idx, count)} · avg {avgStr} · now {rawStr} · {s.OutputPercent:0}%"),
                Inv($"{Ago(idx, count)} · avg {avgStr}\nnow {rawStr} · {s.OutputPercent:0}%"));
        }
    }
}
