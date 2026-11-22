# Native macOS interface

This component continues the approved neutral, Activity Monitor-style SwiftUI
comparison: system typography and colors, a receiver sidebar, sortable native
table, toolbar search and an optional details inspector. Selection uses the
system accent. No custom display font, decorative cards or brand palette is
introduced. Light/dark appearance follows macOS.

The main window opens at 1280 × 820 points, minimum 1020 × 600. The receiver
sidebar is 210–320 points and the inspector 260–420 points. Long names remain
available in the inspector; original records use selectable monospaced text.
System controls supply focus, disabled, hover and keyboard behavior.

The first integrated slice opens empty, with an explicit action to open a
recording or example. The Devices sheet separates passive candidate inventory
from recorded receivers. No live status, scan controls or signal estimates are
invented. Details retain the backend's complete evidence object.

The earlier standalone comparison was approved interactively. This integrated
revision has compiler/model/lifecycle/bundle validation; rendered light/dark,
keyboard and VoiceOver acceptance remain pending. No screenshot or visual-review
approval is claimed for the unlaunched integrated app.

## Capture integration

The integration extends the native system-control style with Capture and
Observations tabs. Capture uses a split view: connection/receiver configuration
on the left, active lifecycle, receiver health and saved recordings on the
right. Receiver setup uses standard text fields, file pickers and disclosures;
raw JSON remains available for advanced configurations. Observations retains
the existing table and inspector, adding bounded page/follow controls.

Capture belongs to the Rust service. The interface distinguishes requested run
state, observed receiver health, and unavailable counters. Closing the app does
not imply stopping capture. No visual acceptance is claimed until a rendered
inspection and actual close/reopen check have been completed.

Service connection occupies a compact bottom status bar shared by both tabs.
Its text and green/red indicator report connected, disconnected or error state;
color is not the only signal. Clicking the status opens a native popover with
backend selection, socket configuration, connect/start and disconnect controls.
The receiver setup area does not repeat those controls.
