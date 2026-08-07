using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;

namespace FanCurves;

/// <summary>
/// Letter-spaced micro-labels. WPF has no letter-spacing property, so
/// <c>local:Tracked.Text="PROFILE"</c> inserts hair spaces between characters.
/// </summary>
public static class Tracked
{
    public static readonly DependencyProperty TextProperty = DependencyProperty.RegisterAttached(
        "Text", typeof(string), typeof(Tracked), new PropertyMetadata(null, OnTextChanged));

    public static void SetText(DependencyObject d, string value) => d.SetValue(TextProperty, value);
    public static string GetText(DependencyObject d) => (string)d.GetValue(TextProperty);

    public static string Space(string s) =>
        string.Join("  ", s.ToCharArray());

    private static void OnTextChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        if (d is TextBlock tb && e.NewValue is string s) tb.Text = Space(s);
    }
}

/// <summary>Win11 polish for the borderless window: rounded corners + dark frame.</summary>
public static class Chrome
{
    [DllImport("dwmapi.dll", PreserveSig = true)]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

    public static void Apply(Window w)
    {
        var hwnd = new WindowInteropHelper(w).Handle;
        int round = 2; // DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND
        DwmSetWindowAttribute(hwnd, 33, ref round, sizeof(int));
        int dark = 1; // DWMWA_USE_IMMERSIVE_DARK_MODE
        DwmSetWindowAttribute(hwnd, 20, ref dark, sizeof(int));
        HwndSource.FromHwnd(hwnd)?.AddHook(WndProc);
    }

    // Without a resize frame (ResizeMode=CanMinimize) a maximized borderless window
    // covers the whole screen, taskbar included — clamp it to the monitor work area.
    private const int WM_GETMINMAXINFO = 0x0024;

    // Windows refuses to drag-restore a maximized window that has no maximize
    // style (ResizeMode=CanMinimize) — the title-bar drag just dead-ends. Catch
    // the caption click ourselves: restore under the cursor, then hand off to
    // the OS move loop so the drag continues seamlessly.
    private const int WM_NCLBUTTONDOWN = 0x00A1;
    private const int WM_SYSCOMMAND = 0x0112;
    private const int HTCAPTION = 2;
    private const int SC_MOVE_CAPTION = 0xF012; // SC_MOVE + HTCAPTION

    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT pt);
    [DllImport("user32.dll")]
    private static extern IntPtr SendMessage(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)] private struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] private struct RECT { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    private struct MINMAXINFO
    {
        public POINT ptReserved, ptMaxSize, ptMaxPosition, ptMinTrackSize, ptMaxTrackSize;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MONITORINFO
    {
        public int cbSize;
        public RECT rcMonitor, rcWork;
        public int dwFlags;
    }

    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromWindow(IntPtr hwnd, int flags);
    [DllImport("user32.dll")]
    private static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFO info);

    /// <summary>Work area of the window's monitor, in WPF device-independent units.</summary>
    public static Rect WorkAreaDip(Window w)
    {
        var hwnd = new WindowInteropHelper(w).Handle;
        IntPtr monitor = MonitorFromWindow(hwnd, 2 /* MONITOR_DEFAULTTONEAREST */);
        var mi = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
        if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref mi))
            return SystemParameters.WorkArea;
        var topLeft = new Point(mi.rcWork.Left, mi.rcWork.Top);
        var bottomRight = new Point(mi.rcWork.Right, mi.rcWork.Bottom);
        if (PresentationSource.FromVisual(w)?.CompositionTarget?.TransformFromDevice is System.Windows.Media.Matrix m)
        {
            topLeft = m.Transform(topLeft);
            bottomRight = m.Transform(bottomRight);
        }
        return new Rect(topLeft, bottomRight);
    }

    /// <summary>Cursor position in WPF device-independent units.</summary>
    private static Point CursorDip(Window w)
    {
        GetCursorPos(out POINT pt);
        var p = new Point(pt.X, pt.Y);
        if (PresentationSource.FromVisual(w)?.CompositionTarget?.TransformFromDevice is System.Windows.Media.Matrix m)
            p = m.Transform(p);
        return p;
    }

    private static IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (msg == WM_NCLBUTTONDOWN && wParam.ToInt64() == HTCAPTION
            && HwndSource.FromHwnd(hwnd)?.RootVisual is Window w
            && w.WindowState == WindowState.Maximized)
        {
            var wa = WorkAreaDip(w);
            var cursor = CursorDip(w);
            // Keep the cursor at the same relative spot on the caption after
            // the restore, so the hand-off to the move loop doesn't jump.
            double frac = wa.Width > 0 ? (cursor.X - wa.Left) / wa.Width : 0.5;
            w.WindowState = WindowState.Normal; // StateChanged → EnterFixed
            w.Left = cursor.X - w.Width * frac;
            w.Top = wa.Top;
            SendMessage(hwnd, WM_SYSCOMMAND, (IntPtr)SC_MOVE_CAPTION, IntPtr.Zero);
            handled = true;
            return IntPtr.Zero;
        }

        if (msg != WM_GETMINMAXINFO) return IntPtr.Zero;
        IntPtr monitor = MonitorFromWindow(hwnd, 2 /* MONITOR_DEFAULTTONEAREST */);
        if (monitor == IntPtr.Zero) return IntPtr.Zero;
        var mi = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
        if (!GetMonitorInfo(monitor, ref mi)) return IntPtr.Zero;

        var mmi = Marshal.PtrToStructure<MINMAXINFO>(lParam);
        mmi.ptMaxPosition.X = mi.rcWork.Left - mi.rcMonitor.Left;
        mmi.ptMaxPosition.Y = mi.rcWork.Top - mi.rcMonitor.Top;
        mmi.ptMaxSize.X = mi.rcWork.Right - mi.rcWork.Left;
        mmi.ptMaxSize.Y = mi.rcWork.Bottom - mi.rcWork.Top;
        Marshal.StructureToPtr(mmi, lParam, true);
        handled = true;
        return IntPtr.Zero;
    }
}
