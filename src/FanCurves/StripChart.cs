using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace FanCurves;

/// <summary>
/// Scroll state of the history strip, panning through the day as one timeline.
/// Live (no anchor) the window follows the newest samples straight from the in-memory
/// ring; scrolled back, the window is anchored to absolute sample indices and served
/// from the channel's spill file through a one-window cache (so hover redraws don't
/// re-read the disk). Owned by MainWindow.
/// </summary>
public class HistoryViewport
{
    public const int WindowSamples = ChannelHistory.Capacity;

    private ChannelHistory? _history;
    private long? _anchorEnd;   // absolute EXCLUSIVE end of the frozen window; null = follow live
    private readonly HistorySample[] _cache = new HistorySample[WindowSamples];
    private long _cacheStart = -1;
    private int _cacheCount;
    private long _cacheLiveTotal = -1;

    /// <summary>Raised on any scroll/anchor/data change the strips should redraw for.</summary>
    public event Action? Changed;

    public ChannelHistory? History
    {
        get => _history;
        set
        {
            _history = value;
            _anchorEnd = null;
            Invalidate();
        }
    }

    public bool Live => _anchorEnd == null;

    public bool CanScroll =>
        _history != null && _history.TotalCount - _history.FirstAvailable > WindowSamples;

    /// <summary>Pan the window; positive = further into the past. Reaching the newest
    /// sample snaps back to following live.</summary>
    public void ScrollBy(long samplesBack)
    {
        if (_history == null || _history.TotalCount == 0) return;
        long total = _history.TotalCount;
        long minEnd = Math.Min(total, _history.FirstAvailable + WindowSamples);
        long end = Math.Clamp((_anchorEnd ?? total) - samplesBack, minEnd, total);
        long? anchor = end >= total ? null : end;
        if (anchor == _anchorEnd) return;
        _anchorEnd = anchor;
        Invalidate();
    }

    public void JumpToLive()
    {
        if (_anchorEnd == null) return;
        _anchorEnd = null;
        Invalidate();
    }

    /// <summary>Drop the cached window (data cleared/replaced underneath) and redraw.</summary>
    public void Invalidate()
    {
        _cacheStart = -1;
        _cacheLiveTotal = -1;
        Changed?.Invoke();
    }

    /// <summary>
    /// The visible window: a reused buffer with <c>Count</c> valid samples, the absolute
    /// index of the first one, and whether the right edge is "now".
    /// </summary>
    public (HistorySample[] Buf, int Count, long StartAbs, bool Live) Snapshot()
    {
        var h = _history;
        if (h == null || h.TotalCount == 0) return (_cache, 0, 0, true);
        long total = h.TotalCount;
        if (_anchorEnd is not long end)
        {
            if (_cacheLiveTotal != total)
            {
                _cacheCount = h.Count;
                for (int i = 0; i < _cacheCount; i++) _cache[i] = h[i];
                _cacheStart = total - _cacheCount;
                _cacheLiveTotal = total;
            }
            return (_cache, _cacheCount, _cacheStart, true);
        }
        // Compaction may have trimmed past the anchor — clamp forward, keep a full window.
        end = Math.Clamp(end, Math.Min(total, h.FirstAvailable + WindowSamples), total);
        long start = Math.Max(h.FirstAvailable, end - WindowSamples);
        if (_cacheStart != start || _cacheCount != (int)(end - start) || _cacheLiveTotal != -1)
        {
            _cacheCount = h.Read(start, (int)(end - start), _cache);
            _cacheStart = start;
            _cacheLiveTotal = -1;
        }
        return (_cache, _cacheCount, _cacheStart, false);
    }
}

/// <summary>
/// Skeleton of the rolling history strip under the curve chart: identical padding to
/// <see cref="CurveEditor"/> so the plots line up vertically, the letter-spaced strip
/// title, the legend, the time axis (wall-clock ticks; the right edge reads "now"
/// while live, the window-end time while scrolled) and the hover crosshair. Scrolling
/// — mouse wheel (Shift = 10×), drag, and double-click back to live — goes through
/// the <see cref="HistoryViewport"/>. The subclass adds its scales, traces and chip.
/// </summary>
public abstract class StripChart : FrameworkElement
{
    // Left/right match CurveEditor's padding so the plots align vertically.
    protected static readonly Thickness Pad = new(42, 22, 18, 20);

    protected static readonly Color Amber = Color.FromRgb(0xFF, 0x9E, 0x5E);
    protected static readonly Color CardBg = Color.FromRgb(0x11, 0x11, 0x16);

    private static readonly Typeface Mono = new(new FontFamily("Cascadia Mono, Consolas"),
        FontStyles.Normal, FontWeights.Normal, FontStretches.Normal);
    private static readonly Typeface TitleTf = new(
        new FontFamily("Segoe UI Variable Text, Segoe UI"),
        FontStyles.Normal, FontWeights.SemiBold, FontStretches.Normal);

    // Frozen paints (see Paint) — the strips redraw every tick and on every hover
    // move; nothing here may be allocated per render. Shared entries carry the
    // whole family's visual vocabulary: dashed = instantaneous, bright = the
    // driving average, dotted = a configured reference line.
    private static readonly Pen SeamPen = Paint.WhitePen(0x12, 1);
    private static readonly Pen CrosshairPen = Paint.WhitePen(0x30, 1);
    private static readonly Brush ChipBrush = Paint.White(0xf2);
    private static readonly Pen ChipEdgePen = Paint.WhitePen(0x26, 1);
    protected static readonly Pen GridPen = Paint.WhitePen(0x0d, 1);
    protected static readonly Pen AxisPen = Paint.WhitePen(0x1a, 1);
    protected static readonly Pen SpanPen = Paint.WhitePen(0x30, 1);
    protected static readonly Brush LegendBrush = Paint.White(0xa6);
    protected static readonly Brush RefLabelBrush = Paint.White(0x33);
    protected static readonly Brush CardBrush = Paint.Brush(CardBg);
    protected static readonly Brush AmberBrush = Paint.Brush(Amber);
    protected static readonly Brush TraceFill = Paint.VerticalFade(0x12, 0x02);
    protected static readonly Pen LegendDashPen = Paint.WhitePen(0x66, 1, Paint.Dash);
    protected static readonly Pen FaintDashPen = Paint.WhitePen(0x40, 1, Paint.Dash);
    protected static readonly Pen BrightPen = Paint.WhitePen(0xf0, 2, rounded: true);

    private HistoryViewport? _viewport;

    public HistoryViewport? Viewport
    {
        get => _viewport;
        set
        {
            if (_viewport != null) _viewport.Changed -= Refresh;
            _viewport = value;
            if (_viewport != null) _viewport.Changed += Refresh;
            InvalidateVisual();
        }
    }

    // Window snapshot for the current render pass — set by TakeSnapshot() at the top of
    // OnRender, read by DrawFrame/AgoText/ClockText and the subclasses' trace loops.
    protected HistorySample[] Win = Array.Empty<HistorySample>();
    protected int WinCount;
    protected bool IsLive = true;

    private double? _hoverX;
    private bool _dragging;
    private Point _dragLast;
    private double _dragRemainder;

    public void Refresh() => InvalidateVisual();

    protected void TakeSnapshot()
    {
        if (Viewport?.Snapshot() is (var buf, var count, _, var live))
        {
            Win = buf;
            WinCount = count;
            IsLive = live;
        }
        else
        {
            Win = Array.Empty<HistorySample>();
            WinCount = 0;
            IsLive = true;
        }
    }

    protected Rect Plot => new(
        Pad.Left, Pad.Top,
        Math.Max(10, ActualWidth - Pad.Left - Pad.Right),
        Math.Max(10, ActualHeight - Pad.Top - Pad.Bottom));

    /// <summary>Mid-resize arrange passes can hand the strip a near-zero size; label
    /// layout in a degenerate plot produces inverted clamp ranges (a min > max
    /// Math.Clamp took the app down, 2026-07-27) — subclasses skip rendering instead.</summary>
    protected bool TooSmallToRender =>
        ActualWidth - Pad.Left - Pad.Right < 60 || ActualHeight - Pad.Top - Pad.Bottom < 24;

    protected double PixelsPerSample => Plot.Width / (ChannelHistory.Capacity - 1);

    protected double XForIndex(int i, int count) => Plot.Right - (count - 1 - i) * PixelsPerSample;

    protected static string Inv(FormattableString f) => FormattableString.Invariant(f);

    protected static readonly Brush LabelBrush = Paint.White(0x59);

    protected FormattedText Label(string s, Brush? brush = null, double size = 10.5) => new(
        s, CultureInfo.InvariantCulture, FlowDirection.LeftToRight, Mono, size,
        brush ?? LabelBrush, 1.25);

    /// <summary>Hairline seam against the plot above, the strip title, and the time axis:
    /// wall-clock labels at 5-minute boundaries, with "now" (live) or the window-end
    /// time (scrolled) pinned to the right edge.</summary>
    protected void DrawFrame(DrawingContext dc, string title)
    {
        var r = Plot;
        dc.DrawLine(SeamPen, new Point(r.Left, 0.5), new Point(r.Right, 0.5));
        var t = new FormattedText(Tracked.Space(title), CultureInfo.InvariantCulture,
            FlowDirection.LeftToRight, TitleTf, 9, LabelBrush, 1.25);
        dc.DrawText(t, new Point(r.Left, 7));

        // Right edge: "now" while following live, the exact window-end time scrolled back.
        var edge = Label(IsLive || WinCount == 0 ? "now" : Win[WinCount - 1].Time.ToString("HH:mm:ss"));
        double edgeLeft = Math.Max(r.Left, r.Right - edge.Width);
        dc.DrawText(edge, new Point(edgeLeft, r.Bottom + 5));

        if (WinCount < 2) return;
        DateTime t0 = Win[0].Time, t1 = Win[WinCount - 1].Time;
        double span = Math.Max(1, (t1 - t0).TotalSeconds);
        // First whole 5-minute wall-clock boundary inside the window, then every 5 min.
        long fiveMin = TimeSpan.FromMinutes(5).Ticks;
        var tick = new DateTime((t0.Ticks + fiveMin - 1) / fiveMin * fiveMin);
        for (; tick < t1; tick = tick.AddMinutes(5))
        {
            double x = r.Right - (t1 - tick).TotalSeconds / span * (WinCount - 1) * PixelsPerSample;
            var lab = Label(tick.ToString("HH:mm"));
            double lx = x - lab.Width / 2;
            if (lx < r.Left || lx + lab.Width > edgeLeft - 8) continue;  // keep clear of the edge label
            dc.DrawText(lab, new Point(lx, r.Bottom + 5));
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
            var t = Label(text, LegendBrush, 10);
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
        dc.DrawLine(CrosshairPen, new Point(x, Plot.Top), new Point(x, Plot.Bottom));

    /// <summary>White dot seated on the card colour, so it reads on top of any trace.</summary>
    protected void DrawMarker(DrawingContext dc, Point p)
    {
        dc.DrawEllipse(CardBrush, null, p, 4.4, 4.4);
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
        var chip = Label(options[^1], ChipBrush);
        foreach (var option in options)
        {
            var candidate = Label(option, ChipBrush);
            if (candidate.Width + 12 <= r.Width) { chip = candidate; break; }
        }
        var pos = new Point(Math.Clamp(x - chip.Width / 2, r.Left, Math.Max(r.Left, r.Right - chip.Width)),
            r.Top + 3);
        dc.DrawRoundedRectangle(CardBrush, ChipEdgePen,
            new Rect(pos.X - 6, pos.Y - 3, chip.Width + 12, chip.Height + 6), 6, 6);
        dc.DrawText(chip, pos);
    }

    /// <summary>"14:32:05" — the wall-clock time the sample was measured.</summary>
    protected string ClockText(int idx) => Win[idx].Time.ToString("HH:mm:ss");

    /// <summary>"now" / "−1:24" / "−1:03:52" — the sample's true distance from now.</summary>
    protected string AgoText(int idx)
    {
        int a = (int)Math.Round(Math.Max(0, (DateTime.Now - Win[idx].Time).TotalSeconds));
        return a <= 1 && IsLive && idx == WinCount - 1 ? "now"
            : a < 3600 ? Inv($"−{a / 60}:{a % 60:00}")
            : Inv($"−{a / 3600}:{a / 60 % 60:00}:{a % 60:00}");
    }

    protected override void OnMouseMove(MouseEventArgs e)
    {
        var p = e.GetPosition(this);
        if (_dragging)
        {
            // Content follows the cursor: dragging right pulls older samples into view.
            double s = (p.X - _dragLast.X) / PixelsPerSample + _dragRemainder;
            int whole = (int)s;
            _dragRemainder = s - whole;
            _dragLast = p;
            if (whole != 0) Viewport?.ScrollBy(whole);
            return;
        }
        _hoverX = p.X;
        InvalidateVisual();
    }

    protected override void OnMouseLeave(MouseEventArgs e)
    {
        _hoverX = null;
        InvalidateVisual();
    }

    protected override void OnMouseWheel(MouseWheelEventArgs e)
    {
        // Wheel up pans into the past, ~1 min per notch (10 min with Shift).
        long step = Keyboard.Modifiers.HasFlag(ModifierKeys.Shift) ? 600 : 60;
        Viewport?.ScrollBy(e.Delta > 0 ? step : -step);
        e.Handled = true;
    }

    protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
    {
        if (e.ClickCount == 2) { Viewport?.JumpToLive(); return; }
        if (Viewport is not { CanScroll: true }) return;
        _dragging = true;
        _dragLast = e.GetPosition(this);
        _dragRemainder = 0;
        _hoverX = null;
        CaptureMouse();
    }

    protected override void OnMouseLeftButtonUp(MouseButtonEventArgs e)
    {
        if (!_dragging) return;
        _dragging = false;
        ReleaseMouseCapture();
    }

    protected override void OnLostMouseCapture(MouseEventArgs e) => _dragging = false;
}
