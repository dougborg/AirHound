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

The Devices sheet separates passive candidate inventory from recorded receivers.
Details retain the backend's complete evidence object. Capture operations belong
to the shared Rust service, while SwiftUI presents state and controls.

Dark-mode rendering, native capture controls, observation browsing and actual
window close/reopen have been exercised locally. Full VoiceOver and light-mode
acceptance are not claimed; see ACCEPTANCE.md for test scope.

## Capture integration

The integration extends the native system-control style with Capture and
Observations tabs. Capture uses a split view: connection/receiver configuration
on the left, active lifecycle, receiver health and saved recordings on the
right. Receiver setup uses standard text fields, file pickers and disclosures;
raw JSON remains available for advanced configurations. Observations retains
the existing table and inspector, adding bounded page/follow controls.

Capture belongs to the Rust service. The interface distinguishes requested run
state, observed receiver health, and unavailable counters. Closing the app does
not imply stopping capture. The rendered inspection and actual close/reopen check are recorded in
ACCEPTANCE.md.

Service connection occupies a compact bottom status bar shared by both tabs.
Its text and green/red indicator report connected, disconnected or error state;
color is not the only signal. Clicking the status opens a native popover with
backend selection, socket configuration, connect/start and disconnect controls.
The receiver setup area does not repeat those controls.

## Saved setups and device configuration

The configured receiver list reflects the current plan directly. Each receiver
expands in place for settings and current producer identity; adding or removing
one preserves all other plan fields. A separate Available devices disclosure
keeps unconfigured and unsupported candidates discoverable without presenting
them as active receivers. Support links and evidence remain inside each row.

Saved setups use a native picker, save sheet and load-review sheet. The review
presents backend binding/resource checks before applying the plan. GPSD location
settings are a separate form section because a location provider is not a radio
receiver. These are presentation adapters to the shared Rust client; readiness,
validation, persistence and device ownership remain backend responsibilities.

Capture actions stay in a fixed native control row above both panes, so scrolling
receiver and GPS settings cannot hide Start, Pause/Resume or Finish. Background
status polling does not flash controls disabled or display a recording-operation
spinner. Completed-run health is labeled historical while the live service state
reads Idle; replaying stored health must not imply that a receiver is still running.
The health pane scrolls independently above the recordings list in a native
vertical split, leaving a usable recordings area even with several receivers
and a GPS provider at the minimum window height.
