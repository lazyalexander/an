# Bridge Studio — Shell + Painting Canvas Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Bridge Studio single window as a working painting application — Tauri shell, pressure-sensitive brush, layers, undo/redo, PNG export, reopenable project archive, and a two-phase skeleton whose adjustment phase is a stub — so that Plan 2 (headless pipeline + viewport) slots into an already-governed product.

**Architecture:** Rust shell (Tauri v2) hosting a web UI. All painting logic lives in DOM-free ES modules (layer model holds pixel buffers; brush engine emits dab geometry; undo is a command stack) tested under `node --test`; a thin compositor module is the only DOM code. Rust owns file persistence (archive read/write, PNG export) behind Tauri commands.

**Tech Stack:** Rust 1.98 + Tauri v2 (+ tauri-plugin-dialog, serde, serde_json); vanilla JS ES modules (zero runtime JS deps); node:test for JS tests; cargo test for Rust.

**Spec:** `docs/superpowers/specs/2026-11-22-bridge-studio-design.md` (read it first; it records the user's decisions and the executor's calls — pose-scoped layer stacks, protected part-layers, fixed-resolution documents, explicit lock/unlock).

**Sequencing:** This is Plan 1 of 2. It delivers a standalone painting app (Plan 2, written after this lands, adds the container pipeline, glTF viewport, real poses, and the ledger). Archive schema and phase machine here are already shaped for Plan 2 so no migration is needed.

## Global Constraints

- macOS only; Retina native-pixel correctness: all canvas buffers are device-pixel sized (`css * devicePixelRatio`), export is always native resolution (spec: Constraints).
- Rust 1.98, edition 2021; Tauri v2. Allowed Rust deps: `tauri`, `tauri-build`, `tauri-plugin-dialog`, `serde`, `serde_json` — nothing else without a recorded reason.
- Zero runtime JS dependencies (vanilla ES modules). Dev-only: `@tauri-apps/cli` (npm), `node:test`.
- All product code is tracked in git on branch `dev-bridge-studio`; lab rules do not apply. The pressure spike (Task 1) is the only throwaway deliverable.
- Test gates (must pass at every task boundary): `cd studio && node --test test/` and `cd studio/src-tauri && cargo test`.
- DOM-free rule: no `document`, `window`, or `ImageData` reference inside `studio/src/engine/` or `studio/src/phase.js`; buffers are plain `{width, height, data: Uint8ClampedArray}` objects. (This is what makes the logic unit-testable.)
- Commit at the end of every task with a conventional message.

## Review Focus

1. **Mouse/trackpad input** (no stylus): `PointerEvent.pressure` is 0.5 for pressed mouse buttons — strokes must render with mid pressure, never zero. Pinned in Task 5.
2. **Dropped pointer coalescing**: fast strokes deliver events via `getCoalescedEvents()`; reading only the top-level event loses dabs and produces polygonal curves. Pinned in Tasks 5 and 7.
3. **Corrupt or foreign archive file**: opening a non-JSON or wrong-version file must produce a clear error and leave the app running with its current document, never crash or blank it. Pinned in Tasks 9 (Rust validation) and 9 (JS parse).
4. **Undo bounds and interleaving**: history is capped (100 commands); a new stroke clears redo; undoing a layer delete restores pixels. Pinned in Task 4.
5. **Window resize while locked**: the document must letterbox (display scaling only), never re-rasterize, never change resolution. Pinned in Task 6 (math) and Task 10 (wiring).

---

### Task 1: Pressure spike (throwaway, stack gate)

**Files:**
- Create: `studio/spikes/pressure/` (minimal self-contained Tauri app)
- Create: `studio/spikes/pressure/RESULTS.md`

**Interfaces:**
- Consumes: nothing.
- Produces: a go/no-go verdict on `PointerEvent.pressure` inside a Tauri WKWebView, recorded in `RESULTS.md` (device, pressure range observed, sample counts). A "no" verdict stops the plan and escalates to the user (design names the Swift fallback).

- [ ] **Step 1: Build the minimal app**

Manually (no `create-tauri-app`): `Cargo.toml` (deps `tauri = "2"`, `tauri-build = "2"`), `build.rs` (`tauri_build::build()`), `tauri.conf.json` (identifier `com.bridge-studio.pressure-spike`, one 800×600 window, `frontendDist: "./ui"`), `src/main.rs` registering no commands. `ui/index.html` contains a full-window `<canvas>` and a `<pre>` log; a pointermove/pointerdown handler appends `{type, pressure, pointerType}` lines to the log and draws dots whose radius is `40 * e.pressure`.

- [ ] **Step 2: Run it and exercise real hardware**

Run: `cd studio/spikes/pressure && cargo run` (first build takes minutes). Draw with (a) mouse/trackpad, (b) every available pressure device (tablet stylus, iPad Sidecar). Requires the user's devices — ask the user to draw if needed.

- [ ] **Step 3: Record the verdict**

Expected: mouse reports pressure 0.5 while pressed; pressure devices report varying values in (0, 1]. Write observed numbers and the go/no-go into `RESULTS.md`. If pressure never varies from 0/0.5/1 constants on stylus hardware, verdict is NO — stop, escalate.

- [ ] **Step 4: Commit**

```bash
git add studio/spikes/pressure && git commit -m "spike: verify PointerEvent.pressure inside a Tauri shell"
```

### Task 2: Product scaffold

**Files:**
- Create: `studio/package.json`, `studio/src-tauri/{Cargo.toml,build.rs,tauri.conf.json,src/main.rs}`, `studio/src/{index.html,style.css,main.js}`
- Test: trivial Rust test + trivial JS test proving both gates run

**Interfaces:**
- Consumes: Task 1 verdict (go).
- Produces: buildable app opening one 1280×800 window titled "Bridge Studio"; npm scripts `dev` (`tauri dev`), `test:js` (`node --test test/`); Rust gate `cd studio/src-tauri && cargo test`.

- [ ] **Step 1: Write the failing tests**

`studio/test/scaffold.test.js`: `assert.equal(1 + 1, 2)` (gate smoke). `studio/src-tauri/src/main.rs`: `#[cfg(test)] mod tests { #[test] fn scaffold_gate_runs() { assert_eq!(2 + 2, 4); } }`.

- [ ] **Step 2: Run tests to verify they fail (no files yet)**

Run: `cd studio && node --test test/` and `cd studio/src-tauri && cargo test`
Expected: both fail (paths/crate missing).

- [ ] **Step 3: Create the scaffold**

`Cargo.toml`: package `bridge-studio`, edition 2021; deps `tauri = "2"`, `tauri-plugin-dialog = "2"`, `serde`, `serde_json`; build-dep `tauri-build = "2"`. `tauri.conf.json`: identifier `dev.bridge.studio`, productName `Bridge Studio`, `frontendDist: "../src"`. `main.rs`: builds the app with the dialog plugin and runs. `index.html`: dark full-window page with a centered placeholder `<div id="phase-adjustment">adjustment (stub)</div>`. `package.json`: devDependency `@tauri-apps/cli` ^2, scripts as in Produces. `.gitignore` entries: `studio/node_modules`, `studio/src-tauri/target`.

- [ ] **Step 4: Run both gates; verify the window opens**

Run: both gates → PASS. Then `cd studio && npm install && npm run dev` → window opens showing the stub. Close it.

- [ ] **Step 5: Commit**

```bash
git add studio && git commit -m "feat(studio): scaffold the Bridge Studio shell"
```

### Task 3: Layer model

**Files:**
- Create: `studio/src/engine/layers.js`
- Test: `studio/test/layers.test.js`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `createLayerStack(width: number, height: number) -> Stack` where `Stack = { width, height, layers: Layer[] }` (bottom-first) and `Layer = { id, name, kind: 'user'|'part', opacity: 0..1, visible: bool, pixels: {width, height, data: Uint8ClampedArray} }`
  - `addLayer(stack, name?) -> { stack, id }` (appends on top, kind `'user'`, fully transparent pixels)
  - `removeLayer(stack, id) -> { stack, removed: {layer, index} }` — returns enough for undo
  - `insertLayer(stack, layer, index) -> stack`
  - `moveLayer(stack, id, toIndex) -> stack`
  - `setOpacity(stack, id, opacity) -> stack` (clamps 0..1), `setVisible(stack, id, visible) -> stack`, `setName(stack, id, name) -> stack`
  - `getLayer(stack, id) -> Layer`
  - All ops return new stack objects (structural sharing of untouched layer objects); pixels buffers are shared by reference and mutated only by paint ops (Tasks 5), never by these ops.

- [ ] **Step 1: Write the failing tests**

```js
test('add/remove/move preserve order and support undo restore', () => {
  let s = createLayerStack(8, 8);
  ({ stack: s, id: a } = addLayer(s, 'a'));
  ({ stack: s, id: b } = addLayer(s, 'b'));
  assert.deepEqual(s.layers.map(l => l.name), ['a', 'b']);
  const { stack: s2, removed } = removeLayer(s, a);
  assert.deepEqual(s2.layers.map(l => l.name), ['b']);
  const s3 = insertLayer(s2, removed.layer, removed.index);
  assert.deepEqual(s3.layers.map(l => l.name), ['a', 'b']);
  assert.equal(moveLayer(s3, a, 1).layers[1].name, 'a');
});
test('part layers reject remove and move', () => { /* kind:'part' layer → removeLayer/moveLayer throw Error('part layers are protected') */ });
test('opacity clamps and visibility toggles', () => { /* setOpacity(s,id,1.7).layers[0].opacity === 1; setOpacity(s,id,-0.2) === 0; setVisible flips */ });
test('new layer pixels are zero-filled and sized', () => { /* data.length === 8*8*4, every byte 0 */ });
```

- [ ] **Step 2: Run to verify fail** — `cd studio && node --test test/` → `layers.js` missing.

- [ ] **Step 3: Implement `layers.js`**

Straight data ops; the protected-part rule is the only policy (spec: "part-layers are protected").

- [ ] **Step 4: Run to verify pass** — same command, PASS.

- [ ] **Step 5: Commit** — `feat(studio): layer stack model with protected part layers`

### Task 4: Undo/redo history

**Files:**
- Create: `studio/src/engine/history.js`
- Test: `studio/test/history.test.js`

**Interfaces:**
- Consumes: Task 3 ops.
- Produces:
  - `createHistory(limit = 100) -> History`
  - `push(history, cmd)` where `cmd = { undo(), redo(), label }`; pushing clears the redo stack and evicts the oldest beyond `limit`
  - `undo(history) -> bool`, `redo(history) -> bool`, `canUndo(history)`, `canRedo(history)`
  - Helper commands used by later tasks:
    - `layerOpCommand(applyOp, inverseOp)` — generic wrapper
    - `strokeCommand(layer, dirty: {x,y,width,height}, before: Uint8ClampedArray, after: Uint8ClampedArray)` — restores/copies the rect region of `layer.pixels.data`

- [ ] **Step 1: Write the failing tests**

```js
test('undo/redo walk the stack in order', () => { /* three labelled commands; undo×2 → labels ['c','b'] undone in that order; redo×1 redoes 'b' */ });
test('push clears redo', () => { /* undo once, push new command → canRedo false */ });
test('history is bounded at limit', () => { /* limit 3, push 5 → only last 3 undoable */ });
test('strokeCommand restores the dirty rect exactly', () => {
  const layer = { pixels: { width: 4, height: 4, data: new Uint8ClampedArray(4*4*4) } };
  layer.pixels.data.fill(7);
  const before = layer.pixels.data.slice();
  layer.pixels.data.fill(9);
  const after = layer.pixels.data.slice();
  const cmd = strokeCommand(layer, {x:0,y:0,width:4,height:4}, before, after);
  cmd.undo(); assert.deepEqual([...layer.pixels.data], [...before]);
  cmd.redo(); assert.deepEqual([...layer.pixels.data], [...after]);
});
```

- [ ] **Step 2: Run to verify fail**.

- [ ] **Step 3: Implement `history.js`**

Two arrays with a cursor is sufficient; `strokeCommand` copies only the dirty rect rows.

- [ ] **Step 4: Run to verify pass**.

- [ ] **Step 5: Commit** — `feat(studio): bounded undo/redo command stack`

### Task 5: Brush engine

**Files:**
- Create: `studio/src/engine/brush.js`, `studio/src/engine/pixels.js`
- Test: `studio/test/brush.test.js`, `studio/test/pixels.test.js`

**Interfaces:**
- Consumes: Task 3 `Layer` shape.
- Produces:
  - `createStroke({ color: [r,g,b,a0..255], size, opacity: 0..1, mode: 'brush'|'eraser' }) -> Stroke`
  - `strokePoint(stroke, points: Array<{x, y, pressure}>) -> Dab[]` — accepts coalesced point arrays; `Dab = {x, y, radius, alpha, mode}`
  - Dab spacing = `radius * 0.25` along each segment; `radius = size * clamp(pressure, 0.05, 1)`; brush `alpha = opacity * pressure`; eraser `alpha = opacity`; the first point of a stroke always emits a dab (single-tap dot)
  - Position smoothing: EMA with factor 0.5 over the raw points
  - `drawDab(layer, dab, color) -> dirtyRect` (in `pixels.js`) — source-over blend of the colored disc into `layer.pixels`; eraser mode subtracts alpha instead; returns the affected rect `{x,y,width,height}` (clamped) for undo capture
  - `captureRect(layer, rect) -> Uint8ClampedArray`, `blitRect(layer, rect, buf) -> void` (undo helpers consumed by Task 4's `strokeCommand`)

- [ ] **Step 1: Write the failing tests**

```js
test('pressure maps to radius and alpha; mouse 0.5 is mid', () => {
  const s = createStroke({ color: [10,20,30,255], size: 20, opacity: 0.8, mode: 'brush' });
  const dabs = strokePoint(s, [{x:0,y:0,pressure:1},{x:10,y:0,pressure:0.5}]);
  assert.equal(dabs[0].radius, 20);           // pressure 1 → full size
  assert.ok(dabs.at(-1).radius <= 10.5);      // pressure 0.5 → half size
  assert.ok(Math.abs(dabs[0].alpha - 0.8) < 1e-6);
});
test('spacing follows the segment and nothing is dropped between coalesced points', () => {
  const s = createStroke({ color: [0,0,0,255], size: 8, opacity: 1, mode: 'brush' });
  const dabs = strokePoint(s, [{x:0,y:0,pressure:1},{x:100,y:0,pressure:1}]);
  assert.ok(dabs.length > 40);                // 100px at radius 8 → spacing 2 → ≈50 dabs
});
test('a single tap emits exactly one dab', () => { /* strokePoint(s,[onePoint]).length === 1 */ });
test('drawDab blends source-over and reports a clamped dirty rect', () => {
  const layer = /* 16×16 transparent layer from Task 3 */;
  const rect = drawDab(layer, {x:15, y:15, radius: 8, alpha: 1, mode: 'brush'}, [255,0,0,255]);
  assert.deepEqual(rect, {x:7, y:7, width:9, height:9});   // clamped to canvas
  const i = (15*16 + 15) * 4;
  assert.equal(layer.pixels.data[i+3], 255);               // center fully covered
});
test('eraser subtracts alpha only inside the radius', () => { /* fill 255, erase alpha 1 at center → center 0, outside untouched */ });
```

- [ ] **Step 2: Run to verify fail**.

- [ ] **Step 3: Implement `brush.js` and `pixels.js`**

Disc rasterization: loop the bounding box, coverage by distance to center with a 1px anti-aliased rim; blend `out_a = a + sa*(1-a)`, `out_rgb = (rgb*sa + dst_rgb*dst_a*(1-sa)) / out_a`.

- [ ] **Step 4: Run to verify pass**.

- [ ] **Step 5: Commit** — `feat(studio): pressure-sensitive brush engine`

### Task 6: Document and letterbox math

**Files:**
- Create: `studio/src/engine/document.js`
- Test: `studio/test/document.test.js`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `createDocument(cssWidth, cssHeight, dpr) -> { width, height }` — device pixels, `Math.round(css * dpr)`
  - `letterboxTransform(docWidth, docHeight, cssWidth, cssHeight) -> { scale, offsetX, offsetY }` — uniform scale to fit, centered; never above 1 on the axis that fits
  - `createDocument` throws when either dimension rounds to 0 (window too small to paint — caller shows the adjustment stub instead)

- [ ] **Step 1: Write the failing tests**

```js
test('retina doubles the backing store', () => {
  assert.deepEqual(createDocument(800, 600, 2), { width: 1600, height: 1200 });
});
test('letterbox fits, centers, and never crops', () => {
  const t = letterboxTransform(1600, 1200, 800, 800);
  assert.equal(t.scale, 0.5);
  assert.equal(t.offsetY, 100); assert.equal(t.offsetX, 0);
});
test('degenerate windows are rejected', () => { assert.throws(() => createDocument(0.2, 0.2, 2)); });
```

- [ ] **Step 2: Run to verify fail**.

- [ ] **Step 3: Implement `document.js`**.

- [ ] **Step 4: Run to verify pass**.

- [ ] **Step 5: Commit** — `feat(studio): fixed-resolution document with letterbox math`

### Task 7: Compositor (DOM wiring)

**Files:**
- Create: `studio/src/ui/compositor.js`
- Modify: `studio/src/index.html`, `studio/src/style.css`

**Interfaces:**
- Consumes: Tasks 3–6 (`createLayerStack`, ops, `createHistory`, `push`, `strokeCommand`, `createStroke`, `strokePoint`, `drawDab`, `captureRect`, `createDocument`, `letterboxTransform`).
- Produces:
  - `mountCompositor(rootEl, document) -> Compositor` where `Compositor = { stack(), history(), setTool(tool), tool(), resize(cssWidth, cssHeight, dpr), destroy() }` and `tool = { color, size, opacity, mode }`
  - Behavior contract (verified by smoke below): pointerdown starts a stroke on the active layer; pointermove consumes `e.getCoalescedEvents()`; pointerup pushes one `strokeCommand` covering the union dirty rect; rAF loop composites visible layers with opacity into the display canvas via `letterboxTransform`; window resize re-letterboxes without touching buffers.

- [ ] **Step 1: Implement `compositor.js`**

One offscreen `<canvas>` per layer (created lazily), `putImageData` of dirty rects into it, then `drawImage` composite; display canvas sized to the window, transform from Task 6. Undo capture: `captureRect` before the stroke's first dab of each dirty rect; `blitRect`-based after-image at pointerup; push via Task 4 helpers.

- [ ] **Step 2: Smoke-verify in the running app**

`npm run dev` → (temporary: mount the compositor directly instead of the stub for this task) draw strokes at several sizes, erase, toggle a layer's visibility, resize the window mid-draw. Expected: smooth pressure strokes with the tablet from Task 1, eraser reveals transparency, resize letterboxes (no repaint of content, no resolution change), undo/redo buttons walk history.

- [ ] **Step 3: Revert the temporary mount; commit**

`feat(studio): compositor wiring pointer events to the layer stack`

### Task 8: Toolbar and layers panel

**Files:**
- Create: `studio/src/ui/toolbar.js`
- Modify: `studio/src/index.html`, `studio/src/style.css`

**Interfaces:**
- Consumes: Task 7 `Compositor`.
- Produces:
  - `mountToolbar(rootEl, compositor) -> void` — controls: color (`<input type="color">`), size (range 1–64), opacity (range 0–100), brush/eraser toggle, undo/redo buttons (disabled state follows `canUndo/canRedo`)
  - `mountLayersPanel(rootEl, compositor) -> void` — rows bottom-to-top: name, visibility checkbox, opacity slider, delete; plus add-layer button and per-row up/down reorder buttons (drag reorder deferred); rows for `kind: 'part'` layers show a lock glyph and no delete/reorder controls (spec: protected)

- [ ] **Step 1: Implement both mounts** against the Task 7 API; re-render the panel from `compositor.stack()` after every op.

- [ ] **Step 2: Smoke-verify** — every control changes the next stroke or the stack visibly; protected-layer rows have no mutation controls.

- [ ] **Step 3: Commit** — `feat(studio): toolbar and layers panel`

### Task 9: Archive save/load + PNG export

**Files:**
- Create: `studio/src/engine/archive-model.js`, `studio/src-tauri/src/archive.rs`, `studio/src-tauri/src/commands.rs`
- Modify: `studio/src-tauri/src/main.rs` (register commands)
- Test: `studio/test/archive-model.test.js`, `studio/src-tauri/src/archive.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Task 3 stack, Task 6 document.
- Produces:
  - JS: `serializeArchive({ document, poses, activePoseId }) -> string` and `parseArchive(json) -> { document, poses, activePoseId }`; archive = `{ schemaVersion: 1, document: {width,height}, poses: [{id, name, partLayers: [], userLayers: [{name, opacity, visible, png: 'data:image/png;base64,...'}]}], activePoseId }` — poses exist in the schema now (Plan 2 fills real ones); v1 writes exactly one pose `{id: 'default', name: 'Default'}`
  - Rust commands (Tauri): `save_archive(path: String, json: String) -> Result<(), String>`, `load_archive(path: String) -> Result<String, String>`, `export_png(path: String, bytes: Vec<u8>) -> Result<(), String>`
  - Rust `archive.rs`: `validate_archive(json: &str) -> Result<ArchiveMeta, ArchiveError>` — parses JSON, requires `schemaVersion == 1`, `document.width/height > 0`, `poses` non-empty; returns `ArchiveMeta { pose_count, layer_count }`

- [ ] **Step 1: Write the failing tests**

```js
test('serialize/parse round-trips layer metadata and png payloads', () => {
  const input = { document: {width:1600,height:1200}, poses: [{id:'default',name:'Default',partLayers:[],userLayers:[{name:'L1',opacity:0.5,visible:true,png:'data:image/png;base64,AAAA'}]}], activePoseId: 'default' };
  const out = parseArchive(serializeArchive(input));
  assert.deepEqual(out, input);
});
test('parseArchive rejects wrong versions and malformed json', () => {
  assert.throws(() => parseArchive('{"schemaVersion": 99}'));
  assert.throws(() => parseArchive('not json'));
});
```
```rust
#[test]
fn validate_accepts_minimal_archive() {
    let meta = validate_archive(r#"{"schemaVersion":1,"document":{"width":2,"height":2},"poses":[{"id":"default"}],"activePoseId":"default"}"#).unwrap();
    assert_eq!(meta.pose_count, 1);
}
#[test]
fn validate_rejects_garbage_and_bad_version() {
    assert!(validate_archive("garbage").is_err());
    assert!(validate_archive(r#"{"schemaVersion":2}"#).is_err());
}
#[test]
fn save_then_load_round_trip_in_tempdir() { /* save_archive into std::env::temp_dir file, load_archive returns identical string */ }
```

- [ ] **Step 2: Run to verify fail** — both gates.

- [ ] **Step 3: Implement**

JS: pure (de)serialization, no DOM (the compositor supplies png data URLs per layer — helper `stackToPngLayers(stack)` lives in `ui/compositor.js`, uses offscreen canvases). Rust: validate, write/read file with `std::fs`, map errors to `String`. `main.rs`: `.invoke_handler` with the three commands. `index.html`: Save/Open/Export buttons wired through `tauri-plugin-dialog` (save dialog default filename `painting.bstudio`, export default `painting.png`).

- [ ] **Step 4: Run to verify pass; then smoke** — paint two layers, save, close, reopen: identical layers; export PNG opens at 1600×1200 (Retina doc).

- [ ] **Step 5: Commit** — `feat(studio): project archive and PNG export`

### Task 10: Phase skeleton + integration

**Files:**
- Create: `studio/src/phase.js`
- Modify: `studio/src/main.js`, `studio/src/index.html`
- Test: `studio/test/phase.test.js`

**Interfaces:**
- Consumes: Tasks 6–9.
- Produces:
  - `createPhaseMachine() -> Machine` — `state(): 'adjustment'|'locked'`, `lock() -> bool`, `unlock() -> bool`, `on('change', cb)`; illegal transitions return `false` and emit nothing
  - `main.js` behavior: start in `adjustment` (stub view with a Lock control); `lock()` creates the fixed-resolution document from the current window size and mounts the compositor + toolbar + panels; `unlock()` destroys the compositor and returns to the stub (document and stack are discarded in Plan 1 — pose persistence arrives with Plan 2); window resize while locked letterboxes only (Review Focus 5)

- [ ] **Step 1: Write the failing tests**

```js
test('transitions are guarded and emit change', () => {
  const m = createPhaseMachine();
  assert.equal(m.state(), 'adjustment');
  assert.equal(m.unlock(), false);
  const seen = []; m.on('change', s => seen.push(s));
  assert.equal(m.lock(), true); assert.equal(m.lock(), false);
  assert.equal(m.unlock(), true);
  assert.deepEqual(seen, ['locked', 'adjustment']);
});
```

- [ ] **Step 2: Run to verify fail**.

- [ ] **Step 3: Implement `phase.js` and wire `main.js`**.

- [ ] **Step 4: Run all gates to verify pass; then the full manual smoke**

`npm run dev`: open → adjustment stub → Lock → paint with pressure stylus → layers/undo → save archive → resize window (letterboxes) → export PNG → unlock → lock again → Open archive → layers intact. This is the Plan-1 acceptance walk.

- [ ] **Step 5: Commit** — `feat(studio): two-phase skeleton with lock/unlock`

---

## Self-Review Notes

- **Spec coverage:** painting feature set (Tasks 3–8), export+archive (9), phases (10), pressure risk (1), Retina/resize calls (6, 7, 10), protected part-layers (3, 8, schema in 9). Plan-2-only items (viewport, pipeline, real poses, ledger, pose-history UI) are out of scope here by the sequencing note; the schema and phase machine are shaped to receive them without migration.
- **Proportion:** bodies appear only for test assertions and the blend/rasterize rules the signatures cannot determine; everything else is signatures and commands.
- **Type consistency:** `Layer`/`Stack` (T3) are the shapes consumed by T4/T5/T7/T9; `strokeCommand` (T4) matches its use in T7; `Compositor` (T7) matches T8/T9/T10; archive schema (T9) matches the spec's pose model.
- **Review Focus mapping:** mouse 0.5 → T5 test; coalesced events → T5 array input + T7 wiring; corrupt archive → T9 tests both sides; undo bounds → T4; resize letterbox → T6 math + T10 wiring.
