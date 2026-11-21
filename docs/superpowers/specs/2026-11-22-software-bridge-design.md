# Universal software-collaboration bridge — design

Date: 2026-11-22
Branch: `dev-software-bridge`
Status: proposed

## Intent and why

Let domain professionals become illustrators of their own work without learning new tools or typing anything. A person who can already drive CAD, a modeling tool, and a painting tool should be able to chain them: draw the part in CAD, see the gray model in the modeler, and paint in the painting app over a canvas that follows the modeler's viewpoint — already decomposed into per-part layers, ready to edit.

The bridge is the invisible colleague in the middle. The user's existing motions are the entire interface: what you draw decides what gets drawn, where you look decides what gets rendered. There is no prompt box, no config file, no command.

This is the flagship instance of a general question: how does context move between cooperating applications? The answer this design commits to: **default isolation, explicit flow.** Applications do not share ambient state; the bridge moves typed artifacts across named boundaries, and every crossing is logged with what moved, how many bytes, and how long it took.

## Goals

- A universal floor: any two GUI applications can be joined using only operating-system primitives (window observation, input simulation, file/clipboard transport). No application needs an API to enter the chain.
- An adapter registry on top of the floor: per-application plugins that supply exact camera pose, per-part renders, and native layer insertion where the application allows it. First-class targets: Blender (modeler) and a painting application.
- Settle-to-update sync: when the modeler's viewpoint changes and settles, the painter's canvas updates to the correct decomposed view shortly after — target under ~2 seconds on the reference machine.
- A context ledger: every artifact that crosses an application boundary is recorded (source, destination, type, bytes, timestamp, latency). The ledger is the audit trail and the performance instrument.
- Zero user-facing text: no prompts, no dialogs, no typed configuration. Setup is fixed presets inferred from what is on screen.

## Non-goals

- Live drag-follow rendering (60 Hz). Sync is settle-to-update only.
- A canonical scene runtime / IR broker (approach 3, deferred indefinitely).
- Improving CAD→modeler geometry import; existing import flows (STEP/OBJ) are accepted as-is.
- Cross-platform support in v1; macOS only.
- Edit-back from the painting app into the modeler or CAD in v1 (my call — paint is terminal output; see below).

## Anti-goals

- No ambient state sharing between applications; every crossing is explicit and logged.
- No silent mock fallback: if an application cannot be observed or adapted, the bridge says so instead of pretending.
- No user-facing modal text interaction at any point in the loop.
- No per-application plugins as a precondition for the chain to work at all; the observation floor must stand alone.

## Constraints

- macOS, driven through KimiCU (window observation, targeted input) for the universal spine; per-app adapters may use in-process APIs where available (Blender Python, Photoshop UXP/AppleScript, browser injection for web apps).
- Reference machine: Apple Silicon, macOS 27.x.
- Latency budget: viewpoint settle → correct canvas update in under ~2 s on the reference machine; faster is welcome, drag-follow is explicitly not required.
- Code discipline per dev-lines: the first working bridge is a spike in the gitignored `lab/` package (or the `an-benchmark`-style dedicated repo if it grows its own identity); promotion to the product is re-implementation to product standard, not a merge of the spike.
- The bridge must be honest about coverage: adapters raise the ceiling; the observation floor defines what is guaranteed.

## Approaches

### Approach 1 — pure observation bridge

A background process watches the modeler's window (screenshot diffing), detects "viewpoint moved, then stopped", triggers that application's own export of the current view via accessibility scripting or simulated input, and pushes the result into the painting app as layers through its normal import path.

Trade-off: works everywhere immediately, but per-part decomposition is impossible through observation alone — the painter receives a flattened render, at best coarsely segmented. The flagship promise ("decomposed, edit-ready layers") degrades.

### Approach 2 — observation spine + per-application adapters (selected)

The observation spine owns the universal parts: settle detection, cross-application transport, the context ledger. Adapters plug in per application and override what they can: real camera pose, per-part view-layer renders, native layer insertion. An application with no adapter still works at floor fidelity; one with an adapter delivers the full experience. New applications enter the chain at the floor and get adapters when they earn one.

Trade-off: two fidelity tiers to document honestly, and an adapter protocol to design — but universality and the flagship experience stop being mutually exclusive, and the spine is spikable within days using KimiCU against real desktop apps. **Selected.**

### Approach 3 — intermediate representation broker

The bridge owns a canonical scene (geometry + camera + part map); modeler and painter become interchangeable clients of it. Best long-term fidelity and true participant interchangeability, but it means building a mini scene runtime first, and off-the-shelf applications do not act as clients of someone else's scene — the user's modeling tool would become a shell over our runtime, which fights the "no new knowledge" premise.

Trade-off: rejected for v1; revisited only if approach 2's tiering proves unmanageable.

## The how, as decided

Decisions the user made:

- **Settle-to-update**, not live drag-follow: the canvas updates to the correct view after the viewpoint settles.
- **Plugins, injection, and throwaway code are allowed** in pursuit of the flagship experience; the observation floor remains the universal guarantee.
- **Universal over perfect**: no application is assumed to expose an API.
- **No text, no new knowledge**: the user's existing motions in their existing tools are the whole interface.

- **Pose-keyed paint archives.** When the user has painted in the painting app and the viewpoint then changes, the bridge archives their strokes keyed by camera pose and restores them when that viewpoint returns. The painter's work is never overwritten by a view update, and painting is per-viewpoint: each viewpoint owns its own strokes. (User decision.) At floor fidelity there is no adapter-supplied pose, so the pose key degrades to a perceptual hash of the captured render; archives there are approximate.
- **Paint never flows back.** Nothing painted in the painting app propagates into the modeler or CAD; paint is terminal relative to upstream applications. (Designer call — back-propagation of paint into geometry is a different product.)

Decisions the designer made (mine, marked for review):

- **Milestone order: spike the floor loop first, then the Blender adapter.** The spike validates watch→settle→capture→push→ledger with a single flattened layer; per-part decomposition and pose capture arrive with the adapter milestone. "Part" defaults to top-level objects in the modeler's scene.
- **Canvas mapping: letterbox scale-to-fit** into the painting app's existing canvas at 2x capture resolution; the bridge never resizes or recreates the user's project.
- **Push deferral**: the spine watches the painting app too; a pending push waits until the painter has been idle (~2 s, builder-tuned) so an update never lands mid-brushstroke.
- **Bridge-owned browser window for Klecks**: the bridge opens a dedicated browser window with a debugging port and loads Klecks there; the user's own browser and files are never touched, and no setup is required of them.
- **Silent snapshots for durability**: the bridge saves the current canvas to its spool directory on every push and whenever the painter goes idle, so the terminal image survives a tab crash without the user exporting anything.

## Architecture sketch (builder details open)

```text
modeler window ──watch──> settle detector ──trigger──> view capture (pose + per-part
                                                            │   via adapter; render only
                              context ledger <──────────────┤   at floor)
                                     │                      v
paint app window <──push layers── transport <── layers ─── paint archive
        │                                                    ▲   │
        └──watch (idle deferral, snapshot)───────────────────┘  keyed by pose
```

- **Watcher / settle detector**: screenshot-diff the modeler window; a change followed by a dwell (~300–500 ms, builder-tuned) means "settled".
- **Trigger**: ask the modeler for the current view — adapter if present (Blender: Python sidecar socket), else accessibility menu scripting / simulated keystroke for the app's own render/export.
- **Capture**: adapter returns camera pose + per-part renders; floor path returns the app's own export, one layer.
- **Transport**: write images to a spool directory, push into the painting app through its native import (adapter: native layer insertion; floor: open/paste). Every hop is recorded in the **context ledger** (source, destination, artifact type, bytes, timestamp, latency).
- **Paint archive**: a pose-keyed store owned by the bridge. On a settled view change it saves the painter's current strokes under the previous pose key before pushing the new render; on returning to a known pose it restores the matching strokes onto the new render. With an adapter the pose key is the exact camera pose; at floor fidelity it is a perceptual hash of the captured render, and restoration is approximate.
- **Adapter protocol** (capability set, each optional): `detect_settle`, `camera_pose`, `render_layers`, `push_layers`, `read_canvas`, `restore_canvas`. The spine supplies floor implementations for `detect_settle` and `push_layers`; pose-keyed paint requires `read_canvas`/`restore_canvas` (Klecks adapter) and a pose source (`camera_pose`, else hash fallback).

## Spike plan

Minimal loop to validate: watch Blender → detect settle → capture current view → appear as a layer in Klecks → repeat on the next settle, with the user typing nothing. Success: the canvas shows the correct current view after each settle, layers update rather than pile up, and the ledger records each crossing.

## What is left to the builder

- Adapter protocol signatures and the registry's discovery mechanism.
- Settle-detection parameters and how viewport regions are isolated from UI chrome.
- Layer naming/identity so repeated settles update in place.
- Klecks canvas read/write mechanics (`read_canvas`, `restore_canvas`, layer insertion); Blender sidecar details.
- Pose-matching tolerance for the floor tier's hash-based archive keys, and archive storage layout in the spool directory.
- Failure surfaces (window occluded, app busy, import rejected) and how the bridge says so without text to the user (status in the ledger; the painting app simply does not update, plus a last-resort visible indicator).
