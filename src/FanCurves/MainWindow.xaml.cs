using System.ComponentModel;
using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using FanCurves.Core;

namespace FanCurves;

public partial class MainWindow : Window
{
    private readonly IHardwareBackend _hw;
    private readonly FanEngine _engine;
    private readonly Profile _profile;
    private readonly List<ChannelVm> _channelVms = new();
    private readonly List<ChannelHistory> _histories = new(); // parallel to _channelVms
    private readonly HistoryViewport _viewport = new();       // scroll state of the strip
    private bool _loadingUi; // guard so slider init doesn't write back into the config
    private bool _devMode;
    private bool _exiting;
    private readonly TrayIcon _tray;
    private IReadOnlyList<ChannelStatus>? _lastStatuses;
    private string? _lastTrayTip; // skip the Shell_NotifyIcon call when unchanged

    // Undo/redo, Ctrl+Z / Ctrl+Y. Each entry is one committed edit, of two kinds:
    // a curve edit (drag / add / remove — the baselines below hold the last known points
    // per channel so a CurveChanged can be diffed into a before/after pair) or a preset
    // switch, which rewrites every channel's tuning and so stores whole-profile snapshots.
    private abstract record Edit;
    private sealed record CurveEdit(ChannelConfig Channel,
        List<CurvePoint> Before, List<CurvePoint> After, string BeforeName) : Edit;
    private sealed record TuningEdit(TuningSnapshot Before, TuningSnapshot After) : Edit;
    private readonly List<Edit> _undoStack = new();
    private readonly List<Edit> _redoStack = new();
    private readonly Dictionary<ChannelConfig, List<CurvePoint>> _pointsBaseline = new();

    // Live readouts in the dev panel's source lists: temp before each sensor name,
    // rpm before each fan header name. Rebuilt with the lists, refreshed per tick.
    private readonly List<(TextBlock Value, string Id)> _sensorReadouts = new();
    private readonly List<(TextBlock Value, string Id)> _controlReadouts = new();

    private static string Inv(FormattableString f) => FormattableString.Invariant(f);

    // AvgSlider is non-linear: notches 0–24 → 0–120 s in 5 s steps, 25–30 → 150–300 s in 30 s steps.
    private static double AvgNotchToSeconds(double notch) =>
        notch <= 24 ? notch * 5 : 120 + (notch - 24) * 30;

    private static double AvgSecondsToNotch(double seconds) =>
        seconds <= 120 ? Math.Round(seconds / 5) : Math.Min(30, 24 + Math.Round((seconds - 120) / 30));

    // SensorHistorySlider is notched over useful windows, not linear hours.
    private static readonly double[] SensorHistoryNotchMinutes =
        { 0, 5, 10, 15, 30, 60, 120, 240, 480, 720, 1440 };

    private static int SensorHistoryHoursToNotch(double hours)
    {
        double minutes = hours * 60;
        int best = 0;
        for (int i = 1; i < SensorHistoryNotchMinutes.Length; i++)
            if (Math.Abs(SensorHistoryNotchMinutes[i] - minutes) <
                Math.Abs(SensorHistoryNotchMinutes[best] - minutes)) best = i;
        return best;
    }

    private static string FormatAvg(double s) =>
        s < 60 ? Inv($"{s:0} s")
        : s % 60 == 0 ? Inv($"{s / 60:0} min")
        : Inv($"{Math.Floor(s / 60):0} min {s % 60:0} s");

    public MainWindow(IHardwareBackend hw, FanEngine engine, Profile profile,
        bool devMode = false)
    {
        _hw = hw;
        _engine = engine;
        _profile = profile;
        InitializeComponent();
        Icon = TrayIcon.CreateWindowIcon();

        _tray = new TrayIcon();
        _tray.OpenRequested += () =>
        {
            Show();
            if (WindowState == WindowState.Minimized) WindowState = WindowState.Normal;
            Activate();
        };
        _tray.ExitRequested += ForceExit;

        BackendText.Text = hw.Description;
        ConfigPathText.Text = Profile.ConfigPath;
        TrayCheck.IsChecked = profile.MinimizeToTrayOnClose;
        AutostartCheck.IsChecked = profile.AutostartEnabled;
        IdleKickCheck.IsChecked = profile.IdleKickEnabled;
        ZeroSnapCheck.IsChecked = profile.ZeroSnapEnabled;
        StopProbeCheck.IsChecked = profile.StopProbeEnabled;
        SafetyFloorCheck.IsChecked = profile.SafetyFloorEnabled;
        InstantApplyCheck.IsChecked = profile.InstantApplyEnabled;
        TelemetryCheck.IsChecked = profile.TelemetryLoggingEnabled;
        HighPriorityCheck.IsChecked = profile.HighPriorityEnabled;
        _loadingUi = true;
        KickIdleSlider.Value = profile.IdleKickStoppedSeconds;
        KickSpeedSlider.Value = profile.IdleKickPercent;
        KickTimeSlider.Value = profile.IdleKickSeconds;
        ZeroSnapSlider.Value = profile.ZeroSnapPercent;
        ProbeRunSlider.Value = profile.StopProbeRunSeconds;
        ProbeLenSlider.Value = profile.StopProbeSeconds;
        ProbeBandSlider.Value = profile.StopProbeStableRangeC;
        ProbeRetrySlider.Value = profile.StopProbeRetrySeconds;
        ProbeMaxTempSlider.Value = profile.StopProbeMaxTempC;
        SensorHistorySlider.Value = SensorHistoryHoursToNotch(profile.SensorHistoryHours);
        _loadingUi = false;
        UpdateSensorHistoryLabel();
        UpdateKickLabels();
        UpdateZeroSnapLabel();
        UpdateStopProbeLabels();
        UpdateFeatureGroupDim();
        SimTag.Visibility = hw.IsSimulated ? Visibility.Visible : Visibility.Collapsed;

        foreach (var ch in _profile.Channels)
        {
            _channelVms.Add(new ChannelVm(ch));
            _histories.Add(new ChannelHistory());
            _pointsBaseline[ch] = ch.Points.ToList();
        }
        HistoryView.Viewport = _viewport;
        // Scrolled back = not live: the LIVE jump-back button appears next to CLEAR.
        _viewport.Changed += () => LiveButton.Visibility =
            _devMode && !_viewport.Live ? Visibility.Visible : Visibility.Collapsed;
        ChannelList.ItemsSource = _channelVms;
        ChannelList.SelectedIndex = 0;

        // Ctrl+Z / Ctrl+Y (ApplicationCommands' default gestures) — plus Ctrl+Shift+Z for redo.
        CommandBindings.Add(new CommandBinding(ApplicationCommands.Undo, (_, _) => UndoEdit()));
        CommandBindings.Add(new CommandBinding(ApplicationCommands.Redo, (_, _) => RedoEdit()));
        InputBindings.Add(new KeyBinding(ApplicationCommands.Redo, Key.Z, ModifierKeys.Control | ModifierKeys.Shift));

        Editor.CurveChanged += OnCurveEdited;
        _engine.Ticked += OnEngineTicked;
        _engine.Start();

        if (devMode) OnToggleDev(this, null!);
        _engine.Apply(); // curves are live from launch; presets/edits take effect instantly
        UpdatePresetHighlight();
        UpdateChip();
        StartSpin();

        // Three sizes only (no drag-resize): the fixed floating window, a quarter of the
        // screen snapped to the nearest corner, and maximized — cycled by the caption
        // button. Chrome's WM_GETMINMAXINFO hook keeps maximize inside the work area.
        StateChanged += (_, _) =>
        {
            if (WindowState == WindowState.Maximized) _sizeMode = SizeMode.Max;
            else if (WindowState == WindowState.Normal && _sizeMode == SizeMode.Max) EnterFixed();
            UpdateSizeGlyph();
            // Restored from minimized: ticks were skipping the UI — catch up now.
            if (WindowState != WindowState.Minimized) RefreshLiveUi();
        };
        UpdateSizeGlyph();

        // Reopened from the tray: same catch-up (per-tick painting is skipped while
        // the window is hidden — the app lives in the tray most of the time).
        IsVisibleChanged += (_, e) => { if (e.NewValue is true) RefreshLiveUi(); };

        Opacity = 0;
        Loaded += (_, _) =>
        {
            var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
            BeginAnimation(OpacityProperty,
                new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(260)) { EasingFunction = ease });
            ContentShift.BeginAnimation(System.Windows.Media.TranslateTransform.YProperty,
                new DoubleAnimation(10, 0, TimeSpan.FromMilliseconds(340)) { EasingFunction = ease });
        };
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);
        Chrome.Apply(this); // Win11 rounded corners + dark frame
    }

    private ChannelConfig? SelectedChannel =>
        (ChannelList.SelectedItem as ChannelVm)?.Config;

    private ChannelStatus? SelectedStatus
    {
        get
        {
            int sel = ChannelList.SelectedIndex;
            return _lastStatuses != null && sel >= 0 && sel < _lastStatuses.Count
                ? _lastStatuses[sel] : null;
        }
    }

    // ---- Title bar ----

    private void OnMinimize(object sender, RoutedEventArgs e) => WindowState = WindowState.Minimized;

    private enum SizeMode { Fixed, Quarter, Max }
    private SizeMode _sizeMode = SizeMode.Fixed;

    private void OnMaximize(object sender, RoutedEventArgs e)
    {
        switch (_sizeMode)
        {
            case SizeMode.Fixed: EnterQuarter(); break;
            case SizeMode.Quarter: WindowState = WindowState.Maximized; break; // StateChanged → Max
            default: WindowState = WindowState.Normal; break;                  // StateChanged → EnterFixed
        }
    }

    /// <summary>Half the work area in each dimension, snapped to the nearest screen corner.</summary>
    private void EnterQuarter()
    {
        var wa = Chrome.WorkAreaDip(this);
        double cx = Left + ActualWidth / 2, cy = Top + ActualHeight / 2;
        _sizeMode = SizeMode.Quarter;
        WindowState = WindowState.Normal;
        Width = wa.Width / 2;
        Height = wa.Height / 2;
        Left = cx < wa.Left + wa.Width / 2 ? wa.Left : wa.Right - Width;
        Top = cy < wa.Top + wa.Height / 2 ? wa.Top : wa.Bottom - Height;
        UpdateSizeGlyph();
    }

    private void EnterFixed()
    {
        _sizeMode = SizeMode.Fixed;
        var wa0 = Chrome.WorkAreaDip(this);
        // Developer mode adds the settings panel and the history strip under the
        // curve — wider and taller, unless the screen is too small for it.
        Width = _devMode ? Math.Min(1336, wa0.Width) : 1010;
        Height = _devMode ? Math.Min(830, wa0.Height) : 660;
        var wa = Chrome.WorkAreaDip(this);
        Left = Math.Max(wa.Left, Math.Min(Left, wa.Right - Width));
        Top = Math.Max(wa.Top, Math.Min(Top, wa.Bottom - Height));
        UpdateSizeGlyph();
    }

    // The glyph previews the size the button switches to next.
    private void UpdateSizeGlyph() =>
        MaxGlyph.Data = Geometry.Parse(_sizeMode switch
        {
            SizeMode.Fixed => "M 0,0 H 9 V 9 H 0 Z M 4.5,9 V 4.5 H 9", // quarter of the square
            SizeMode.Quarter => "M 0,0 H 9 V 9 H 0 Z",                 // full square: maximize
            _ => "M 2,0 H 9 V 7 M 0,2 H 7 V 9 H 0 Z",                  // restore: offset back square
        });

    private void OnCloseButton(object sender, RoutedEventArgs e) => Close();

    /// <summary>Title-bar fan glyph turns while curves are applied.</summary>
    private void StartSpin()
    {
        var spin = new DoubleAnimation(FanSpin.Angle, FanSpin.Angle + 360, TimeSpan.FromSeconds(9))
        { RepeatBehavior = RepeatBehavior.Forever };
        // The one perpetual animation in the app. At the default 60 fps it forces a
        // composition pass per frame for as long as the window shows; the glyph turns
        // a lazy 40°/s, so 20 fps is indistinguishable and three times cheaper.
        Timeline.SetDesiredFrameRate(spin, 20);
        FanSpin.BeginAnimation(RotateTransform.AngleProperty, spin);
    }

    private void StopSpin()
    {
        double angle = FanSpin.Angle;
        FanSpin.BeginAnimation(RotateTransform.AngleProperty, null);
        FanSpin.Angle = angle % 360;
    }

    // ---- Presets (the whole UI in simple mode) ----

    private void OnQuietPreset(object sender, RoutedEventArgs e) => AdoptPreset(Profile.MacBookLike());
    private void OnPerformancePreset(object sender, RoutedEventArgs e) => AdoptPreset(Profile.Performance());

    private void AdoptPreset(Profile preset)
    {
        var before = _profile.CaptureTuning();
        _profile.AdoptTuning(preset);
        var after = _profile.CaptureTuning();
        _profile.Save();
        // A preset rewrites every curve and every behaviour knob, so the whole tuning
        // goes on the undo stack as one entry — re-clicking the preset you are already
        // on changes nothing and must not push one. Curve edits made before the switch
        // stay on the stack below it: undo is LIFO, so they are only reached once this
        // entry has put their state back.
        if (!before.Matches(after))
        {
            _undoStack.Add(new TuningEdit(before, after));
            if (_undoStack.Count > 100) _undoStack.RemoveAt(0);
            _redoStack.Clear();
        }
        ResetCurveBaselines();
        OnChannelSelected(this, null!);
        Editor.InvalidateVisual();
        UpdatePresetHighlight();
    }

    private void ResetCurveBaselines()
    {
        foreach (var ch in _profile.Channels)
            _pointsBaseline[ch] = ch.Points.ToList();
    }

    private void UpdatePresetHighlight()
    {
        bool quiet = _profile.Name == Profile.MacBookLike().Name;
        bool perf = _profile.Name == Profile.Performance().Name;
        QuietPresetButton.Tag = quiet ? "on" : null;
        PerfPresetButton.Tag = perf ? "on" : null;
        CustomNote.Visibility = quiet || perf ? Visibility.Collapsed : Visibility.Visible;
    }

    private void OnCurveEdited()
    {
        var ch = Editor.Channel;
        if (ch == null) return;
        var current = ch.Points.ToList();
        var before = _pointsBaseline.TryGetValue(ch, out var b) ? b : current;
        if (before.SequenceEqual(current)) return; // e.g. a click on a point with no drag

        _undoStack.Add(new CurveEdit(ch, before, current, _profile.Name));
        if (_undoStack.Count > 100) _undoStack.RemoveAt(0);
        _redoStack.Clear();
        _pointsBaseline[ch] = current;

        _profile.Name = "Custom";
        _profile.Save();
        UpdatePresetHighlight();
    }

    private void UndoEdit()
    {
        if (_undoStack.Count == 0) return;
        var edit = _undoStack[^1];
        _undoStack.RemoveAt(_undoStack.Count - 1);
        _redoStack.Add(StepEdit(edit, undo: true));
    }

    private void RedoEdit()
    {
        if (_redoStack.Count == 0) return;
        var edit = _redoStack[^1];
        _redoStack.RemoveAt(_redoStack.Count - 1);
        _undoStack.Add(StepEdit(edit, undo: false));
    }

    /// <summary>Applies one edit in the given direction and returns the entry to push
    /// onto the opposite stack.</summary>
    private Edit StepEdit(Edit edit, bool undo)
    {
        switch (edit)
        {
            case CurveEdit c:
                ApplyCurveEdit(c, undo ? c.Before : c.After, undo ? c.BeforeName : "Custom");
                return c;
            case TuningEdit t:
                // Tuning that is not itself undoable (the dev-panel behaviour sliders) may
                // have moved since the switch. Re-capture before restoring, so stepping
                // back the other way returns what was really on screen, not a stale
                // snapshot that would silently drop those edits.
                var current = _profile.CaptureTuning();
                ApplyTuningEdit(undo ? t.Before : t.After);
                return undo ? t with { After = current } : t with { Before = current };
            default:
                return edit;
        }
    }

    private void ApplyTuningEdit(TuningSnapshot snapshot)
    {
        _profile.ApplyTuning(snapshot);
        _profile.Save();
        ResetCurveBaselines();
        OnChannelSelected(this, null!); // dev sliders + editor follow the restored tuning
        Editor.InvalidateVisual();
        UpdatePresetHighlight();
    }

    private void ApplyCurveEdit(CurveEdit edit, List<CurvePoint> points, string name)
    {
        var ch = edit.Channel;
        ch.Points.Clear();
        ch.Points.AddRange(points);
        _pointsBaseline[ch] = points.ToList();
        _profile.Name = name;
        _profile.Save();

        // Bring the affected channel on screen so the restore is visible.
        int i = _channelVms.FindIndex(vm => vm.Config == ch);
        if (i >= 0 && ChannelList.SelectedIndex != i) ChannelList.SelectedIndex = i;
        Editor.InvalidateVisual();
        UpdatePresetHighlight();
    }

    // ---- Developer mode ----

    private void OnToggleDev(object sender, RoutedEventArgs e)
    {
        _devMode = !_devMode;
        DevPanel.Visibility = _devMode ? Visibility.Visible : Visibility.Collapsed;
        EditorHint.Visibility = _devMode ? Visibility.Visible : Visibility.Collapsed;
        Editor.IsReadOnly = !_devMode;
        Editor.ShowRaw = _devMode;
        HistoryView.Visibility = _devMode ? Visibility.Visible : Visibility.Collapsed;
        ClearHistoryButton.Visibility = _devMode ? Visibility.Visible : Visibility.Collapsed;
        LiveButton.Visibility = _devMode && !_viewport.Live ? Visibility.Visible : Visibility.Collapsed;
        DevButton.Tag = _devMode ? "on" : null;
        // Developer mode needs room for the panel and the history strip; the floating
        // window grows in both directions rather than squeezing the curve editor
        // (EnterFixed keeps the grown window inside the work area).
        if (_sizeMode == SizeMode.Fixed) EnterFixed();
        if (_devMode) OnChannelSelected(this, null!); // sync sliders to selection
        UpdateDetail();
    }

    private void OnChannelSelected(object sender, SelectionChangedEventArgs? e)
    {
        var ch = SelectedChannel;
        Editor.Channel = ch;
        int idx = ChannelList.SelectedIndex;
        // Switching channel drops any scroll-back anchor — the strip lands on live data.
        _viewport.History = idx >= 0 && idx < _histories.Count ? _histories[idx] : null;
        if (ch == null) return;

        _loadingUi = true;
        AvgSlider.Value = AvgSecondsToNotch(ch.AveragingSeconds);
        HystSlider.Value = ch.HysteresisC;
        HoldSlider.Value = ch.StepDownHoldSeconds;
        SlewUpSlider.Value = ch.SlewUpPercentPerSec;
        SlewDownSlider.Value = ch.SlewDownPercentPerSec;
        FloorSlider.Value = ch.MinPercent;
        RebuildSourceChecks(ch);
        _loadingUi = false;
        UpdateParamLabels(ch);
        UpdateHero();
        UpdateDetail();
    }

    private void RebuildSourceChecks(ChannelConfig ch)
    {
        SensorChecks.Items.Clear();
        _sensorReadouts.Clear();
        foreach (var s in _hw.Sensors.Where(s => s.Kind == "temp"))
        {
            var cb = new CheckBox { Content = SourceLabel(s.Name, 44, out var val), IsChecked = ch.SensorIds.Contains(s.Id), Tag = s.Id };
            _sensorReadouts.Add((val, s.Id));
            cb.Checked += (_, _) => { if (!ch.SensorIds.Contains(s.Id)) ch.SensorIds.Add(s.Id); _profile.Save(); };
            cb.Unchecked += (_, _) => { ch.SensorIds.Remove(s.Id); _profile.Save(); };
            SensorChecks.Items.Add(cb);
        }
        ControlChecks.Items.Clear();
        _controlReadouts.Clear();
        foreach (var c in _hw.Controls)
        {
            var cb = new CheckBox { Content = SourceLabel(c.Name, 58, out var val), IsChecked = ch.ControlIds.Contains(c.Id), Tag = c.Id };
            _controlReadouts.Add((val, c.Id));
            cb.Checked += (_, _) =>
            {
                // A header obeys exactly ONE PWM value, so ownership is exclusive:
                // with the same header on two channels, FanEngine.Tick writes both
                // in Profile.Channels order and the LAST one silently wins every
                // tick — a fan parked at 0% by the other channel's curve, with no
                // why-chip and nothing in the UI to hint at it. Sensors are not
                // exclusive (both channels may legitimately read one temp).
                foreach (var other in _profile.Channels)
                    if (!ReferenceEquals(other, ch)) other.ControlIds.Remove(c.Id);
                if (!ch.ControlIds.Contains(c.Id)) ch.ControlIds.Add(c.Id);
                _profile.Save();
                // No list rebuild here: the panel only ever shows the SELECTED
                // channel, and RebuildSourceChecks runs on every channel switch.
                // Calling it from this handler would clear ControlChecks.Items
                // while the checkbox raising the event still lives in it.
            };
            cb.Unchecked += (_, _) => { ch.ControlIds.Remove(c.Id); _hw.ReleaseControl(c.Id); _profile.Save(); };
            ControlChecks.Items.Add(cb);
        }
        if (_hw.Controls.Count == 0)
            ControlChecks.Items.Add(new TextBlock
            {
                Text = "No controllable headers found.",
                Foreground = Paint.White(0x8c),
            });
        RefreshSourceReadouts();
    }

    /// <summary>Checkbox label "value column · name" — the value TextBlock is filled per tick.
    /// The whole row runs in the mono font so value and name read as one line; long names
    /// wrap under themselves (value column stays fixed) so the full name is always visible.</summary>
    private Grid SourceLabel(string name, double valueWidth, out TextBlock value)
    {
        var mono = (FontFamily)FindResource("Mono");
        value = new TextBlock
        {
            Text = "—",
            FontFamily = mono,
            FontSize = 11.5,
            Foreground = Paint.White(0x8c),
            TextAlignment = TextAlignment.Left,
            Margin = new Thickness(0, 0, 8, 0),
            VerticalAlignment = VerticalAlignment.Top,
        };
        var nameBlock = new TextBlock
        {
            Text = name,
            FontFamily = mono,
            FontSize = 11.5,
            TextWrapping = TextWrapping.Wrap,
            VerticalAlignment = VerticalAlignment.Top,
        };
        var grid = new Grid();
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(valueWidth + 8) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        Grid.SetColumn(nameBlock, 1);
        grid.Children.Add(value);
        grid.Children.Add(nameBlock);
        return grid;
    }

    private void RefreshSourceReadouts()
    {
        foreach (var (tb, id) in _sensorReadouts)
            tb.Text = _hw.ReadValue(id) is double v ? Inv($"{v:0.0}°") : "—";
        foreach (var (tb, id) in _controlReadouts)
            tb.Text = _hw.ReadControlRpm(id) is double r ? Inv($"{r:0} rpm") : "—";
    }

    private void OnParamChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        if (_loadingUi) return;
        var ch = SelectedChannel;
        if (ch == null) return;
        ch.AveragingSeconds = AvgNotchToSeconds(AvgSlider.Value);
        ch.HysteresisC = HystSlider.Value;
        ch.StepDownHoldSeconds = HoldSlider.Value;
        ch.SlewUpPercentPerSec = SlewUpSlider.Value;
        ch.SlewDownPercentPerSec = SlewDownSlider.Value;
        ch.MinPercent = FloorSlider.Value;
        UpdateParamLabels(ch);
        _profile.Name = "Custom";
        _profile.Save();
        UpdatePresetHighlight();
    }

    private void UpdateParamLabels(ChannelConfig ch)
    {
        AvgValue.Text = FormatAvg(ch.AveragingSeconds);
        HystValue.Text = Inv($"{ch.HysteresisC:0.#} °C");
        HoldValue.Text = Inv($"{ch.StepDownHoldSeconds:0} s");
        SlewUpValue.Text = Inv($"{ch.SlewUpPercentPerSec:0.#} %/s");
        SlewDownValue.Text = Inv($"{ch.SlewDownPercentPerSec:0.#} %/s");
        FloorValue.Text = Inv($"{ch.MinPercent:0} %");
        Tracked.SetText(HeroLabel,
            $"{ch.Name} · {FormatAvg(ch.AveragingSeconds)} avg".ToUpperInvariant());
    }

    // ---- Engine feedback ----

    private void OnEngineTicked(IReadOnlyList<ChannelStatus> statuses)
    {
        Dispatcher.BeginInvoke(() =>
        {
            _lastStatuses = statuses;
            for (int i = 0; i < statuses.Count && i < _histories.Count; i++)
            {
                var t = statuses[i];
                _histories[i].Add(new HistorySample(
                    DateTime.Now, t.RawTemp, t.EffectiveTemp, t.OutputPercent));
            }

            // Tray tooltip: rebuilt per tick, but the Win32 update only goes out when
            // the whole-degree readout actually changed (most idle ticks it hasn't).
            string tip = "Fan Curves — " + string.Join(" · ", statuses.Select(t =>
                $"{(double.IsNaN(t.EffectiveTemp) ? "?" : Inv($"{t.EffectiveTemp:0}°"))}→{t.OutputPercent:0}%"));
            if (tip != _lastTrayTip)
            {
                _lastTrayTip = tip;
                _tray.SetStatus(tip);
            }

            // Everything below only paints the window. Hidden in the tray (where this
            // app spends most of its life) or minimized, skip it all — the histories
            // above are already fed, and RefreshLiveUi catches the window up the
            // moment it comes back.
            if (IsVisible && WindowState != WindowState.Minimized) RefreshLiveUi();
        });
    }

    /// <summary>Everything a tick paints: segment readouts, charts, hero, chips, and
    /// the dev-panel readouts. Runs per tick while the window shows, and once on
    /// reopen/restore to catch up after the hidden-in-tray skip.</summary>
    private void RefreshLiveUi()
    {
        var statuses = _lastStatuses;
        if (statuses == null) return;
        for (int i = 0; i < statuses.Count && i < _channelVms.Count; i++)
            _channelVms[i].UpdateFrom(statuses[i]);

        var s = SelectedStatus;
        if (s != null)
            Editor.UpdateLive(s.RawTemp, s.EffectiveTemp, s.OutputPercent);
        HistoryView.Refresh();

        UpdateHero();
        UpdateDetail();
        UpdateChip();
        if (_devMode) RefreshSourceReadouts();
    }

    /// <summary>Big numeral: the selected channel's rolling average (what drives the steps).</summary>
    private void UpdateHero()
    {
        var s = SelectedStatus;
        double? t = s == null ? null : double.IsNaN(s.EffectiveTemp) ? s.RawTemp : s.EffectiveTemp;
        if (t is double v)
        {
            string str = v.ToString("0.0", CultureInfo.InvariantCulture);
            int dot = str.IndexOf('.');
            HeroInt.Text = str[..dot];
            HeroFrac.Text = str[dot..] + "°";
        }
        else
        {
            HeroInt.Text = "—";
            HeroFrac.Text = "";
        }
    }

    /// <summary>Card-header line for the selected channel: rpm, raw temp (dev), warnings.</summary>
    private void UpdateDetail()
    {
        var s = SelectedStatus;
        if (s == null) { DetailText.Text = ""; WhyChip.Visibility = Visibility.Collapsed; return; }
        var parts = new List<string>();
        if (s.RawTemp == null) parts.Add("no temp sensor");
        else if (_devMode) parts.Add(Inv($"now {s.RawTemp:0.0}°"));
        if (s.Rpm is double r) parts.Add(Inv($"{r:0} rpm"));
        if (!s.Applied) parts.Add("preview — no fan header");
        DetailText.Text = string.Join(" · ", parts);
        UpdateWhy(s);
    }

    /// <summary>
    /// Chart-corner notification: why the commanded % differs from the curve's
    /// configured level (hidden when they match).
    /// </summary>
    private void UpdateWhy(ChannelStatus s)
    {
        var ch = SelectedChannel;
        string? why = ch == null ? null : s.Reason switch
        {
            OutputReason.RampUp =>
                Inv($"ramping up to {s.TargetPercent:0}% · {ch.SlewUpPercentPerSec:0.#} %/s"),
            OutputReason.RampDown =>
                Inv($"ramping down to {s.TargetPercent:0}% · {ch.SlewDownPercentPerSec:0.#} %/s"),
            OutputReason.StepDownHold =>
                Inv($"holding {s.OutputPercent:0}% · steps down to {s.ReasonLevel:0}% in {Math.Ceiling(s.ReasonSeconds):0} s"),
            OutputReason.Hysteresis =>
                Inv($"holding {s.OutputPercent:0}% · avg not yet {ch.HysteresisC:0.#}° below the step"),
            OutputReason.ZeroSnap =>
                Inv($"stopped · curve's {s.ReasonLevel:0}% is under the {_profile.ZeroSnapPercent:0}% stop threshold"),
            OutputReason.MinFloor =>
                Inv($"safety floor {s.OutputPercent:0}% · curve asks {s.ReasonLevel:0}%"),
            OutputReason.IdleKick => "idle kick · spinning the stopped fan briefly",
            OutputReason.StopProbe => "trial stop · resumes the moment the temp rises",
            _ => null,
        };
        WhyChip.Visibility = why == null ? Visibility.Collapsed : Visibility.Visible;
        WhyText.Text = why ?? "";
    }

    private void UpdateChip()
    {
        bool applying = _engine.Applying;
        string text;
        bool warm; // amber dot = live; gray dot = paused
        if (!applying) { text = "Paused — fans on the BIOS curve"; warm = false; }
        else if (_lastStatuses != null && _lastStatuses.Any(s => s.RawTemp == null))
        { text = "No temp sensor — assign in Developer mode"; warm = true; }
        else if (_hw.IsSimulated) { text = "Simulation — demo data"; warm = true; }
        else { text = "Curves active"; warm = true; }

        ChipText.Text = text;
        ChipDot.Fill = warm ? (Brush)FindResource("Accent") : Paint.White(0x66);
        ChipGlow.Opacity = warm ? 0.9 : 0.0;
    }

    private void OnTogglePause(object sender, RoutedEventArgs e)
    {
        if (_engine.Applying)
        {
            _engine.StopApplying();
            PauseButton.Content = "Resume curves";
            PauseButton.Style = (Style)FindResource("PrimaryButton");
            StopSpin();
        }
        else
        {
            _engine.Apply();
            PauseButton.Content = "Pause — fans to BIOS";
            PauseButton.Style = (Style)FindResource(typeof(Button));
            StartSpin();
        }
        UpdateChip();
    }

    /// <summary>Exit regardless of the tray setting (tray menu, exit.signal).</summary>
    public void ForceExit() { _exiting = true; Close(); }

    private void OnTrayCheckChanged(object sender, RoutedEventArgs e)
    {
        _profile.MinimizeToTrayOnClose = TrayCheck.IsChecked == true;
        _profile.Save();
    }

    /// <summary>Feature groups dim while their master switch is off. The sliders stay
    /// fully editable either way — settings must be reachable with the checkbox off
    /// (Kuba, 2026-07-21) — the opacity only signals the group's state at a glance.</summary>
    private void UpdateFeatureGroupDim()
    {
        KickGroup.Opacity = IdleKickCheck.IsChecked == true ? 1.0 : 0.45;
        ZeroSnapGroup.Opacity = ZeroSnapCheck.IsChecked == true ? 1.0 : 0.45;
        StopProbeGroup.Opacity = StopProbeCheck.IsChecked == true ? 1.0 : 0.45;
        SafetyFloorGroup.Opacity = SafetyFloorCheck.IsChecked == true ? 1.0 : 0.45;
    }

    private void OnIdleKickCheckChanged(object sender, RoutedEventArgs e)
    {
        _profile.IdleKickEnabled = IdleKickCheck.IsChecked == true;
        _profile.Save();
        UpdateFeatureGroupDim();
    }

    private void OnKickParamChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        // KickTimeValue is the last kick control in the XAML: while it is still null
        // the tree is mid-parse (Minimum="…" fires ValueChanged) — nothing to update yet.
        if (_loadingUi || KickTimeValue == null) return;
        _profile.IdleKickStoppedSeconds = KickIdleSlider.Value;
        _profile.IdleKickPercent = KickSpeedSlider.Value;
        _profile.IdleKickSeconds = KickTimeSlider.Value;
        UpdateKickLabels();
        _profile.Save();
    }

    private void UpdateKickLabels()
    {
        KickIdleValue.Text = FormatAvg(_profile.IdleKickStoppedSeconds);
        KickSpeedValue.Text = Inv($"{_profile.IdleKickPercent:0} %");
        KickTimeValue.Text = Inv($"{_profile.IdleKickSeconds:0} s");
    }

    private void OnZeroSnapCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked from the profile
        _profile.ZeroSnapEnabled = ZeroSnapCheck.IsChecked == true;
        _profile.Save();
        UpdateFeatureGroupDim();
    }

    private void OnZeroSnapParamChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        // Fires mid-XAML-parse (setting Minimum) and from the constructor — both guarded.
        if (!IsLoaded || _loadingUi) return;
        _profile.ZeroSnapPercent = ZeroSnapSlider.Value;
        UpdateZeroSnapLabel();
        _profile.Save();
    }

    private void UpdateZeroSnapLabel()
    {
        ZeroSnapValue.Text = Inv($"{_profile.ZeroSnapPercent:0} %");
    }

    private void OnStopProbeCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked from the profile
        _profile.StopProbeEnabled = StopProbeCheck.IsChecked == true;
        _profile.Save();
        UpdateFeatureGroupDim();
    }

    private void OnSafetyFloorCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked from the profile
        _profile.SafetyFloorEnabled = SafetyFloorCheck.IsChecked == true;
        _profile.Save();
        UpdateFeatureGroupDim();
    }

    private void OnInstantApplyCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked from the profile
        _profile.InstantApplyEnabled = InstantApplyCheck.IsChecked == true;
        _profile.Save();
    }

    private void OnTelemetryCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked from the profile
        bool on = TelemetryCheck.IsChecked == true;
        _profile.TelemetryLoggingEnabled = on;
        _profile.Save();
        // Mark the seam in the log itself, and push the buffered rows out before the
        // per-tick writer goes quiet (it only flushes every 5 s).
        App.Telemetry?.Event(on ? "review logging on" : "review logging off");
        if (!on) App.Telemetry?.Flush();
    }

    private void OnSensorHistoryChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        if (_loadingUi || SensorHistoryValue == null) return;
        _profile.SensorHistoryHours = SensorHistoryNotchMinutes[(int)SensorHistorySlider.Value] / 60.0;
        UpdateSensorHistoryLabel();
        _profile.Save();
        // The engine applies the window on its next tick (same thread as Update()).
    }

    private void UpdateSensorHistoryLabel()
    {
        double minutes = SensorHistoryNotchMinutes[(int)SensorHistorySlider.Value];
        if (minutes <= 0) { SensorHistoryValue.Text = "off"; return; }
        // LHM appends one 16-byte SensorValue per sensor every 4th update (~1 s tick),
        // for every sensor it tracks internally, until entries age past the window.
        double mb = _hw.InternalSensorCount * (minutes * 60 / 4.0) * 16 / (1024.0 * 1024.0);
        string dur = minutes < 60 ? Inv($"{minutes:0} min") : Inv($"{minutes / 60:0} h");
        SensorHistoryValue.Text = mb < 1 ? dur + " ≈ <1 MB" : Inv($"{dur} ≈ {mb:0} MB");
    }

    private void OnHighPriorityCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked from the profile
        _profile.HighPriorityEnabled = HighPriorityCheck.IsChecked == true;
        _profile.Save();
        App.ApplyProcessPriority(_profile.HighPriorityEnabled);
    }

    private void OnStopProbeParamChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
    {
        // Fires mid-XAML-parse (setting Minimum) and from the constructor — both guarded.
        if (!IsLoaded || _loadingUi) return;
        _profile.StopProbeRunSeconds = ProbeRunSlider.Value;
        _profile.StopProbeSeconds = ProbeLenSlider.Value;
        _profile.StopProbeStableRangeC = ProbeBandSlider.Value;
        _profile.StopProbeRetrySeconds = ProbeRetrySlider.Value;
        _profile.StopProbeMaxTempC = ProbeMaxTempSlider.Value;
        UpdateStopProbeLabels();
        _profile.Save();
    }

    private void UpdateStopProbeLabels()
    {
        ProbeRunValue.Text = FormatAvg(_profile.StopProbeRunSeconds);
        ProbeLenValue.Text = FormatAvg(_profile.StopProbeSeconds);
        ProbeBandValue.Text = Inv($"{_profile.StopProbeStableRangeC:0.0} °C");
        ProbeRetryValue.Text = FormatAvg(_profile.StopProbeRetrySeconds);
        ProbeMaxTempValue.Text = Inv($"{_profile.StopProbeMaxTempC:0} °C");
    }

    /// <summary>Wipe the history (ring + spill file) on every channel; the strip starts fresh.</summary>
    private void OnClearHistory(object sender, RoutedEventArgs e)
    {
        foreach (var h in _histories) h.Clear();
        _viewport.JumpToLive();
        _viewport.Invalidate();   // the cached window still holds the wiped samples
    }

    /// <summary>Jump the scrolled-back strip home to the live right edge.</summary>
    private void OnJumpToLive(object sender, RoutedEventArgs e) => _viewport.JumpToLive();

    private void OnAutostartCheckChanged(object sender, RoutedEventArgs e)
    {
        if (!IsLoaded) return; // constructor sets IsChecked; App handles registration at launch
        _profile.AutostartEnabled = AutostartCheck.IsChecked == true;
        _profile.Save();
        if (_profile.AutostartEnabled) Autostart.Ensure();
        else Autostart.Remove();
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        if (!_exiting && !App.ShuttingDown && _profile.MinimizeToTrayOnClose)
        {
            e.Cancel = true;
            Hide();
            _tray.ShowHiddenHint();
            return;
        }
        base.OnClosing(e); // default: X exits, OnClosed hands fans back to the BIOS
    }

    protected override void OnClosed(EventArgs e)
    {
        _tray.Dispose();
        _engine.Dispose();
        _hw.Dispose();
        base.OnClosed(e);
        App.RequestShutdown();
    }
}

/// <summary>Per-channel line shown inside its segment: "46° · 25%".</summary>
public class ChannelVm : INotifyPropertyChanged
{
    public ChannelConfig Config { get; }
    public string Name => Config.Name;
    public string Segment { get; private set; } = "—";

    public ChannelVm(ChannelConfig config) => Config = config;

    public void UpdateFrom(ChannelStatus s)
    {
        string temp = !double.IsNaN(s.EffectiveTemp)
            ? FormattableString.Invariant($"{s.EffectiveTemp:0}°")
            : s.RawTemp is double raw ? FormattableString.Invariant($"{raw:0}°") : "—";
        Segment = FormattableString.Invariant($"{temp} · {s.OutputPercent:0}%");
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Segment)));
    }

    public event PropertyChangedEventHandler? PropertyChanged;
}
