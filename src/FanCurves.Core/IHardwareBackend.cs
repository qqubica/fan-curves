namespace FanCurves.Core;

public record HwSensor(string Id, string Name, string Kind); // Kind: "temp" | "rpm"
public record HwControl(string Id, string Name);

/// <summary>Abstraction over the machine so the app runs identically in simulation.</summary>
public interface IHardwareBackend : IDisposable
{
    string Description { get; }
    bool IsSimulated { get; }

    IReadOnlyList<HwSensor> Sensors { get; }
    IReadOnlyList<HwControl> Controls { get; }

    /// <summary>Refresh hardware readings; call once per engine tick.</summary>
    void Update();

    double? ReadValue(string sensorId);

    /// <summary>Write a PWM duty cycle 0–100 to a control.</summary>
    void SetControl(string controlId, double percent);

    /// <summary>Give control back to the BIOS/firmware curve.</summary>
    void ReleaseControl(string controlId);

    /// <summary>Fan RPM sensor associated with a control, if the backend knows it.</summary>
    double? ReadControlRpm(string controlId);
}
