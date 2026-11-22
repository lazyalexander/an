# Bridge Studio — single-window painter for the software bridge

Date: 2026-11-22
Branch: `dev-bridge-studio`
Status: proposed
Builds on: `docs/superpowers/specs/2026-11-22-software-bridge-design.md`
(spike report: `docs/superpowers/reports/2026-11-22-software-bridge-spike-report.md`,
branch `dev-software-bridge`)

## Intent and why

The floor-loop spike proved that pictures can move between
uncooperative applications with accountability. Its shape, however, was
two visible windows and a screenshot pipeline — a demonstration, not a
tool. This milestone turns the bridge into the tool the original vision
described: **the user faces exactly one window**, and everything else —
CAD import, model rendering, decomposition, transport — runs as
containers and headless programs.

The window has two phases:

- **Adjustment (调整期):** the window is a gray-model viewport. The
  user orbits the imported model with gestures until the view is right.
- **Locked (锁定期):** the view freezes, the headless pipeline renders
  the model decomposed into per-part layers, and the same window
  becomes a painting canvas. The user paints. Unlocking returns to
  adjustment.

The GUI is built by us and is a **governed product**: spec, plan,
tests, and review apply to it exactly as to the rest of the repository.
It is not lab code, and the lab rules (gitignored, ledger-only) do not
apply to it.

## Goals

- One visible window, owned by us, hosting both phases. No second
  window ever appears; the user's own browser is never touched.
- A pressure-sensitive painting canvas in the locked phase:
  - brush with multiple sizes, mapped from stylus pressure
    (radius/opacity), color and opacity control, eraser;
  - layer management: add/delete/reorder/opacity/visibility, with the
    model's decomposed part-layers at the bottom of the stack;
  - multi-level undo/redo;
  - export: flattened PNG; project archive that reopens with layers
    and the pose-keyed stroke archive intact.
- Adjustment phase: a locally rendered 3D viewport showing the imported
  model; gestures update the camera with zero pipeline round-trips; the
  camera pose streams back to the headless renderer.
- Headless pipeline in containers: watch a folder for CAD exports
  (STEP/OBJ), convert into a Blender scene, and on lock render
  per-part layers offscreen at the streamed pose.
- The spine's discipline is preserved: boundaries are explicit, every
  crossing is ledger-recorded (what moved, bytes, latency), failures
  are data.

## Non-goals

- Modeling inside our window. The user models in their own CAD/modeler;
  we are not a DCC. Models enter only as exported files.
- Live drag-follow sync (60 Hz). Sync happens on lock, not during the
  drag.
- Edit-back from paint into CAD/modeler. Paint is terminal output.
- Cross-platform. macOS only.
- Tilt/barrel-rotation stylus input (pressure only in v1), text/vector/
  selection tools, PSD export.

## Anti-goals

- A second visible window, or any use of the user's own browser
  profile.
- Ambient shared state between components: default isolation, explicit
  flow, as in the bridge spec.
- Any part of the GUI living outside product governance (no lab rules,
  no untracked scratch code).

## Constraints

- macOS with Retina displays: rendering and capture must be
  native-pixel correct (2×), as verified in the spike.
- Pressure input arrives via system pointer pressure events
  (macOS tablets, iPad Sidecar); the web view inside the shell must
  surface `PointerEvent.pressure`. This is the stack's main risk and is
  spike-tested before the stack is committed (see Risks).
- The spine framework layer keeps its zero-domain-knowledge rule; the
  GUI is a product surface above adapters, not part of the spine.
- Every artifact crossing remains ledger-recorded; the ledger is the
  audit trail and the performance instrument.
- Containers make the pipeline reproducible and CI-runnable; the only
  non-containerized component is the shell binary itself.
- **Complexity envelope: small assemblies, ≤50 parts** (user's answer —
  matches the vision's "draw a part" framing). Per-part layer rendering
  and the ~2 s lock render are engineered for this envelope; larger
  assemblies degrade gracefully (flattened fallback) but are not
  designed for.

## Approaches

### Painting capability (decided)

- **Embed an existing web painter** (serve Kleki inside our shell):
  cheapest, but its brush quality, bugs, and layer model belong to
  someone else — rejected by the user.
- **Build our own canvas — CHOSEN.** Full ownership of brush engine,
  layer model, undo, and archive format. Cost: a real painting
  application's workload, accepted deliberately.
- **Export-only (no painting):** smallest scope, but abandons the
  vision — rejected.

### GUI stack (decided)

- **Rust shell + web canvas (Tauri) — CHOSEN.** One native binary; the
  shell is Rust (same language as the An core, artifact/spool concepts
  connect directly), the UI is web technology (Canvas 2D for the brush
  engine and layers, WebGL for the adjustment viewport). Governance
  fits: UI tests run headless in CI, the whole system shares the
  container reproducibility chain, and a version-locked shell
  guarantees the user's browser is never involved. Pressure via
  `PointerEvent.pressure` is the known risk, spike-tested first.
- **Pure Swift native** (AppKit + Metal + SceneKit): highest input
  ceiling, but a third language ecosystem with zero project
  accumulation, a fresh CI line, and no place in the container chain.
  Designated fallback if the pressure spike fails.
- **Pure Rust native** (wgpu, self-drawn canvas): single-language
  governance, but brush engine, layers, undo, and viewport all
  self-built — largest effort, least mature crates for this exact job.

### Adjustment viewport rendering (executor's call)

- **Local embedded WebGL viewport — CHOSEN.** The scene flows from
  headless Blender as glTF once per import; gestures are local and
  latency-free; the camera pose streams back to the container.
- Streamed headless renders per gesture: simpler GUI, but every
  gesture pays a container round-trip — against the "painless" intent.

## Architecture

```
 user's CAD ──exports──> watch folder ──> import container
                                              │ (STEP/OBJ → scene)
                                              ▼
                                    headless Blender container
                                       │            ▲
                              glTF scene│            │camera pose
                                       ▼            │
┌─────────────────────── Bridge Studio (single window) ───────────┐
│  Rust shell (orchestration, ledger, archive, transport)          │
│  ┌────────────────────── web UI ──────────────────────────────┐  │
│  │ adjustment: WebGL viewport  │  locked: Canvas 2D painting  │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
                                       │ lock at pose
                                       ▼
                          headless per-part layer render
                                       │
                                       ▼
                          decomposed layers → canvas (bottom stack)
```

- **Phase machine.** Two states. Adjustment → Locked on an explicit
  lock action (a control in the window); Locked → Adjustment on an
  unlock gesture. While locked, the import pipeline is frozen at the
  current scene snapshot — the model never drifts under the brush.
  Unlocking re-syncs to whatever the watch folder has since delivered.
  (Executor's call: explicit lock action rather than settle-based, so
  the user can keep orbiting past a pause without an unexpected
  freeze.)
- **Pose-keyed archive.** User strokes are stored per camera pose
  (exact pose now, from the renderer — no longer a perceptual-hash
  proxy). Returning to a pose restores the strokes made there.
- **Spine.** The ledger, explicit-boundary transport, and
  zero-domain-knowledge framework layer carry over from the spike as a
  **pattern re-implemented in the Rust shell**, not as a dependency on
  the An runtime: the An core is an MCP bridge for agents, and coupling
  the desktop app to it would weld two products together. The watcher
  that consumes the CAD export folder is itself a container, so the
  shell's only coupling to the pipeline is artifacts and events.
  (Executor's call.)
- **Pose history.** Exact camera poses can never be reproduced by
  hand-orbiting, so "returning to a pose" is a UI feature, not a
  navigation accident: each scene snapshot keeps a browsable pose
  history (thumbnails, nameable, deletable). Recalling a pose restores
  its cached layer set when the scene snapshot is unchanged.
  (Executor's call, forced by the exact-pose decision.)
- **Archive scoping.** Strokes live in **per-pose user-layer stacks**:
  each pose has its own stack of user layers (created empty on first
  lock at that pose), sitting above the model's part-layers. Archives
  are scoped to a scene snapshot; when a re-import changes the model,
  the outgoing snapshot's session is auto-exported (flattened PNG per
  pose) before switching, so no user work is ever silently discarded,
  and the new snapshot starts with a clean pose history. (Executor's
  call.)
- **Part-layers are protected.** The decomposed model layers form a
  locked base group: their content cannot be painted on, deleted, or
  reordered; visibility and opacity are adjustable and stored per pose.
  All painting lives on user layers above. This keeps undo semantics
  and the archive layout sane. (Executor's call.)
- **Canvas resolution.** A locked canvas is a fixed-resolution document
  equal to the window's drawable size in device pixels at lock time.
  Resizing the window while locked letterboxes the document (display
  scaling only) — it never triggers a re-render, and PNG export is
  always honest native resolution. (Executor's call.)
- **Governance.** The GUI lives in the product repository on its own
  dev branch line (`dev-bridge-studio`), with the full
  spec/plan/test/review cycle. Pipeline containers are likewise
  versioned artifacts.

## Risks

- **Pressure events inside the shell web view** (main stack risk):
  `PointerEvent.pressure` behavior under macOS tablets and Sidecar must
  be verified in a minimal Tauri shell before the stack is committed.
  Fallback: Swift native shell (approach two).
- **Brush-engine quality** (feel: smoothing, latency, pressure curves):
  managed by early dogfooding against a reference stroke sheet, not by
  specification detail.
- **Container cold-start latency** at lock time: the per-part render
  must land within the original spec's ~2 s target on the reference
  machine; warm-container strategy is the builder's problem, and the
  ledger records the real numbers.

## Left to the builder

- Brush engine specifics: smoothing/stabilization, pressure curve,
  tile-based compositing or full-canvas, memory bounds.
- Undo model (command-based; bounds and serialization into archive).
- Project archive format (file layout; layers + pose archive + scene
  snapshot reference).
- Container choice and orchestration (runtime, warm pool, folder watch
  mechanics).
- glTF pipeline details (STEP/OBJ import fidelity, per-part naming →
  layer naming contract).
- CI layout for shell UI tests (headless) and pipeline tests
  (containerized fixtures).
- Naming/branding of the window itself; "Bridge Studio" is a working
  title and the executor's call.
