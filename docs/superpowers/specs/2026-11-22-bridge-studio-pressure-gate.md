# Bridge Studio — phase gate report: pressure input

Date: 2026-11-22
Milestone: Bridge Studio (design: `2026-11-22-bridge-studio-design.md`, approved)
Gate: pressure-capable pen input must reach the painting carrier before
any pipeline build starts (design doc, Risks).

## Status

**Gate FAIL on the plain-Sidecar route. Recovery route selected:
driver-level injector (Codonic Pad). Retest pending.**

Pipeline work remains blocked on this gate. Nothing in this document
changes the approved design; the design revision it implies
(Sidecar → injector wording, dependency list) is deferred until the
retest passes and is subject to re-approval.

## What was measured

Full evidence: `studio/spikes/sidecar-pressure/RESULTS.md`
(protocol: `PROTOCOL.md` in the same directory). Summary:

1. **Krita Tablet Tester** (three strokes: light / heavy / ramping,
   Apple Pencil over Sidecar): ~466 events, all plain mouse events.
   Zero tablet events, zero `P=` (pressure) fields, red grid lines
   only.
2. **Native AppKit probe** (`probe/` in the spike directory): macOS
   itself delivers the Sidecar Pencil to ordinary apps as a plain
   mouse — every event `subtype=raw(0)`, pressure pinned at 1.0
   (down/drag) / 0.0 (up); the `tabletPoint` / `tabletProximity`
   overrides never fired. The loss is therefore a property of the
   input route, not a Qt bug: any carrier reading standard NSEvent
   input receives the same nothing.
3. **Route A (PencilKit thin shell): platform-dead.** `PKCanvasView`
   does not exist in the macOS SDK (verified at swiftinterface
   source level). A Catalyst detour would receive the same measured
   pressureless mouse delivery and was rejected without a build.

## Decision

**Route B — driver-level injector.** Selected product: **Codonic Pad**
(PadMac host + PadPad iPad client). Rationale:

- Its mechanism is exactly the measured gap: the host streams a chosen
  window to the tablet; the client returns stylus input as real tablet
  events (pressure, tilt, hover, eraser) with no kernel driver.
- The vendor's own Krita guide prescribes the same Tablet-Tester
  verification this gate uses, so pass/fail stays comparable.
- Transport is direct local network or USB-C; the vendor states canvas
  and pen traffic is not relayed through its cloud, and its privacy
  policy states no personal data is collected.

Recorded in the milestone ledger as a Ruling. Reserve ladder if the
retest fails: permission/restart retry → Astropad Slate (same
protocol) → hardware tablet.

## Preconditions (as configuration, not narrative)

| Slot | Requirement |
| --- | --- |
| Host | Apple-silicon Mac, macOS 15 or later |
| Carrier | Krita 5.3.4+ (`/Applications/krita.app`) |
| Input | iPad + Apple Pencil, PadPad client installed |
| Injector host | PadMac (v1.3+), Screen Recording **and** Accessibility permissions granted, host relaunched after granting |
| Link | Same trusted LAN, or USB-C direct (lowest latency) |
| Constraint | Sidecar disconnected during the retest — the injector is the single input path; two parallel display/pen routes would confound the measurement |

Exact reference-machine particulars stay in the spike's RESULTS.md;
this document intentionally carries requirements only.

## Retest protocol and pass criteria

Unchanged from `PROTOCOL.md`:

1. Open Krita → Preferences → Tablet settings → Open Tablet Tester.
2. Draw three strokes through the injector path: light, heavy,
   ramping light→firm.
3. **Pass**: blue tablet-event lines appear **and** `P=` spans roughly
   0.2–0.9 between the light and heavy strokes.
4. Capture: tester text export + screenshot, appended to RESULTS.md
   under a new "Route B retest" section, with a ledger entry.

Latency and feel are explicitly **not** gate criteria; observed
latency is recorded as a remark only.

## Failure handling

On FAIL, in order: verify the Accessibility permission (Screen
Recording alone yields picture without input) → quit and relaunch the
host → retest once. Still failing: Astropad Slate under the same
protocol. Still failing: hardware tablet, and the iPad input premise
is dropped from this milestone.

## Consequences of a pass

1. Minor design-doc revision: input route wording Sidecar → injector,
   dependency list gains injector host/client names and versions,
   Risks updated (Sidecar-pressure risk closes; new risk: third-party
   host with elevated entitlements).
2. User re-approval of the design (HARD-GATE).
3. New Plan 1 written against the approved design.

## Defense and privacy notes

- The injector host requires Screen Recording + Accessibility
  entitlements on the reference machine. Both are revocable in System
  Settings and should be revoked if the milestone abandons this route.
- No cloud account is required by the injector; pairing stays on the
  local network (optional pairing code available).
- Evidence artifacts (tester text, screenshots) are cropped to the
  tester window and must not carry desktop content, file names, or
  network identifiers.
