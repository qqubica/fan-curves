# fan-curves Rust port

Rust rewrite of the temperature-only engine, targeting a headless daemon +
on-demand UI split across Windows and Linux.

## Layout

- `crates/fan-core` — the engine. Each module mirrors one C# file in
  `src/FanCurves.Core`: `curve` (staircase), `filter` (the MacBook feel:
  rolling average, step-down hold + hysteresis, slew, zero snap), `kick`,
  `probe`, `profile` (serde-compatible with the WPF app's `profile.json`),
  `engine` (tick orchestration), `backend` (hardware trait), `sim`
  (thermal-plant simulation). No dependencies beyond serde. Every component
  takes monotonic `now` seconds as a parameter — the caller owns the clock, so
  tests replay any timeline deterministically.
- `crates/fan-daemon` — headless engine loop: jittered ~1 s tick, profile
  loading/auto-assign, clean handback of every header to the BIOS on exit
  (engine `Drop`) and Ctrl+C. Flags: `--sim`, `--ticks N`, `--profile <path>`,
  `--no-apply`. Only the simulated backend exists so far.
- `parity-harness` — C# console app referencing the real `FanCurves.Core`; it
  generates golden per-tick traces into `crates/fan-core/tests/golden/`.

## Build / test

```
cargo test                                  # unit tests + C# parity replay
cargo run -p fan-daemon -- --sim --ticks 10
```

## Parity contract

`tests/golden.rs` replays the committed golden traces through the port and
compares every output column (tolerance 1e-9; in practice bit-identical — both
sides run the same f64 operations in the same order). After any intentional
behaviour change in `FanCurves.Core`, regenerate the traces:

```
dotnet run --project rust/parity-harness
```

Otherwise the golden tests pin the old behaviour and fail honestly.
