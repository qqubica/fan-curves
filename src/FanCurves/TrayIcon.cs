using System.Drawing;
using System.Drawing.Drawing2D;
using System.Windows.Forms;

namespace FanCurves;

/// <summary>
/// System-tray presence. The window "closes" into this; Exit lives in its menu.
/// Icon is drawn at runtime (no asset pipeline for one glyph).
/// </summary>
public class TrayIcon : IDisposable
{
    private readonly NotifyIcon _icon;
    private bool _balloonShown;

    public event Action? OpenRequested;
    public event Action? ExitRequested;

    public TrayIcon()
    {
        var menu = new ContextMenuStrip();
        menu.Items.Add("Open Fan Curves", null, (_, _) => OpenRequested?.Invoke());
        menu.Items.Add(new ToolStripSeparator());
        menu.Items.Add("Exit (fans back to BIOS)", null, (_, _) => ExitRequested?.Invoke());

        _icon = new NotifyIcon
        {
            Icon = DrawIcon(),
            Text = "Fan Curves",
            Visible = true,
            ContextMenuStrip = menu,
        };
        _icon.DoubleClick += (_, _) => OpenRequested?.Invoke();
    }

    public void SetStatus(string text)
    {
        // NotifyIcon.Text is capped at 127 chars (63 pre-Win11); stay safe.
        _icon.Text = text.Length > 60 ? text[..60] + "…" : text;
    }

    public void ShowHiddenHint()
    {
        if (_balloonShown) return;
        _balloonShown = true;
        _icon.BalloonTipTitle = "Fan Curves is still running";
        _icon.BalloonTipText = "Fans stay managed in the background. Right-click the tray icon to exit.";
        _icon.ShowBalloonTip(3000);
    }

    /// <summary>Same glyph as the tray, as a WPF ImageSource for Window.Icon (taskbar/alt-tab).</summary>
    internal static System.Windows.Media.ImageSource CreateWindowIcon()
    {
        using var icon = DrawIcon();
        return System.Windows.Interop.Imaging.CreateBitmapSourceFromHIcon(
            icon.Handle, System.Windows.Int32Rect.Empty,
            System.Windows.Media.Imaging.BitmapSizeOptions.FromEmptyOptions());
    }

    private static Icon DrawIcon()
    {
        using var bmp = new Bitmap(32, 32);
        using var g = Graphics.FromImage(bmp);
        g.SmoothingMode = SmoothingMode.AntiAlias;
        // Lookbook: dark pill with a hairline white ring, monochrome white glyph.
        using var bg = new SolidBrush(Color.FromArgb(8, 8, 10));
        g.FillEllipse(bg, 1, 1, 30, 30);
        using var ring = new Pen(Color.FromArgb(80, 255, 255, 255), 2);
        g.DrawEllipse(ring, 2, 2, 28, 28);
        // Three fan blades + hub.
        using var blade = new SolidBrush(Color.White);
        for (int i = 0; i < 3; i++)
        {
            g.TranslateTransform(16, 16);
            g.RotateTransform(120);
            g.TranslateTransform(-16, -16);
            g.FillEllipse(blade, 13.5f, 3f, 5f, 10f);
        }
        g.FillEllipse(blade, 13f, 13f, 6f, 6f);
        return Icon.FromHandle(bmp.GetHicon());
    }

    public void Dispose()
    {
        _icon.Visible = false;
        _icon.Dispose();
    }
}
