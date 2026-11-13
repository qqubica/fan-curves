The full-speed developer toggle belongs in daemon engine state so every discovered fan control can be driven at 100% without changing saved curves.

The override must cover unassigned controls and work even when the curve controller is paused or a channel has no valid temperature sensor. When disabled, release the controls first; the next engine tick resumes curves only if they were applying before the override. Pause and shutdown cancel the override and hand all controls back to firmware or the GPU driver. Report the override in daemon status so a newly opened UI shows its actual state.

The UI button must send a toggle intent, which the daemon applies while holding the engine lock. Deriving an explicit `enabled` value from the last polled UI status loses rapid clicks: two clicks can both send `true` before the first reply arrives. Keep the explicit set command for clients that need idempotent control.

Show a button click immediately by projecting the daemon's last confirmed state through the parity of unacknowledged clicks. Reconcile each click with the daemon's toggle reply, and let the next status poll refresh the rest of the UI. Waiting for status plus inventory before changing the button makes it appear unresponsive while hardware writes and RPM reads run. Do not run a full status/inventory poll between queued toggle clicks.
