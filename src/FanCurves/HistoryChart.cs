using System.Buffers.Binary;
using System.IO;
using System.Windows;
using System.Windows.Media;

namespace FanCurves;

/// <summary>
/// One engine tick's worth of a channel's state, stamped with the wall-clock time of the
/// measurement. The temperature/output fields feed the history strip; the rest is
/// thermal-budget telemetry for the budget strip (defaults apply on channels the budget
/// controller does not drive).
/// </summary>
public readonly record struct HistorySample(
    DateTime Time,                  // wall clock at the tick — chips and the axis print it
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

/// <summary>
/// Two-tier per-channel history: a fixed ring of the last <see cref="Capacity"/> samples
/// (one strip window, ~10 min at the engine's jittered ~1 s tick) in memory, plus a
/// binary spill file in the temp directory holding ~24 h so the strips can scroll back
/// without keeping the day in RAM. The file is opened with DeleteOnClose, so it
/// disappears with the process even on a crash; if it cannot be opened (or a write
/// fails) the history quietly degrades to the RAM-only ring.
/// </summary>
public class ChannelHistory : IDisposable
{
    public const int Capacity = 600;

    // Records are quantized integers, not doubles — the signals carry nowhere near
    // double precision and the strips display even less: temps/watts/percent in
    // tenths (int16), headroom in whole seconds (uint16, 0xFFFF = ∞), joules whole
    // (int32), time in whole seconds since Epoch (uint32; the ring keeps the exact
    // DateTime for the live window). 26 B/record ≈ 2.2 MB per channel per day.
    private const int RecordSize = 26;
    private const long DiskRetention = 86_400;      // ~24 h at the ~1 s tick
    private const long CompactAt = DiskRetention * 2; // rewrite cost is ~2 MB, ~once a day
    private static readonly DateTime Epoch = new(2020, 1, 1);
    private const short NullI16 = short.MinValue;   // "no value" for the nullable tenths fields
    private const ushort InfTau = ushort.MaxValue;  // headroom ∞ (or anything ≥ ~18 h)

    private readonly HistorySample[] _buf = new HistorySample[Capacity];
    private int _next, _count;

    private FileStream? _file;      // null = RAM-only fallback
    private long _total;            // samples added this session — the absolute index space
    private long _fileStart;        // absolute index of the spill file's first record
    private readonly byte[] _rec = new byte[RecordSize];

    public int Count => _count;

    /// <summary>0 = oldest RING sample, Count-1 = newest (the live strip window).</summary>
    public HistorySample this[int i] => _buf[(_next - _count + i + Capacity) % Capacity];

    /// <summary>Samples added this session; absolute indices run [FirstAvailable, TotalCount).</summary>
    public long TotalCount => _total;

    /// <summary>Oldest absolute index still scrollable to (spill file, or the ring tail).</summary>
    public long FirstAvailable => _file != null ? _fileStart : _total - _count;

    public ChannelHistory()
    {
        try
        {
            string dir = Path.Combine(Path.GetTempPath(), "FanCurves");
            Directory.CreateDirectory(dir);
            _file = new FileStream(
                Path.Combine(dir, $"history-{Environment.ProcessId}-{Guid.NewGuid():N}.bin"),
                FileMode.CreateNew, FileAccess.ReadWrite, FileShare.None, 1 << 15,
                FileOptions.DeleteOnClose);
        }
        catch
        {
            _file = null;
        }
    }

    public void Add(HistorySample s)
    {
        _buf[_next] = s;
        _next = (_next + 1) % Capacity;
        if (_count < Capacity) _count++;
        _total++;
        if (_file == null) return;
        try
        {
            Encode(s, _rec);
            _file.Seek(0, SeekOrigin.End);
            _file.Write(_rec, 0, RecordSize);
            if (_total - _fileStart > CompactAt) Compact();
        }
        catch
        {
            DropFile();
        }
    }

    /// <summary>
    /// Read <paramref name="count"/> samples starting at absolute index
    /// <paramref name="startAbs"/> into <paramref name="into"/>; returns how many were
    /// actually available. Callers clamp to [FirstAvailable, TotalCount) themselves.
    /// </summary>
    public int Read(long startAbs, int count, HistorySample[] into)
    {
        long start = Math.Max(startAbs, FirstAvailable);
        long end = Math.Min(startAbs + count, _total);
        int n = (int)Math.Clamp(end - start, 0, into.Length);
        if (n == 0) return 0;
        if (_file == null)
        {
            int ringBase = (int)(start - (_total - _count));
            for (int i = 0; i < n; i++) into[i] = this[ringBase + i];
            return n;
        }
        try
        {
            _file.Seek((start - _fileStart) * RecordSize, SeekOrigin.Begin);
            var bytes = new byte[n * RecordSize];
            int read = 0;
            while (read < bytes.Length)
            {
                int got = _file.Read(bytes, read, bytes.Length - read);
                if (got <= 0) break;
                read += got;
            }
            int have = read / RecordSize;
            for (int i = 0; i < have; i++)
                into[i] = Decode(bytes.AsSpan(i * RecordSize, RecordSize));
            return have;
        }
        catch
        {
            DropFile();
            return 0;
        }
    }

    /// <summary>Forget everything; the strips grow in from the right again. Absolute
    /// indices stay monotonic so a viewport anchored in the past just clamps forward.</summary>
    public void Clear()
    {
        _next = 0;
        _count = 0;
        if (_file == null) return;
        try
        {
            _file.SetLength(0);
            _fileStart = _total;
        }
        catch
        {
            DropFile();
        }
    }

    public void Dispose() => DropFile();

    private void DropFile()
    {
        try { _file?.Dispose(); } catch { /* the ring keeps working */ }
        _file = null;
    }

    /// <summary>Trim the spill file back to the newest <see cref="DiskRetention"/> records.</summary>
    private void Compact()
    {
        long drop = (_total - _fileStart) - DiskRetention;
        var tail = new byte[DiskRetention * RecordSize];
        _file!.Seek(drop * RecordSize, SeekOrigin.Begin);
        int read = 0;
        while (read < tail.Length)
        {
            int got = _file.Read(tail, read, tail.Length - read);
            if (got <= 0) break;
            read += got;
        }
        _file.Seek(0, SeekOrigin.Begin);
        _file.Write(tail, 0, read);
        _file.SetLength(read);
        _fileStart += drop;
    }

    // Layout: 0 time u32 · 4 raw i16 · 6 avg i16 · 8 out% i16 · 10 W i16 · 12 Wavg i16
    //         · 14 joules i32 · 18 tau u16 · 20 demand u8 · 21 ceiling i16 · 23 aim i16
    //         · 25 flags u8 — tenths fields are value ×10.
    private static void Encode(in HistorySample s, byte[] b)
    {
        double secs = (s.Time - Epoch).TotalSeconds;
        BinaryPrimitives.WriteUInt32LittleEndian(b, (uint)Math.Clamp(secs, 0, uint.MaxValue));
        WriteTenths(b, 4, s.RawTemp);
        WriteTenths(b, 6, double.IsNaN(s.EffectiveTemp) ? null : s.EffectiveTemp);
        WriteTenths(b, 8, s.OutputPercent);
        WriteTenths(b, 10, s.Watts);
        WriteTenths(b, 12, s.WattsAvg);
        BinaryPrimitives.WriteInt32LittleEndian(b.AsSpan(14),
            (int)Math.Clamp(s.BudgetJoules, int.MinValue, int.MaxValue));
        double tau = s.TauSeconds;
        BinaryPrimitives.WriteUInt16LittleEndian(b.AsSpan(18),
            double.IsNaN(tau) ? (ushort)0
            : tau >= InfTau ? InfTau
            : (ushort)Math.Clamp(Math.Round(tau), 0, InfTau - 1));
        b[20] = (byte)Math.Clamp(Math.Round(s.DemandLevel), 0, 255);
        WriteTenths(b, 21, s.CeilingC);
        WriteTenths(b, 23, s.AimC);
        b[25] = s.Override ? (byte)1 : (byte)0;
    }

    private static HistorySample Decode(ReadOnlySpan<byte> b)
    {
        ushort tau = BinaryPrimitives.ReadUInt16LittleEndian(b.Slice(18));
        return new HistorySample(
            Epoch.AddSeconds(BinaryPrimitives.ReadUInt32LittleEndian(b)),
            ReadTenths(b, 4),
            ReadTenths(b, 6) ?? double.NaN,
            ReadTenths(b, 8) ?? 0,
            ReadTenths(b, 10),
            ReadTenths(b, 12),
            BinaryPrimitives.ReadInt32LittleEndian(b.Slice(14)),
            tau == InfTau ? double.PositiveInfinity : tau,
            b[20],
            ReadTenths(b, 21) ?? 0,
            ReadTenths(b, 23) ?? 0,
            b[25] != 0);
    }

    private static void WriteTenths(byte[] b, int off, double? v) =>
        BinaryPrimitives.WriteInt16LittleEndian(b.AsSpan(off),
            v is double d && !double.IsNaN(d)
                ? (short)Math.Clamp(Math.Round(d * 10), NullI16 + 1, short.MaxValue)
                : NullI16);

    private static double? ReadTenths(ReadOnlySpan<byte> b, int off)
    {
        short v = BinaryPrimitives.ReadInt16LittleEndian(b.Slice(off));
        return v == NullI16 ? null : v / 10.0;
    }
}

/// <summary>
/// Rolling strip under the curve editor (developer mode only): a 10-minute window of the
/// selected channel's rolling-average temperature, raw temperature and commanded fan %.
/// Live, the trace grows in from the right (right edge = now); the shared viewport lets
/// the strip scroll back through the day's spill file. Hovering shows a crosshair with a
/// clock · ago · avg · now · % readout chip; amber is live-data-only, so it disappears
/// while scrolled into the past.
/// </summary>
public class HistoryChart : StripChart
{
    private const double TempMin = 15, TempMax = 100;   // same span as CurveEditor

    // Frozen paints (see Paint); the shared vocabulary pens live in StripChart.
    private static readonly Pen LegendFanPen = Paint.WhitePen(0x8c, 1.5);
    private static readonly Pen AimPen = Paint.WhitePen(0x20, 1, Paint.Dots);
    private static readonly Pen FanPen = Paint.WhitePen(0x66, 1.5);
    private static readonly Pen OnMarkPen = Paint.WhitePen(0x8c, 1);
    private static readonly Pen OffMarkPen = Paint.WhitePen(0x46, 1);

    private double YForPct(double p) => Plot.Bottom - Math.Clamp(p, 0, 100) / 100.0 * Plot.Height;
    private double YForTemp(double t) =>
        Plot.Bottom - (Math.Clamp(t, TempMin, TempMax) - TempMin) / (TempMax - TempMin) * Plot.Height;

    protected override void OnRender(DrawingContext dc)
    {
        if (TooSmallToRender) return;
        // Transparent fill keeps hit-testing (hover) alive across the whole strip.
        dc.DrawRectangle(Brushes.Transparent, null, new Rect(0, 0, ActualWidth, ActualHeight));
        var r = Plot;
        TakeSnapshot();

        DrawFrame(dc, "HISTORY");
        DrawLegend(dc,
            ("fan %", LegendFanPen),
            ("now temp", LegendDashPen),
            ("avg temp", BrightPen));

        // Sparse horizontal grid on the % scale, labels on the left axis like the curve chart.
        for (double pct = 0; pct <= 100; pct += 50)
        {
            var y = YForPct(pct);
            dc.DrawLine(pct == 0 ? AxisPen : GridPen, new Point(r.Left, y), new Point(r.Right, y));
            var t = Label(pct == 100 ? "100%" : $"{pct:0}");
            dc.DrawText(t, new Point(r.Left - t.Width - 10, y - t.Height / 2));
        }

        // Quiet temperature references inside the right edge (second scale, no gridlines).
        foreach (double temp in new[] { 40.0, 60.0, 80.0 })
        {
            var t = Label($"{temp:0}°", RefLabelBrush, 9.5);
            dc.DrawText(t, new Point(r.Right - t.Width - 4, YForTemp(temp) - t.Height / 2));
        }

        var h = Win;
        int count = WinCount;
        if (count == 0) return;

        // Budget ceiling (power control only): the temperature the thermal credit is
        // measured against — the trace approaching it is what makes the fans step up.
        // (its label goes on last, over the traces)
        double ceiling = h[count - 1].CeilingC;
        if (ceiling > 0)
            dc.DrawLine(DottedRefPen,
                new Point(r.Left, YForTemp(ceiling)), new Point(r.Right, YForTemp(ceiling)));

        // Sustained aim (power control only): the line the controller actually defends —
        // headroom counts down to it and the fans engage before the trend settles past it.
        // Dimmer than the ceiling; skipped when the margins make the two coincide.
        double aim = h[count - 1].AimC;
        if (aim > 0 && aim < ceiling - 0.5)
            dc.DrawLine(AimPen,
                new Point(r.Left, YForTemp(aim)), new Point(r.Right, YForTemp(aim)));

        // Fan % trace: soft under-fill + a quiet line (same treatment as the curve's band fill).
        DrawUnderFill(dc, count, i => YForPct(h[i].OutputPercent), TraceFill);
        DrawTrace(dc, count, i => YForPct(h[i].OutputPercent), FanPen);

        // Raw ("now") temp: faint dashed reference under the average.
        DrawTrace(dc, count, i => h[i].RawTemp is double raw ? YForTemp(raw) : double.NaN,
            FaintDashPen);

        // Rolling-average temp: the bright trace (it is what drives the steps).
        DrawTrace(dc, count, i => double.IsNaN(h[i].EffectiveTemp) ? double.NaN : YForTemp(h[i].EffectiveTemp),
            BrightPen);

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
            foreach (int i in onMarks)
            {
                double x = XForIndex(i, count);
                dc.DrawLine(OnMarkPen, new Point(x, r.Bottom), new Point(x, r.Bottom - 6));
            }
            foreach (int i in offMarks)
            {
                double x = XForIndex(i, count);
                dc.DrawLine(OffMarkPen, new Point(x, r.Bottom), new Point(x, r.Bottom - 6));
            }
            // Stopped-time spans: one per completed stop (off → on), plus a live one that
            // counts up while the newest LIVE sample is still stopped. Labels seat on the
            // card background so they stay readable even when the gap is only a few pixels;
            // a label is only skipped when it would overlap the previous span's label.
            var spans = new List<(int Off, int End, bool Live)>();
            for (int k = 0; k < onMarks.Count; k++)
                if (offBefore[k] >= 0)   // stop that began before the visible window has no measurable start
                    spans.Add((offBefore[k], onMarks[k], false));
            if (IsLive && h[count - 1].OutputPercent <= 0 && lastOff >= 0 &&
                (onMarks.Count == 0 || lastOff > onMarks[^1]))
                spans.Add((lastOff, count - 1, true));

            double spanY = r.Bottom - 14;
            double lastLabelRight = double.NegativeInfinity;
            foreach (var (off, end, live) in spans)
            {
                double x1 = XForIndex(off, count), x2 = XForIndex(end, count);
                if (x2 - x1 >= 8)
                {
                    dc.DrawLine(SpanPen, new Point(x1 + 3, spanY), new Point(x2 - 3, spanY));
                    dc.DrawLine(SpanPen, new Point(x1 + 3, spanY - 3), new Point(x1 + 3, spanY + 3));
                    if (!live)   // an ongoing stop has no end tick — the right edge is "now"
                        dc.DrawLine(SpanPen, new Point(x2 - 3, spanY - 3), new Point(x2 - 3, spanY + 3));
                }
                int sec = (int)Math.Round((h[end].Time - h[off].Time).TotalSeconds);
                var t = Label(Inv($"{sec / 60}:{sec % 60:00}"), LegendBrush, 10);
                double tx = Math.Clamp((x1 + x2) / 2 - t.Width / 2, r.Left, Math.Max(r.Left, r.Right - t.Width));
                if (tx - 3 <= lastLabelRight + 4) continue;   // would collide with the previous label
                var tp = new Point(tx, spanY - t.Height / 2);
                dc.DrawRectangle(CardBrush, null, new Rect(tp.X - 3, tp.Y, t.Width + 6, t.Height));
                dc.DrawText(t, tp);
                lastLabelRight = tp.X + t.Width + 3;
            }
        }

        if (ceiling > 0)
        {
            var t = Label(Inv($"ceiling {ceiling:0}°"), RefLabelBrush, 9.5);
            DrawSeatedText(dc, t, new Point(r.Left + 4, YForTemp(ceiling) - t.Height - 1));
        }
        if (aim > 0 && aim < ceiling - 0.5)
        {
            // Below its line (the ceiling label sits above its own), so the two never
            // collide even when the margins put the lines a few pixels apart.
            var t = Label(Inv($"aim {aim:0}°"), RefLabelBrush, 9.5);
            DrawSeatedText(dc, t, new Point(r.Left + 4, YForTemp(aim) + 2));
        }

        // The newest sample is live thermal state — the one place amber is allowed here.
        // Scrolled into the past nothing is live, so the dots disappear with the scroll.
        if (IsLive)
        {
            var newest = h[count - 1];
            double nx = XForIndex(count - 1, count);
            if (!double.IsNaN(newest.EffectiveTemp))
                dc.DrawEllipse(AmberBrush, null, new Point(nx, YForTemp(newest.EffectiveTemp)), 2.6, 2.6);
            dc.DrawEllipse(AmberBrush, null, new Point(nx, YForPct(newest.OutputPercent)), 2.2, 2.2);
        }

        // Hover "ticket": crosshair at the nearest sample with a clock · ago · temp · % chip.
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
                Inv($"{ClockText(idx)} · {AgoText(idx)} · avg {avgStr} · now {rawStr} · {s.OutputPercent:0}%"),
                Inv($"{ClockText(idx)} · {AgoText(idx)}\navg {avgStr} · now {rawStr} · {s.OutputPercent:0}%"));
        }
    }
}
