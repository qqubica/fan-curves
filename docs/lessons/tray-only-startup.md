Retired startup arguments must stay harmless because existing scheduled tasks can outlive an application update.

The previous tray task passed `--open-when-ready`, which opened the UI on the
first hardware-ready timer tick. Removing the argument from the registration
script fixes future registrations, but existing tasks can require administrator
rights to change. Remove the automatic-open behavior from the tray binary too,
so installing it corrects those existing tasks without re-registering them.
Keep explicit `--open`, tray double-click, and Open Fan Curves as user launch paths.
