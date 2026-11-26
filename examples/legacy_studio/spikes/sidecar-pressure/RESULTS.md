# RESULTS — Sidecar pressure gate

Date: 2026-11-22
Machine: reference machine (Apple M5 Max; Sidecar Display 2732×2048, AirPlay)
Krita: 5.3.4 (`/Applications/krita.app`)
Input: iPad + Apple Pencil via Sidecar

## Verdict

**FAIL** — Krita receives no tablet events and no pressure from Apple
Pencil over Sidecar. Per the gate protocol, the carrier decision
reopens pending isolation of the failing layer (OS vs Qt).

## Evidence

### Tablet Tester text output (three strokes: light / heavy / ramping)

Full log captured via the tester's AX text area. ~466 events total,
all of the form:

```
Mouse press X=63 Y=24 B=1 S=0
Mouse move X=62 Y=25 B=1 S=0
…
Mouse release
```

- Zero tablet events (the tester renders those as blue lines and logs
  them with `P=` / `TX,TY=` fields — none appear anywhere).
- The grid showed red lines only (screenshot captured).
- No `P=` (pressure) field on any event; only `B=` (buttons) and
  `S=` (speed).

### Canvas strokes (earlier, same session)

Two strokes on a normal document with "Bristles-2 Flat Rough" showed a
visible width/opacity difference. The Tester evidence supersedes this
observation: whatever produced the canvas difference, the raw event
stream carries no pressure. Recorded here for completeness, not as
counter-evidence.

## Consequence

Two layers remain indistinguishable from inside Krita:

1. macOS delivers pressure (NSEvent pressure / tablet subtypes) and Qt
   fails to translate it → carrier-side mitigations (driver-level
   injector, hardware tablet, or different carrier).
2. macOS/Sidecar does not deliver pressure to ordinary apps at all →
   the Sidecar input route itself is wrong.

A native AppKit probe (`probe/` in this directory) is the decisive
instrument for layer 1 vs 2. Its results are appended below.

## Native probe

Verdict: **OS layer also fails.** The Sidecar input route itself
carries no pressure for ordinary third-party apps on this machine.

Instrument: `probe/probe.swift` (~80 lines, AppKit, compiled with
swiftc -O, Swift 6.4.2, SDK macosx27.2). Single NSWindow/NSView on the
iPad (Sidecar) display logging every event's type, subtype, and
`NSEvent.pressure`, plus `tabletPoint` / `tabletProximity` /
`pressureChange` overrides.

User drew with Apple Pencil on the probe window. Full log:
`probe/probe.log`. 31 events: 2 down, 27 drag, 2 up.

- Every event: `subtype=raw(0)` — plain mouse events. Zero
  tabletPoint/tabletProximity subtypes; the `tabletPoint(with:)` and
  `tabletProximity(with:)` overrides never fired.
- `pressure=1.0000` on every down/drag event, `0.0000` on up — the
  binary signature of a no-pressure device, not a varying stylus
  signal.

Layer isolation conclusion: macOS/Sidecar delivers the Pencil to
ordinary apps as a **plain mouse** (no tablet events, no pressure) on
this OS (macosx27.2 SDK era). Krita's failure is therefore not a Qt
bug to fix but a property of the input route: any carrier reading
standard NSEvent input gets the same nothing.

Implication for the reopened decision (recorded in the ledger):
restoring pressure requires leaving the plain-Sidecar route — options
are a driver-level injector (virtual tablet device), a hardware
tablet, or PencilKit (the only first-party API that receives Sidecar
pencil pressure; using it means a thin native canvas shell).

## Route A probe (PencilKit): platform-dead

Follow-up, same day: the user's choice was to verify the PencilKit
route. `probe/probe_pencilkit.swift` (PKCanvasView + stroke-level
pressure/tilt dump) fails to compile: `PKCanvasView` and
`PKCanvasViewDelegate` do not exist in the macOS SDK. Verified at the
source level — the macOS PencilKit swiftinterface
(`PencilKit.swiftmodule/arm64e-apple-macos.swiftinterface`, SDK
MacOSX27.2) contains zero occurrences of `PKCanvasView`; the macOS
module only carries auxiliary types. Route A (native thin shell with
PKCanvasView on macOS) is dead at the platform level. A Mac Catalyst
detour would receive input through the same mouse-event delivery the
native probe already measured (no pressure) and adds signing/toolchain
governance for a speculative payoff — rejected without a build.

Remaining routes: driver-level injector (virtual tablet), hardware
tablet, iPad-native drawing + file bridge, or pressureless v1.

## Route B retest (Codonic Pad injector): PASS

Same day, route B executed. Injector: PadMac host + PadPad iPad
client (Codonic Pad). Sidecar disconnected for the whole retest
(single input path).

### Timeline and root-cause chain

1. **Retest run 1 — FAIL signature.** Three protocol strokes in the
   Tablet Tester arrived as plain `Mouse` events, red lines, no `P=`.
   Cause isolated to the Mac side: PadMac had been relaunched several
   times after install, and macOS ties TCC permissions to the app
   signature — the Accessibility grant went silently stale, and
   PadMac's injection degraded to plain mouse fallback. (Documented
   PadMac failure mode; the System Settings toggle *looks* on.)
2. **Permission repair.** Removed and re-added both PadMac entries
   (Accessibility + Screen Recording), quit and relaunched PadMac.
3. **OS-layer verification — PASS.** The native probe (same binary as
   the Sidecar probe) received one heavy stroke as full tablet input:
   `subtype=tabletPoint` on down/drag/up, pressure 0.078→0.81→0.0
   (1/256 quantization), real tilt `(0.282,-0.330)`,
   `deviceID=33059`, proximity events. Log: `probe/probe-padmac-os-pass.log`.
   The injector delivers exactly what the gate needs at the OS layer.
4. **Krita still mouse — Qt device cache.** Same injector, same era:
   the Krita Tester still logged `Mouse` events. Krita had been
   running since before PadMac's install/relaunch cycle; Qt's tablet
   device handling did not pick up the re-granted injector.
5. **Krita restart — PASS.** Fresh Krita, fresh Tester, three
   protocol strokes. Full evidence below. Screenshot: `tester-pass.jpg`.

### Tester evidence (post-restart, three strokes)

All three strokes are tablet events: blue lines, each bracketed by
`Pen tip brought near` / `Pen tip taken away`, `TX,TY` tilt live
(23–25 / 25–28°), sub-pixel coordinates.

- **Stroke 1 (light):** `P=` 1.2%–14.5% (mostly 5–11%).
- **Stroke 2 (heavy):** `P=` 7.8%–52.5%, plateau 40–52%.
- **Stroke 3 (ramp):** `P=` 1.2% → 76.1% continuous climb, then
  release through 49.4%.

Pass criterion (blue tablet lines + light/heavy `P=` spanning
~0.2–0.9): met. Observed span 1.2%–76.1% covers the criterion band;
light stroke sits far below 0.2, ramp peaks inside the upper band.

Latency/feel: not measured (explicitly outside gate criteria).

### Operational lessons (carry into design Risks)

- After any PadMac reinstall/permission change: remove and re-add
  both TCC entries, then relaunch. A "looks on" toggle can be stale.
- After any injector/device change: **restart Krita** (Qt tablet
  device cache). Symptom of a stale cache is exactly "other apps get
  tablet events, Krita gets mouse".
- The probe + Tester two-layer protocol isolates injector vs carrier
  in minutes; keep both instruments.

Gate verdict: **PASS — pressure path is Pad injector, not Sidecar.**
Design revision (Sidecar→injector wording, dependency list gains
PadMac/PadPad versions, Risks update) proceeds per the phase-gate
document, then Plan 1.
