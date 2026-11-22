# Bridge Studio — phase gate report: pressure input

Date: 2026-11-22
Milestone: Bridge Studio (design: `2026-11-22-bridge-studio-design.md`, approved)
Gate: pressure-capable pen input must reach the painting carrier before
any pipeline build starts (design doc, Risks).

## Status

**Gate PASS on the injector route (2026-11-22). Pipeline work is
unblocked.**

Retest outcome (evidence: `studio/spikes/sidecar-pressure/RESULTS.md`,
"Route B retest"; screenshot `tester-pass.jpg`; OS-layer log
`probe/probe-padmac-os-pass.log`):

- After TCC re-grant and PadMac relaunch, the native OS probe received
  full `tabletPoint` delivery (pressure 0.078→0.81, real tilt,
  `deviceID` present, proximity events).
- After a Krita restart (Qt tablet device cache — see Failure
  handling), the Krita Tablet Tester showed blue tablet-event lines on
  all three protocol strokes: light `P=` 1.2%–14.5%, heavy
  7.8%–52.5%, ramp 1.2%→76.1%, live tilt 23–28°. The pass criterion
  (blue lines + `P=` spanning ~0.2–0.9) is met with margin.

The design revision this implies (Sidecar → injector wording,
dependency list, Risks) is applied to the design document in the same
change and is subject to user re-approval (HARD-GATE) before Plan 1
is written.

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

On FAIL, in order: remove and re-grant **both** PadMac permissions
(Accessibility + Screen Recording — the System Settings toggle can
show "on" while the TCC binding is silently dead; observed in retest
run 1) → quit and relaunch the host → **restart Krita** (Qt caches
tablet devices at startup; symptom of a stale cache: other apps and
the OS probe show pressure while Krita logs plain mouse) → retest
once. Still failing: Astropad Slate under the same protocol. Still
failing: hardware tablet, and the iPad input premise is dropped from
this milestone.

## Consequences of a pass

1. ~~Minor design-doc revision~~ — **done**: input route wording
   Sidecar → injector, injector dependency constraint added, Risks
   updated (Sidecar-pressure risk closed; three operational risks
   recorded).
2. User re-approval of the design (HARD-GATE) — **pending**.
3. New Plan 1 written against the approved design — after
   re-approval.

## Defense and privacy notes

- The injector host requires Screen Recording + Accessibility
  entitlements on the reference machine. Both are revocable in System
  Settings and should be revoked if the milestone abandons this route.
- No cloud account is required by the injector; pairing stays on the
  local network (optional pairing code available).
- Evidence artifacts (tester text, screenshots) are cropped to the
  tester window and must not carry desktop content, file names, or
  network identifiers.
