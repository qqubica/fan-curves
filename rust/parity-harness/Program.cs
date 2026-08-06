// Golden-trace generator for the Rust port: runs deterministic scenarios through
// the REAL C# FanCurves.Core components and writes per-tick CSVs into
// rust/crates/fan-core/tests/golden/. The Rust test `golden.rs` replays the same
// inputs through the ported components and compares every output column.
//
// Regenerate after any intentional behaviour change in FanCurves.Core:
//   dotnet run --project rust/parity-harness
using System.Globalization;
using System.Text;
using FanCurves.Core;

var outDir = args.Length > 0
    ? args[0]
    : Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "crates", "fan-core", "tests", "golden");
Directory.CreateDirectory(outDir);

string R(double v) => double.IsNaN(v) ? "" : v.ToString("R", CultureInfo.InvariantCulture);
string B(bool v) => v ? "1" : "0";

void WriteCsv(string name, string header, IEnumerable<string> rows)
{
    var path = Path.Combine(outDir, name);
    File.WriteAllLines(path, new[] { header }.Concat(rows), new UTF8Encoding(false));
    Console.WriteLine($"wrote {Path.GetFullPath(path)}");
}

// Deterministic tick spacing: 0.85–1.15 s, no RNG, exact in f64 on both sides.
static IEnumerable<double> Timeline(int ticks)
{
    double now = 0;
    for (int i = 0; i < ticks; i++)
    {
        yield return now;
        now += 0.85 + 0.3 * ((i * 7919) % 100) / 100.0;
    }
}

var quietCpu = new[]
{
    new CurvePoint(20, 0), new CurvePoint(50, 10), new CurvePoint(55, 20),
    new CurvePoint(62, 40), new CurvePoint(70, 50), new CurvePoint(76, 65),
    new CurvePoint(84, 81), new CurvePoint(88, 90), new CurvePoint(92, 100),
};
var quietCase = new[]
{
    new CurvePoint(20, 0), new CurvePoint(40, 5), new CurvePoint(42, 10),
    new CurvePoint(60, 20), new CurvePoint(70, 40), new CurvePoint(75, 60),
    new CurvePoint(82, 82), new CurvePoint(86, 100),
};
var perfCpu = new[]
{
    new CurvePoint(20, 45), new CurvePoint(50, 60), new CurvePoint(65, 75),
    new CurvePoint(80, 90), new CurvePoint(90, 100),
};

// ---- filter scenarios ------------------------------------------------------
// Temp is a pure function of tick index — spiky enough to cross bands both ways.

void FilterScenario(string name, ResponseFilter filter, CurvePoint[] curvePoints,
    Func<int, double> temp, Func<int, bool>? applyNow = null)
{
    var curve = new FanCurve(curvePoints);
    var rows = new List<string>();
    int i = 0;
    foreach (var now in Timeline(1400))
    {
        bool apply = applyNow?.Invoke(i) ?? false;
        if (apply) filter.ApplyNow();
        double t = temp(i);
        double output = filter.Step(now, t, curve);
        rows.Add(string.Join(',',
            R(now), R(t), B(apply), R(output), R(filter.TargetLevel), R(filter.CurveLevel),
            R(filter.PendingDownLevel), R(filter.DownHoldRemaining),
            B(filter.HysteresisHolding), B(filter.SnappedToZero)));
        i++;
    }
    WriteCsv(name, "now,temp,applynow,out,target,curve_level,pending,remaining,hyst,snapped", rows);
}

// A: Kuba's Quiet CPU tuning — long window, spike, sustained load, slow cooldown.
FilterScenario("filter-quiet-cpu.csv",
    new ResponseFilter
    {
        AveragingSeconds = 90, HysteresisC = 1.5, StepDownHoldSeconds = 25,
        SlewUpPercentPerSec = 9, SlewDownPercentPerSec = 8, ZeroSnapPercent = 20,
    },
    quietCpu,
    i => i switch
    {
        < 300 => 45 + (i % 7) * 0.1,          // idle wiggle
        < 310 => 90,                           // 10-tick spike the 90 s window must swallow
        < 420 => 45 + (i % 5) * 0.1,
        < 480 => 45 + (i - 420) * 0.55,        // climb toward load temp
        < 900 => 78 + (i % 9) * 0.2,           // sustained load
        _ => Math.Max(40, 78 - (i - 900) * 0.12), // cooldown → multi-band descent
    });

// B: Quiet case tuning — the 5/10% bands sit under the 20% snap: zero-snap city.
FilterScenario("filter-case-snap.csv",
    new ResponseFilter
    {
        AveragingSeconds = 25, HysteresisC = 4, StepDownHoldSeconds = 10,
        SlewUpPercentPerSec = 7, SlewDownPercentPerSec = 7, ZeroSnapPercent = 20,
    },
    quietCase,
    i => i switch
    {
        < 100 => 35,
        < 300 => 41 + (i % 3) * 0.2,           // 5/10% bands → snapped to 0
        < 400 => 45,
        < 700 => 65 + (i % 6) * 0.3,           // real 20/40% territory
        _ => Math.Max(38, 65 - (i - 700) * 0.08),
    });

// C: ApplyNow mid-settle and mid-hold — the instant-apply jump.
FilterScenario("filter-applynow.csv",
    new ResponseFilter
    {
        AveragingSeconds = 20, HysteresisC = 3, StepDownHoldSeconds = 10,
        SlewUpPercentPerSec = 8, SlewDownPercentPerSec = 8, ZeroSnapPercent = 0,
    },
    quietCpu,
    i => i switch
    {
        < 200 => 63,                           // settled in the 40% band
        < 260 => 52,                           // dropping: hold in progress…
        _ => 63,
    },
    applyNow: i => i == 100 || i == 230);      // once settled, once mid-hold

// ---- stop probe ------------------------------------------------------------

{
    var probe = new StopProbe
    {
        RunSeconds = 30, ProbeSeconds = 30, StableRangeC = 3.5,
        FailRetrySeconds = 60, MaxTempC = 78,
    };
    var rows = new List<string>();
    int i = 0;
    foreach (var now in Timeline(1200))
    {
        // Stable → trial → heat rise kills it (failed) → stable again (backoff
        // holds, then a trial that survives) → demand rise resumes → demand=0
        // stretch → hot stretch above MaxTempC → cool finish.
        double t = i switch
        {
            < 120 => 55 + (i % 4) * 0.2,
            < 140 => 55 + (i - 120) * 0.4,     // +8° while (probably) holding
            < 600 => 55 + (i % 4) * 0.2,
            < 700 => 80,                        // too hot for trials
            _ => 55 + (i % 4) * 0.2,
        };
        double demand = i switch
        {
            < 500 => 30,
            < 520 => 40,                        // curve steps up past demand-at-stop
            < 560 => 0,                         // curve stops wanting the fan at all
            _ => 30,
        };
        double output = probe.Step(now, t, demand);
        rows.Add(string.Join(',', R(now), R(t), R(demand), R(output), B(probe.Holding)));
        i++;
    }
    WriteCsv("probe.csv", "now,temp,demand,out,holding", rows);
}

// ---- idle kick -------------------------------------------------------------

{
    var kick = new IdleKick { StoppedSeconds = 10, KickPercent = 5, KickSeconds = 5 };
    var rows = new List<string>();
    int i = 0;
    foreach (var now in Timeline(300))
    {
        double demand = i switch
        {
            < 40 => 0,
            < 45 => 30,                         // cancels a kick mid-flight
            _ => 0,
        };
        double output = kick.Step(now, demand);
        rows.Add(string.Join(',', R(now), R(demand), R(output), B(kick.Kicking)));
        i++;
    }
    WriteCsv("kick.csv", "now,demand,out,kicking", rows);
}

// ---- raw staircase ---------------------------------------------------------

{
    var quiet = new FanCurve(quietCpu);
    var perf = new FanCurve(perfCpu);
    var single = new FanCurve(new[] { new CurvePoint(60, 33) });
    var empty = new FanCurve(Array.Empty<CurvePoint>());
    var rows = new List<string>();
    for (double t = -5; t <= 105; t += 0.5)
        rows.Add(string.Join(',', R(t), R(quiet.Evaluate(t)), R(perf.Evaluate(t)),
            R(single.Evaluate(t)), R(empty.Evaluate(t))));
    WriteCsv("curve-eval.csv", "temp,quiet,perf,single,empty", rows);
}

Console.WriteLine("done");
