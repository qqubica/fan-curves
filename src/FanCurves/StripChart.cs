using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace FanCurves;

/// <summary>
/// Shared skeleton of the rolling strips under the curve chart (thermal history and
/// thermal budget): identical padding to <see cref="CurveEditor"/> so every plot lines
/// up vertically, the letter-spaced strip title, the legend, the shared time axis
/// (right edge = now, ticks every 5 minutes back) and the hover crosshair. Subclasses
/// add their own scales, traces and hover chip.
/// </summary>
public abstract class StripChart : FrameworkElement
{
    protected const double SecondsPerSample = 1;          // engine tick
    // Left/right match CurveEditor's padding so the plots align vertically.
    protected static readonly Thickness Pad = new(42, 22, 18, 20);

    protected static readonly Color Amber = Color.FromRgb(0xFF, 0x9E, 0x5E);
    protected static readonly Color CardBg = Color.FromRgb(0x11, 0x11, 0x16);

    private static readonly Typeface Mono = new(new FontFamily("Cascadia Mono, Consolas"),
        FontStyles.Normal, FontWeights.Normal, FontStretches.Normal);

    public ChannelHistory? History { get; set; }

    private double? _hoverX;

    public void Refresh() => InvalidateVisual();

    protected Rect Plot => new(
        Pad.Left, Pad.Top,
        Math.Max(10, ActualWidth - Pad.Left - Pad.Right),
        Math.Max(10, ActualHeight - Pad.Top - Pad.Bottom));

    protected double PixelsPerSample => Plot.Width / (ChannelHistory.Capacity - 1);

    protected double XForIndex(int i, int count) => Plot.Right - (count - 1 - i) * PixelsPerSample;

    protected static string Inv(FormattableString f) => FormattableString.Invariant(f);

    protected static readonly Brush LabelBrush = new SolidColorBrush(Color.FromArgb(0x59, 0xff, 0xff, 0xff));

    protected FormattedText Label(string s, Brush? brush = null, double size = 10.5) => new(
        s, CultureInfo.InvariantCulture, FlowDirection.LeftToRight, Mono, size,
        brush ?? LabelBrush, 1.25);

    /// <summary>Hairline seam against the plot above, the strip title, and the time axis.</summary>
    protected void DrawFrame(DrawingContext dc, string title)
    {
        var r = Plot;
        dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x12, 0xff, 0xff, 0xff)), 1),
            new Point(r.Left, 0.5), new Point(r.Right, 0.5));
        var t = new FormattedText(Tracked.Space(title), CultureInfo.InvariantCulture,
            FlowDirection.LeftToRight,
            new Typeface(new FontFamily("Segoe UI Variable Text, Segoe UI"),
                FontStyles.Normal, FontWeights.SemiBold, FontStretches.Normal),
            9, LabelBrush, 1.25);
        dc.DrawText(t, new Point(r.Left, 7));

        // Right edge is now; ticks every 5 minutes into the past.
        for (int s = 0; s <= 600; s += 300)
        {
            double x = r.Right - s / SecondsPerSample * PixelsPerSample;
            var lab = Label(s == 0 ? "now" : Inv($"−{s / 60}:00"));
            dc.DrawText(lab, new Point(
                Math.Clamp(x - lab.Width / 2, r.Left, r.Right - lab.Width), r.Bottom + 5));
        }
    }

    /// <summary>
    /// Legend at the top-right, laid out right to left in call order (so the first
    /// entry ends up rightmost). The pens mirror the actual trace styles.
    /// </summary>
    protected void DrawLegend(DrawingContext dc, params (string Text, Pen Pen)[] items)
    {
        double lx = Plot.Right;
        foreach (var (text, pen) in items)
        {
            var t = Label(text, new SolidColorBrush(Color.FromArgb(0xa6, 0xff, 0xff, 0xff)), 10);
            lx -= t.Width;
            double cy = 6 + t.Height / 2;
            dc.DrawText(t, new Point(lx, 6));
            lx -= 22;
            dc.DrawLine(pen, new Point(lx, cy), new Point(lx + 17, cy));
            lx -= 18;
        }
    }

    /// <summary>Polyline over the samples; NaN breaks the trace (e.g. missing sensor).</summary>
    protected void DrawTrace(DrawingContext dc, int count, Func<int, double> yFor, Pen pen)
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

    /// <summary>Under-fill from the trace down to the baseline (the softer band treatment).</summary>
    protected void DrawUnderFill(DrawingContext dc, int count, Func<int, double> yFor, Brush brush)
    {
        var r = Plot;
        var geo = new StreamGeometry();
        using (var ctx = geo.Open())
        {
            ctx.BeginFigure(new Point(XForIndex(0, count), r.Bottom), true, true);
            for (int i = 0; i < count; i++)
            {
                double y = yFor(i);
                ctx.LineTo(new Point(XForIndex(i, count), double.IsNaN(y) ? r.Bottom : y), true, false);
            }
            ctx.LineTo(new Point(r.Right, r.Bottom), false, false);
        }
        geo.Freeze();
        dc.DrawGeometry(brush, null, geo);
    }

    /// <summary>Text seated on the card colour, so a reference label stays readable
    /// where a trace or a dashed line runs underneath it.</summary>
    protected void DrawSeatedText(DrawingContext dc, FormattedText t, Point at)
    {
        dc.DrawRectangle(new SolidColorBrush(CardBg), null,
            new Rect(at.X - 2, at.Y, t.Width + 4, t.Height));
        dc.DrawText(t, at);
    }

    /// <summary>Sample index under the cursor, or null when the pointer is off the trace.</summary>
    protected int? HoverIndex(int count)
    {
        if (count <= 0 || _hoverX is not double hx) return null;
        var r = Plot;
        if (hx < XForIndex(0, count) - 4 || hx > r.Right + 4) return null;
        int idx = count - 1 - (int)Math.Round((r.Right - Math.Clamp(hx, r.Left, r.Right)) / PixelsPerSample);
        return Math.Clamp(idx, 0, count - 1);
    }

    protected void DrawCrosshair(DrawingContext dc, double x) =>
        dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x30, 0xff, 0xff, 0xff)), 1),
            new Point(x, Plot.Top), new Point(x, Plot.Bottom));

    /// <summary>White dot seated on the card colour, so it reads on top of any trace.</summary>
    protected void DrawMarker(DrawingContext dc, Point p)
    {
        dc.DrawEllipse(new SolidColorBrush(CardBg), null, p, 4.4, 4.4);
        dc.DrawEllipse(Brushes.White, null, p, 3, 3);
    }

    /// <summary>
    /// Hover "ticket" pinned to the top of the plot, clamped inside it. Pass wider and
    /// narrower wordings of the same readout (a two-line variant works — FormattedText
    /// honours "\n"): the first one that fits the plot wins, so a quarter-screen window
    /// gets the short form instead of a chip running off the edge.
    /// </summary>
    protected void DrawChip(DrawingContext dc, double x, params string[] options)
    {
        var r = Plot;
        var brush = new SolidColorBrush(Color.FromArgb(0xf2, 0xff, 0xff, 0xff));
        var chip = Label(options[^1], brush);
        foreach (var option in options)
        {
            var candidate = Label(option, brush);
            if (candidate.Width + 12 <= r.Width) { chip = candidate; break; }
        }
        var pos = new Point(Math.Clamp(x - chip.Width / 2, r.Left, Math.Max(r.Left, r.Right - chip.Width)),
            r.Top + 3);
        dc.DrawRoundedRectangle(new SolidColorBrush(CardBg),
            new Pen(new SolidColorBrush(Color.FromArgb(0x26, 0xff, 0xff, 0xff)), 1),
            new Rect(pos.X - 6, pos.Y - 3, chip.Width + 12, chip.Height + 6), 6, 6);
        dc.DrawText(chip, pos);
    }

    /// <summary>"now" / "−1:24" for a sample's distance from the right edge.</summary>
    protected static string Ago(int idx, int count)
    {
        int ago = (int)Math.Round((count - 1 - idx) * SecondsPerSample);
        return ago == 0 ? "now" : Inv($"−{ago / 60}:{ago % 60:00}");
    }

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
