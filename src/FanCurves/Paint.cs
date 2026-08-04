using System.Windows;
using System.Windows.Media;

namespace FanCurves;

/// <summary>
/// Builders for the charts' frozen drawing resources. Every chart repaints at least
/// once per engine tick (and on every hover move), and a fresh unfrozen Freezable per
/// draw call is exactly the churn Freeze() exists to avoid: frozen pens/brushes skip
/// the thread-affinity bookkeeping, are shared with the render thread by reference,
/// and produce no per-frame garbage. Charts call these once, from static readonly
/// field initializers — the palette is monochrome white plus the one amber accent,
/// so the whole set is a couple dozen objects for the process lifetime.
/// </summary>
internal static class Paint
{
    /// <summary>3-on/3-off dashes — the "instantaneous value" reference style.</summary>
    public static readonly DashStyle Dash = Frozen(new DashStyle(new double[] { 3, 3 }, 0));

    /// <summary>1-on/4-off dots — the "configured limit" reference style (ceiling, ramp lead).</summary>
    public static readonly DashStyle Dots = Frozen(new DashStyle(new double[] { 1, 4 }, 0));

    public static SolidColorBrush Brush(Color c) => Frozen(new SolidColorBrush(c));

    /// <summary>White at the given opacity — the monochrome layer system.</summary>
    public static SolidColorBrush White(byte alpha) => Brush(Color.FromArgb(alpha, 0xff, 0xff, 0xff));

    public static Pen Pen(System.Windows.Media.Brush brush, double width,
        DashStyle? dash = null, bool rounded = false)
    {
        var p = new Pen(brush, width);
        if (dash != null) p.DashStyle = dash;
        if (rounded)
        {
            p.LineJoin = PenLineJoin.Round;
            p.StartLineCap = PenLineCap.Round;
            p.EndLineCap = PenLineCap.Round;
        }
        return Frozen(p);
    }

    public static Pen WhitePen(byte alpha, double width, DashStyle? dash = null, bool rounded = false) =>
        Pen(White(alpha), width, dash, rounded);

    /// <summary>Top-to-bottom white fade — the under-trace band fill.</summary>
    public static LinearGradientBrush VerticalFade(byte topAlpha, byte bottomAlpha) =>
        Frozen(new LinearGradientBrush(
            Color.FromArgb(topAlpha, 0xff, 0xff, 0xff), Color.FromArgb(bottomAlpha, 0xff, 0xff, 0xff),
            new System.Windows.Point(0, 0), new System.Windows.Point(0, 1)));

    public static T Frozen<T>(T freezable) where T : Freezable
    {
        freezable.Freeze();
        return freezable;
    }
}
