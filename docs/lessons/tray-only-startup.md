A registered logon task outlives every update and needs administrator to change, so startup behaviour must be decided by the installed files, not by the task's arguments.

An early tray task passed `--open-when-ready`, which opened the UI at logon.
Dropping the argument from the registration script only fixed future
registrations: the task already on the machine kept passing it, and an
unelevated update cannot edit a task. The fix that reached existing installs
was in the tray binary itself, which now shows the icon and nothing else,
whatever arguments it is given (only `--open` means something: launch the UI
and exit).

The same reasoning put the daemon's crash restart into `start-daemon.vbs`
rather than the task's settings. The tasks only name a file in the install
directory; everything that may need to change later lives in that file.
