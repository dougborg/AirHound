# M5StickC Plus2 controls

The no_std `m5stickc` build supports two local controls:

| Button | Action |
| --- | --- |
| A (front, below the screen) | Switch Overview / Details |
| B (side) | Mute / enable detection sound |

The footer names the next action for each button. Sound state appears above it
and stays synchronized with companion buzzer commands. Settings reset on reboot.
Changing pages or sound leaves scanning and accumulated counts running.

Overview shows AP, other WiFi and BLE address counts plus signature hits and the
last matching label. Details shows free internal RAM, free PSRAM, WiFi packets
and queue drops. Both retain scan state, BLE client count and uptime. Counts are
since boot; signature labels are heuristic matches, not authenticated identity.

Buttons are polled every 20 ms with a 40 ms stable debounce interval. Each
stable press triggers once; a held button never auto-repeats. A button held at
boot must be released before it acts. The screen refreshes every 500 ms. Button
handling is a separate task, so display initialization failure does not disable
sound control. The power/wake button is not assigned an application action.

Pin assignments follow the [manufacturer documentation](https://docs.m5stack.com/en/core/StickC-Plus2):
A GPIO37, B GPIO39. These input-only pins use the board's external pull-ups.
No framebuffer or additional heap allocation is introduced.

Validation: host tests cover contact bounce, long holds, boot-held buttons,
short pulses and simultaneous buttons. Firmware compilation and the startup
stack gate check resource constraints. Physical press and screen appearance
validation require the connected device; host tests cannot establish these.

On 2026-11-23, the user confirmed both buttons working and a quiet buzzer during
the ten-minute observation of installed `96b19ae` / `0.2.0`, then confirmed both
buttons still working after the reader closed without reset or unplug. This is
bounded physical-control evidence, not proof of audible detection alerts or
current host capture lifecycle. The observation's strict acceptance still failed
for missing health reports, reported output loss and varying internal-memory
samples. Earlier post-close button failures remain preserved in the
[hardware ledger](attached-hardware-validation.md#current-m5-uart-correction--2026-11-23-utc)
and [#175](https://github.com/dougborg/AirHound/issues/175).
