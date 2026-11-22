# Bridge Studio — one painting window on the software bridge

Date: 2026-11-22 (revised 2026-11-22: carrier pivot)
Branch: `dev-bridge-studio`
Status: proposed (revision pending user approval)
Builds on: `docs/superpowers/specs/2026-11-22-software-bridge-design.md`
(spike report: `docs/superpowers/reports/2026-11-22-software-bridge-spike-report.md`,
branch `dev-software-bridge`)

> **Revision note.** The first revision of this document specified a
> self-built shell + painting canvas (Tauri). The user pivoted: **we do
> not build a GUI at all.** The one studio window is an existing
> open-source painting application — **Krita** — driven by our adapter
> plugin. The pressure spike (`studio/spikes/pressure/RESULTS.md`,
> commit `c21163a`) was run against the superseded stack and is kept as
> history; its remaining question (stylus pressure) is re-posed against
> Krita as a new gate. This revision supersedes the shell/canvas design;
> the old text remains in git history.

## Intent and why

The floor-loop spike proved that pictures can move between
uncooperative applications with accountability. This milestone turns the
bridge into the tool the original vision described: **the user faces
exactly one studio window**, and everything else — CAD import, model
rendering, decomposition, transport, orchestration — runs as containers
and headless programs.

The studio window is **Krita** (stock, version-pinned, never forked or
patched). Our product surface inside it is a Python plugin using
Krita's official scripting API: a "Bridge" docker panel plus the
adapter that moves layers in and out. The vision explicitly allows
plugin injection and temporary code; a plugin on a pinned host is the
disciplined form of that allowance.

The flow has two phases:

- **Adjustment (调整期):** the user orbits the model in their own
  modeler (a Blender instance holding the imported scene). A thin
  plugin in that modeler streams the camera pose to our orchestrator.
  The studio window is not involved.
- **Locked (锁定期):** the user presses lock in the Bridge docker. The
  last streamed pose freezes, the headless pipeline renders the model
  decomposed into per-part layers, and the adapter pushes them into the
  Krita document as a protected bottom group. The user paints with
  Krita's native brush engine. Unlocking (same panel) returns to
  adjustment.

The user interacts with the studio window on an **iPad with Apple
Pencil via Sidecar**. Krita handles tablet input natively; our job is
to verify that path, not to implement it.

## Goals

- One visible studio window: stock Krita plus our plugin. We build no
  window, canvas, or viewport of our own; the user's own browser is
  never touched.
- Painting capability entirely from Krita's native engine: brushes,
  pressure/tilt/rotation, eraser, layer stack, undo — none of this is
  our code.
- The Bridge docker (our only UI): lock/unlock, pose history (browse,
  name, delete, recall), pipeline status.
- Per-pose user-layer stacks inside the document: each pose gets its
  own group of user layers above the model's protected part-layers;
  returning to a pose restores the layers made there.
- Adjustment phase: the user orbits in their own Blender holding the
  imported scene; the camera pose streams to the orchestrator
  continuously; zero round-trips per gesture on our side.
- Headless pipeline in containers: watch a folder for CAD exports
  (STEP/OBJ), convert into a Blender scene, and on lock render per-part
  layers offscreen at the frozen pose.
- The spine's discipline is preserved: boundaries are explicit, every
  crossing is ledger-recorded (what moved, bytes, latency), failures
  are data.

## Non-goals

- Any GUI of our own: no shell, no canvas, no viewport, no second
  window. (Revised: the previous revision made the GUI our own governed
  product; the pivot removes it entirely.)
- Modeling inside the studio flow. The user models in their own
  CAD/modeler; we are not a DCC. Models enter only as exported files.
- Live drag-follow sync (60 Hz). Sync happens on lock, not during the
  drag.
- Edit-back from paint into CAD/modeler. Paint is terminal output.
- Cross-platform. macOS only.
- PSD export; text/vector tool additions; any modification of Krita
  itself (no fork, no patch, stock plugins only).

## Anti-goals

- Building our own GUI stack "temporarily" — the spike answer is in;
  carrier it is.
- A second visible window beyond Krita and the user's own modeler, or
  any use of the user's own browser profile.
- Ambient shared state between components: default isolation, explicit
  flow, as in the bridge spec.
- Any part of the adapter, pipeline, or orchestrator living outside
  product governance (no lab rules, no untracked scratch code).

## Constraints

- macOS with Retina displays: rendering and capture must be
  native-pixel correct (2×), as verified in the spike.
- Pressure input is Krita's native tablet handling with Apple Pencil
  over iPad Sidecar. This is the stack's main risk and is gate-tested
  on the reference machine before the pipeline is built (see Risks).
- Krita is a pinned external dependency (reference machine: 5.3.4).
  The adapter uses only the official Python API (`krita` module,
  LibKis). If a needed capability is missing from the API, the answer
  is a design change on our side, not a Krita patch.
- The spine framework layer keeps its zero-domain-knowledge rule; the
  adapter and pipeline are product surfaces above adapters, not part of
  the spine.
- Every artifact crossing remains ledger-recorded; the ledger is the
  audit trail and the performance instrument.
- Containers make the pipeline reproducible and CI-runnable. The
  non-containerized components are exactly: Krita (host app), our
  plugin inside it, the user's Blender + pose plugin, and the local
  orchestrator process.
- **Complexity envelope: small assemblies, ≤50 parts** (user's answer —
  matches the vision's "draw a part" framing). Per-part layer rendering
  and the ~2 s lock render are engineered for this envelope; larger
  assemblies degrade gracefully (flattened fallback) but are not
  designed for.

## Approaches

### Painting carrier (decided)

- **Krita — CHOSEN.** Professional brush engine with first-class
  pressure/tilt/rotation and a mature Sidecar path; official Python
  scripting API gives a real adapter (document and layer CRUD, pixel
  read/write, native `.kra` save) rather than DOM hacks; `.kra` (zip +
  layers + metadata) maps near-directly onto the pose-keyed archive
  semantics. Accepted costs: a heavy external dependency; the adapter
  lives inside the Krita process and outside the container story.
- **Self-hosted Kleki (web).** The bridge spike already validated the
  automation path (~1.2 s push) and owning the source would dissolve
  the governance problem. Rejected: basic brush engine, weak layers, no
  project file format, and — decisively — the web-on-Sidecar pressure
  path is unverified, a viability risk against a hard requirement.
- **Self-built shell + canvas (previous revision's choice).** Rejected
  by the carrier pivot: full ownership in exchange for building a
  painting application ourselves. The user chose not to build a GUI.

### Adjustment carrier (decided)

- **User's own Blender + thin pose plugin — CHOSEN.** The import
  pipeline already targets Blender scenes; the user opens the generated
  scene in their own Blender to orbit, and a small plugin streams the
  camera pose to the orchestrator. No viewport to build; the modeler of
  record for the camera is the user's own tool.
- Our own embedded viewport (previous revision): removed with the
  pivot.
- Streamed headless renders per gesture: every gesture pays a container
  round-trip — against the "painless" intent. Rejected.

## Architecture

```
 user's CAD ──exports──> watch folder ──> import container
                                              │ (STEP/OBJ → scene)
                                              ▼
                              scene package (.blend + parts manifest)
                                              │
 user's Blender (adjustment) ◄──opens scene───┘
   └─ pose plugin ──camera pose stream──► orchestrator (headless, ledger)
                                              │ lock at frozen pose
                                              ▼
                       headless Blender container: per-part layer render
                                              │
                                              ▼
┌──────────── Krita (the one studio window) ───────────────────────┐
│  Bridge adapter plugin                                            │
│  ├─ Bridge docker: lock/unlock · pose history · pipeline status  │
│  └─ document: [user layer groups per pose] over [model group]    │
└───────────────────────────────────────────────────────────────────┘
```

- **Phase machine.** Two states. Adjustment → Locked on an explicit
  lock action in the Bridge docker; Locked → Adjustment on unlock from
  the same panel. While locked, the import pipeline is frozen at the
  current scene snapshot — the model never drifts under the brush.
  Unlocking re-syncs to whatever the watch folder has since delivered.
- **Pose-keyed archive, simplified by the carrier.** The Krita document
  *is* the archive: strokes live on layers, layers live in per-pose
  groups, `.kra` saves them natively. No stroke-event capture is needed
  (Krita's API offers no per-stroke hooks, and none are required). Each
  pose group carries its exact camera pose (from the renderer, not a
  perceptual hash) in document metadata or a sidecar JSON — placement
  is the builder's call.
- **Orchestrator.** A headless local process owns the phase machine,
  the ledger, and the pipeline's trigger points. The Krita plugin and
  the Blender pose plugin are thin clients of it (IPC mechanism is the
  builder's call). The spine pattern — explicit boundaries, recorded
  crossings, zero domain knowledge in the framework layer — is
  re-implemented here, not imported from the An runtime: the An core is
  an MCP bridge for agents, and coupling this product to it would weld
  two products together.
- **Pose history.** Exact camera poses can never be reproduced by
  hand-orbiting, so "returning to a pose" is a UI feature in the Bridge
  docker: each scene snapshot keeps a browsable pose history
  (thumbnails, nameable, deletable). Recalling a pose restores its
  layer group when the scene snapshot is unchanged.
- **Archive scoping.** Archives are scoped to a scene snapshot; when a
  re-import changes the model, the outgoing snapshot's session is
  auto-exported (flattened PNG per pose) before switching, so no user
  work is ever silently discarded, and the new snapshot starts with a
  clean pose history.
- **Part-layers are protected.** The decomposed model layers form a
  bottom group that the adapter keeps flagged locked; the adapter never
  treats marks there as user work. Honesty note: Krita cannot
  hard-prevent a user from unlocking and editing that group by hand —
  protection is adapter-managed convention plus a restore-on-save
  guard, documented as such, not a security boundary.
- **Document resolution.** The Krita document is created at lock time
  at the render resolution (from the render configuration / modeler
  viewport), fixed for the locked session. Zoom and window size are
  display-only; PNG export is always honest native resolution.
- **Governance.** The adapter plugin, the Blender pose plugin, the
  orchestrator, and the pipeline containers all live in the product
  repository on `dev-bridge-studio`, with the full
  spec/plan/test/review cycle. Krita and Blender are pinned external
  dependencies, never forked.

## Risks

- **Pressure over Sidecar** (main gate): Apple Pencil pressure through
  iPad Sidecar into Krita's tablet handling must be verified on the
  reference machine (draw test + Krita's tablet event log) before
  pipeline work begins. If it fails, the fallback is a native input
  path decision (e.g., direct USB/Bluetooth stylus or a different
  carrier), which reopens this section — the gate exists to fail early.
- **Krita API surface**: document/layer CRUD, pixel transfer, and
  docker UI are documented and sufficient on paper; the thin vertical
  slice (create document → push layers → save/reopen) doubles as the
  API validation. Headless/batch operation for CI is limited in Krita;
  the adapter's logic is therefore kept in a testable core separate
  from the thin Krita-facing shim.
- **Blender pose plugin**: Blender's Python API shifts across versions;
  Blender is pinned like Krita, and the plugin is deliberately tiny
  (read camera, send pose).
- **Container cold-start latency** at lock time: the per-part render
  must land within the original spec's ~2 s target on the reference
  machine; warm-container strategy is the builder's problem, and the
  ledger records the real numbers.

## Left to the builder

- Orchestrator runtime/language and the IPC mechanism between it and
  the two plugins.
- `.kra` layout details: group naming contract (per-part naming →
  layer naming), metadata placement (native annotations vs sidecar).
- Container choice and orchestration (runtime, warm pool, folder watch
  mechanics).
- STEP/OBJ → Blender import fidelity and scene package format.
- CI layout: pipeline tests (containerized fixtures) and adapter-core
  tests (pure logic, no Krita); how far the Krita-facing shim can be
  tested without a display.
- Render resolution defaults and lock-time document sizing.
- Naming/branding of the docker; "Bridge Studio" is a working title and
  the executor's call.
