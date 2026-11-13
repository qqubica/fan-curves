Read-only IPC commands must answer from the published view, never from the engine lock — a tick holds that lock for most of a second.

On this board a tick holds `Shared::engine` through every header write, and each
NCT6686D write has two 50 ms EC handshake waits: six motherboard headers make
about 600 ms per tick. When `ping`, `status`, `profile` and `inventory` locked
the engine, each one waited for the tick to finish (600–850 ms measured on
2026-10-24). A UI connect issues five of them in sequence, so the window stayed
empty for well over a second.

`inventory` also ran `schtasks` twice and `powershell.exe` once (~800 ms) while
holding the engine lock, on every one of the UI's once-a-second polls. That
delayed the fan tick loop for as long as the UI was open.

Rules now in `fan-daemon/src/ipc.rs`:

- `Shared::publish(&engine)` refreshes `Shared::view` after every tick and
  after every command that changes the engine. Read-only commands lock only the
  view. Lock order is engine, then view.
- Start-with-Windows state is cached (`AutostartCache`). It is seeded by the
  startup check, re-checked on a background thread when older than 30 s, and
  re-checked immediately after `set_autostart`.
- The WPF-task conflict check uses `schtasks /Query /XML` (not localized,
  ~40 ms) instead of PowerShell's `Get-ScheduledTask`. Only `<Settings>` holds
  the task-level `<Enabled>false</Enabled>`; triggers carry their own.
- The UI paints status and inventory before a large history backfill. The
  backfill replays up to ~24 h per channel and can take a second.

Measuring: time `fan-daemon --send '{"cmd":"status"}'` from Bash. PowerShell
5.1 strips the JSON quotes. About 40–50 ms of that is process start.
