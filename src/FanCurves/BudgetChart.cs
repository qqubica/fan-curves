using System.Windows;
using System.Windows.Media;

namespace FanCurves;

/// <summary>
/// Second rolling strip (developer mode, under the history strip): the last 10 minutes of
/// the thermal-budget controller's own signals — the channel's power draw and its sustained
/// average (left scale, watts) against the predicted headroom before the guarded line (the
/// sustained aim, or the ceiling once the trend is past the aim; right scale, seconds) and
/// the ramp-lead threshold that triggers a step up. Spans where
/// the hard-override fuse was latched are shaded. Same geometry as the history strip, so
/// the two read as one timeline; hovering shows draw · avg · buffer · headroom · needs.
/// </summary>
public class BudgetChart : StripChart
{
    /// <summary>Ramp lead in force (Profile.RampLeadSeconds) — the trigger line.</summary>
    public double LeadSeconds { get; set; } = 45;

    /// <summary>Why the window holds no power samples — "no power sensor on this
    /// channel" or "temperature mode"; the owner sets it to match the control mode.</summary>
    public string NoPowerNote { get; set; } = "no power sensor on this channel — temperature control";

    private double _wattsMax = 100;

    // Headroom is drawn on a LOG scale from 10 s to 30 min: time-to-trouble matters by
    // its order of magnitude, and a linear axis capped near the trigger collapsed
    // "3 hours", "20 minutes and falling" and "∞" into the same flat line at the top —
    // a slow warm-up read as "headroom not moving" (Kuba's report, 2026-07-26). The top
    // still means "≥30 min / ∞" (the chip prints ∞ there); the trigger line sits in the
    // lower third, so a genuine dive toward it is a long visible descent.
    private const double SecondsLo = 10, SecondsHi = 1800;

    private double YForWatts(double w) => Plot.Bottom - Math.Clamp(w, 0, _wattsMax) / _wattsMax * Plot.Height;
    private double YForSeconds(double s)
    {
        double v = Math.Clamp(double.IsNaN(s) ? 0 : s, SecondsLo, SecondsHi);
        double f = Math.Log(v / SecondsLo) / Math.Log(SecondsHi / SecondsLo);
        return Plot.Bottom - f * Plot.Height;
    }

    /// <summary>Round a peak up to a readable axis top (the ladder keeps the label short).
    /// Shared with the curve editor's power reference lines so both read the same scale.</summary>
    internal static double NiceWatts(double peak)
    {
        foreach (double step in new[] { 25.0, 50, 75, 100, 150, 200, 250, 300, 400, 500, 600, 800, 1000 })
            if (peak <= step) return step;
        return Math.Ceiling(peak / 250) * 250;
    }

    /// <summary>"45 s" / "2:30" / "∞" — minutes truncate, so 90 s is 1:30, not 2:30.</summary>
    private static string Headroom(double s) =>
        double.IsInfinity(s) || s > 1800 ? "∞"
        : s < 60 ? Inv($"{Math.Max(0, s):0} s")
        : Inv($"{(int)s / 60}:{(int)s % 60:00}");

    protected override void OnRender(DrawingContext dc)
    {
        if (TooSmallToRender) return;
        // Transparent fill keeps hit-testing (hover) alive across the whole strip.
        dc.DrawRectangle(Brushes.Transparent, null, new Rect(0, 0, ActualWidth, ActualHeight));
        var r = Plot;

        var gridPen = new Pen(new SolidColorBrush(Color.FromArgb(0x0d, 0xff, 0xff, 0xff)), 1);
        var axisPen = new Pen(new SolidColorBrush(Color.FromArgb(0x1a, 0xff, 0xff, 0xff)), 1);
        var refBrush = new SolidColorBrush(Color.FromArgb(0x33, 0xff, 0xff, 0xff));

        DrawFrame(dc, "THERMAL BUDGET");
        DrawLegend(dc,
            ("headroom", new Pen(new SolidColorBrush(Color.FromArgb(0x59, 0xff, 0xff, 0xff)), 1.5)),
            ("draw", new Pen(new SolidColorBrush(Color.FromArgb(0x66, 0xff, 0xff, 0xff)), 1)
                { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) }),
            ("avg power", new Pen(new SolidColorBrush(Color.FromArgb(0xf0, 0xff, 0xff, 0xff)), 2)
                { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round }));

        var h = History;
        int count = h?.Count ?? 0;

        // Watts scale follows the window's own peak — an idle channel would otherwise
        // draw a flat line along the bottom of a 300 W axis.
        double peak = 0;
        bool anyPower = false;
        for (int i = 0; i < count; i++)
        {
            if (h![i].Watts is double w) { peak = Math.Max(peak, w); anyPower = true; }
            if (h[i].WattsAvg is double a) peak = Math.Max(peak, a);
        }
        _wattsMax = NiceWatts(Math.Max(25, peak));

        // Nothing to scale against: say why, and skip axes that would only invite
        // reading numbers into an empty strip.
        if (count == 0 || !anyPower)
        {
            var note = Label(count == 0 ? "waiting for the first tick…" : NoPowerNote,
                new SolidColorBrush(Color.FromArgb(0x59, 0xff, 0xff, 0xff)));
            dc.DrawText(note, new Point(r.Left + (r.Width - note.Width) / 2,
                r.Top + (r.Height - note.Height) / 2));
            dc.DrawLine(axisPen, new Point(r.Left, r.Bottom), new Point(r.Right, r.Bottom));
            return;
        }

        // Watts grid: baseline, half, top — labels outside the left edge like the other strips.
        foreach (double w in new[] { 0, _wattsMax / 2, _wattsMax })
        {
            double y = YForWatts(w);
            dc.DrawLine(w == 0 ? axisPen : gridPen, new Point(r.Left, y), new Point(r.Right, y));
            var t = Label(w == _wattsMax ? Inv($"{w:0}W") : Inv($"{w:0}"));
            dc.DrawText(t, new Point(r.Left - t.Width - 10, y - t.Height / 2));
        }

        // Headroom scale references inside the right edge (second scale, no gridlines).
        // Log axis: decade-ish marks. Nothing near the top — that row belongs to the
        // legend (30 min IS the top, so it gets no mark; the chip prints ∞ up there).
        foreach (double s in new[] { 60.0, 600.0 })
        {
            var t = Label(Headroom(s), refBrush, 9.5);
            dc.DrawText(t, new Point(r.Right - t.Width - 4, YForSeconds(s) - t.Height / 2));
        }

        // The trigger: the fan steps up when the headroom trace crosses below this.
        double leadY = YForSeconds(LeadSeconds);
        dc.DrawLine(new Pen(new SolidColorBrush(Color.FromArgb(0x2e, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 1, 4 }, 0) },
            new Point(r.Left, leadY), new Point(r.Right, leadY));
        // Its label goes on last, over the traces — the spiky draw trace crosses it here.

        // Fuse spans: the die was at/over the override temp and the raw temp curve had
        // the fan, budget ignored. Shaded under everything so the traces stay readable.
        var fuseFill = new SolidColorBrush(Color.FromArgb(0x14, 0xff, 0xff, 0xff));
        bool fuseLabelled = false;
        for (int i = 0; i < count; i++)
        {
            if (!h![i].Override) continue;
            int start = i;
            while (i + 1 < count && h[i + 1].Override) i++;
            double x1 = XForIndex(start, count), x2 = XForIndex(i, count);
            dc.DrawRectangle(fuseFill, null,
                new Rect(x1, r.Top, Math.Max(1.5, x2 - x1), r.Height));
            // Hairline edges so the span reads as a marked period, not as another fill.
            var edgePen = new Pen(new SolidColorBrush(Color.FromArgb(0x33, 0xff, 0xff, 0xff)), 1);
            dc.DrawLine(edgePen, new Point(x1, r.Top), new Point(x1, r.Bottom));
            dc.DrawLine(edgePen, new Point(x2, r.Top), new Point(x2, r.Bottom));
            if (!fuseLabelled)
            {
                // Bottom of the span: the top row belongs to the legend and the hover chip.
                var t = Label("fuse", new SolidColorBrush(Color.FromArgb(0x8c, 0xff, 0xff, 0xff)), 9.5);
                dc.DrawText(t, new Point(Math.Min(x1 + 3, r.Right - t.Width), r.Bottom - t.Height - 1));
                fuseLabelled = true;
            }
        }

        // Headroom: a quiet line only. No under-fill — a healthy buffer pins the trace to
        // the top of the scale, and filling under it would flood the whole strip.
        DrawTrace(dc, count, i => YForSeconds(h![i].TauSeconds),
            new Pen(new SolidColorBrush(Color.FromArgb(0x59, 0xff, 0xff, 0xff)), 1.5)
            { LineJoin = PenLineJoin.Round });

        // Instantaneous draw: faint dashed reference under the average, like "now temp".
        DrawTrace(dc, count, i => h![i].Watts is double w ? YForWatts(w) : double.NaN,
            new Pen(new SolidColorBrush(Color.FromArgb(0x40, 0xff, 0xff, 0xff)), 1)
            { DashStyle = new DashStyle(new double[] { 3, 3 }, 0) });

        // Sustained power average: the bright trace — this is the demand the fans answer.
        // Its under-fill gives the strip the same weighted baseline the fan % has above.
        DrawUnderFill(dc, count, i => h![i].WattsAvg is double a ? YForWatts(a) : double.NaN,
            new LinearGradientBrush(
                Color.FromArgb(0x12, 0xff, 0xff, 0xff), Color.FromArgb(0x02, 0xff, 0xff, 0xff),
                new Point(0, 0), new Point(0, 1)));
        DrawTrace(dc, count, i => h![i].WattsAvg is double a ? YForWatts(a) : double.NaN,
            new Pen(new SolidColorBrush(Color.FromArgb(0xf0, 0xff, 0xff, 0xff)), 2)
            { LineJoin = PenLineJoin.Round, StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round });

        var leadLabel = Label(Inv($"ramp lead {Headroom(LeadSeconds)}"), refBrush, 9.5);
        DrawSeatedText(dc, leadLabel, new Point(r.Left + 4, leadY - leadLabel.Height - 1));

        // Live values — the one place amber is allowed here, as on the other plots.
        var newest = h![count - 1];
        double nx = XForIndex(count - 1, count);
        var amber = new SolidColorBrush(Amber);
        if (newest.WattsAvg is double na) dc.DrawEllipse(amber, null, new Point(nx, YForWatts(na)), 2.6, 2.6);
        dc.DrawEllipse(amber, null, new Point(nx, YForSeconds(newest.TauSeconds)), 2.2, 2.2);

        if (HoverIndex(count) is int idx)
        {
            var s = h[idx];
            double x = XForIndex(idx, count);
            DrawCrosshair(dc, x);
            if (s.WattsAvg is double a) DrawMarker(dc, new Point(x, YForWatts(a)));
            DrawMarker(dc, new Point(x, YForSeconds(s.TauSeconds)));

            string draw = s.Watts is double w ? Inv($"{w:0} W") : "—";
            string avg = s.WattsAvg is double av ? Inv($"{av:0} W") : "—";
            // Same vocabulary as the dev panel's power readout: buffer / headroom / needs.
            string budget = Inv($"buffer {s.BudgetJoules / 1000:0.0} kJ · headroom {Headroom(s.TauSeconds)} · needs {s.DemandLevel:0}%");
            DrawChip(dc, x,
                Inv($"{Ago(idx, count)} · {draw} · avg {avg} · {budget}"),
                Inv($"{Ago(idx, count)} · {draw} · avg {avg}\n{budget}"));
        }
    }
}
