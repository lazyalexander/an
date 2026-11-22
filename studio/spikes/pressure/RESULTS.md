# Pressure spike — results

Date: 2026-11-22
Status: superseded by the carrier pivot (no self-built GUI); evidence kept.

## Verdict (for the Tauri stack question it was built for)

GO for real input: inside a Tauri v2 WKWebView, real mouse input
reports `pointerType: "mouse"`, `buttons: 1`, `pressure: 0.5` while
pressed. The webview receives real hardware input correctly.

The stylus-on-Sidecar leg was not exercised: the stack was abandoned
(carrier pivot) before the user completed a stylus run. If a future
carrier is embedded in a webview, this leg must be re-run there.

## Findings (kept for future harnesses)

- **Synthetic OS input cannot exercise button or pressure state inside
  a WKWebView.** CGEvent-based synthetic drags/clicks reach the pointer
  handlers but report `buttons: 0`, `pressure: 0` — verified with the
  KimiCU background route. Any automated test of pointer behavior must
  dispatch `PointerEvent`s inside the page instead.
- **Per-event DOM writes stall rendering.** Logging every pointermove
  to the DOM made the drawn dots lag the pointer; batching log flushes
  (~150 ms) fixed following. Draw immediately, batch DOM work.

## Method

Minimal Tauri app (this directory): full-window canvas logging
`{type, pointerType, pressure, buttons}` per pointer event and drawing
dots with radius `40 * pressure`. Run with `cargo run`.
