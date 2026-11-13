A CPU fan curve follows its assigned temperature sensor, which can differ substantially from CPU package temperature.

On 2026-09-23 the live CPU cooler channel used CPU CCD1 at about 48°C while
Tctl/Tdie was about 63°C. Its first nonzero curve step was 50°C, so the 0%
command was consistent with the selected sensor. Case fans simultaneously
reported 50% and roughly 800 RPM. Compare the assigned sensor, rolling average,
curve step, and all fan RPMs before diagnosing an unchanging fan as a failed
hardware backend. Preserve an intentional sensor selection unless the user
requests a different cooling policy.
