A fault inside a driver library kills the daemon without unwinding, so the launcher must restart it — nothing else will notice.

On 2026-11-13 at 09:42 the installed daemon died with an access violation
(0xC0000005) inside `nvml.dll`, six minutes after the machine came back from an
NVIDIA driver blue screen (bugcheck 0x116, `nvlddmkm.sys`). The Rust panic
handler never ran: an access violation in foreign code is not a panic, so
`FanEngine::drop` did not hand the headers back. The Super I/O kept the last
written duties (CPU 40 %, case 20 %) and no process managed the fans for twelve
minutes, until the crash was noticed by chance and the task was run by hand.
The task's "Last Result" was -1073741819 and the behavior log simply stopped,
with no "daemon exiting" line.

What exists now:

- `windows/start-daemon.vbs` runs the daemon in a loop. Exit code 0 (shutdown
  over IPC), 1 (refused to start: no hardware, socket busy) and 2 (bad
  arguments) end the loop; any other code is a crash and the daemon is
  started again after two seconds. More than five crashes, each within a
  minute of its start, end the loop so a daemon that cannot run does not spin
  forever.
- The launcher is a file in the install directory, so an unelevated
  `windows/install.ps1` update can replace it. Putting the restart into the
  task's own settings would need elevation to deploy.

What is still true:

- During the two seconds before the restart, and for good after five quick
  crashes, the headers stay at the last written duty. Only a handler that runs
  inside the dying process could release them, and none exists.
- After a crash restart, the "firmware state" the motherboard backend saves on
  its first write is the crashed process's last duty, not the BIOS's. Pause or
  a clean exit restores that value until the next reboot.
- When diagnosing "the fans stopped reacting": check that `fan-daemon` is
  running and answers `fan-daemon --send ping`, then the Application event log
  (Application Error 1000 names the faulting module).
