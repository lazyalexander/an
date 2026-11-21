# Software-Bridge Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate the floor loop of the universal software bridge: watch the modeler window, detect viewpoint settle, capture the current view, push it into the painting app as an in-place-updating layer, and log every crossing — with zero user text interaction.

**Architecture:** App-agnostic spine (watcher → settle detector → capture → transport → ledger) with an adapter registry; Blender and Klecks appear only as adapters at floor fidelity. The framework layer carries no domain knowledge: nothing in the spine, ledger, or registry knows what a camera, a part, or a brush is.

**Tech Stack:** Python 3.13 (venv at `lab/bridge/.venv`), macOS `screencapture`/Quartz for observation, Pillow for image diff/dhash, Chrome DevTools Protocol over `websockets` for Klecks injection. Spike lives in the gitignored `lab/bridge/` package per dev-lines; no product files change.

**Spec:** `docs/superpowers/specs/2026-11-22-software-bridge-design.md`

## Global Constraints

- Framework layer is domain-free: spine, settle detector, transport, ledger, and registry must not mention CAD, Blender, Klecks, cameras, parts, or paint (user instruction, 2026-11-22). Application names appear only inside `adapters/`.
- Settle-to-update only; no live drag-follow. Target: viewpoint settle → canvas update under ~2 s on the reference machine.
- No user-facing text, prompts, dialogs, or typed configuration at any point in the loop.
- No silent mock fallback: if an application cannot be observed or reached, the bridge records the failure in the ledger and simply does not update.
- Lab discipline per dev-lines: all code under `lab/bridge/`, gitignored by the product repo; may be dirty; never pushed to the product remote.
- Tests are minimal and cover only pure logic (settle state machine, diff metric, dhash, ledger). GUI-facing behavior is verified by the spike's manual end-to-end check, not unit tests.

## Review Focus

Inputs and failure modes the spec implies but unit tests cannot exercise — each gets a manual check step in Task 9:

1. **Modeler window occluded by another app during capture** — capture must still read the modeler's own pixels (verify `screencapture -l<windowid>` behavior; if it returns occluder pixels, the watcher must detect the mismatch and skip the cycle).
2. **Modeler window closed mid-session** — spine records a ledger entry and idles; no crash, no stale pushes.
3. **Retina scaling mismatch** — window bounds are in points, captures in pixels; regions must scale or layers arrive at half size.
4. **Animated viewport (video, animated cursor) defeats settle detection** — a threshold so small continuous noise never settles must exist (diff ratio below epsilon does not count as change).
5. **Klecks tab reloaded/crashed** — next push detects missing session, relaunches the bridge-owned window, and re-pushes; the user loses nothing (snapshots in the spool dir).

## File Structure

```text
lab/bridge/
├── requirements.txt          # pillow, websockets
├── README.md                 # what the spike is, how to run, floor-tier honesty
├── spine/
│   ├── __init__.py
│   ├── ledger.py             # context ledger: JSONL append + read
│   ├── images.py             # screenshot diff ratio, dhash pose proxy, region crop
│   ├── settle.py             # settle detector state machine
│   ├── watcher.py            # window discovery + per-window screenshot loop
│   ├── transport.py          # move artifact across a boundary + ledger entry
│   ├── registry.py           # adapter protocol + floor defaults
│   └── main.py               # spine loop wiring (watch → settle → capture → push)
├── adapters/
│   ├── __init__.py
│   ├── generic_modeler.py    # floor: window capture of any modeler
│   └── klecks.py             # bridge-owned Chrome window + CDP layer push/read
└── tests/
    ├── test_ledger.py
    ├── test_images.py
    ├── test_settle.py
    └── test_registry.py
```

---

### Task 1: Context ledger

**Files:**
- Create: `lab/bridge/spine/ledger.py`
- Test: `lab/bridge/tests/test_ledger.py`

**Interfaces:**
- Produces: `Ledger(path: Path)` with `record(source: str, destination: str, artifact: str, bytes_count: int, latency_ms: float, status: str, error: str | None = None) -> None` and `entries() -> list[dict]`; schema_version 1 per line; UTC timestamps for identity only.

- [ ] **Step 1: Write the failing test** — append two entries, read them back; assert field names, schema version, byte counts round-trip, and that a `status="failed"` entry with `error` text is preserved verbatim.
- [ ] **Step 2:** Run `python -m unittest discover -s tests` from `lab/bridge` — expect FAIL (module missing).
- [ ] **Step 3:** Implement `ledger.py` (stdlib `json`, `datetime`; one JSON object per line; open-append-close per record so a crash loses at most the in-flight entry).
- [ ] **Step 4:** Tests pass.
- [ ] **Step 5:** Commit `feat(lab): bridge spike ledger`.

### Task 2: Image primitives

**Files:**
- Create: `lab/bridge/spine/images.py`
- Test: `lab/bridge/tests/test_images.py`

**Interfaces:**
- Produces: `diff_ratio(a: Path, b: Path) -> float` (fraction of pixels differing beyond per-pixel tolerance 8/255, after downscale to 256px on the long edge); `dhash(path: Path) -> str` (64-bit hex, difference hash at 8x8); `hamming(a: str, b: str) -> int`; `crop_to_region(path: Path, region: tuple[int,int,int,int], scale: float) -> Path` (region in points, scale = pixel/point ratio, writes PNG to a sibling path).

- [ ] **Step 1: Write the failing tests** — synthetic Pillow images: identical → 0.0; 30% region recolored → ~0.3 within tolerance slack; dhash of an image vs a one-pixel-shifted copy differs by ≤ 6 bits; crop maps point-region to pixel-region with scale 2.0.
- [ ] **Step 2–5:** RED → implement (Pillow only) → GREEN → commit `feat(lab): bridge image primitives`.

### Task 3: Settle detector

**Files:**
- Create: `lab/bridge/spine/settle.py`
- Test: `lab/bridge/tests/test_settle.py`

**Interfaces:**
- Consumes: `diff_ratio` (Task 2).
- Produces: `SettleDetector(change_epsilon: float, dwell_ms: int)` with `feed(diff: float, now_ms: int) -> str` returning `"quiet"` / `"moving"` / `"settled"`; a diff ≥ epsilon marks movement, and `"settled"` fires exactly once when no movement for `dwell_ms` after movement; subsequent quiet feeds return `"quiet"` until the next movement.

- [ ] **Step 1: Write the failing tests** — scripted diff sequences: move→quiet bursts of 400ms fires settled once; continuous noise below epsilon never settles; a new movement re-arms.
- [ ] **Step 2–5:** RED → implement as a 3-state machine with monotonic ms input (no wall clock inside; testability) → GREEN → commit `feat(lab): settle detector`.

### Task 4: Window watcher

**Files:**
- Create: `lab/bridge/spine/watcher.py`
- Test: none (GUI-facing; manual verification in Task 9)

**Interfaces:**
- Consumes: `diff_ratio` (Task 2), `SettleDetector` (Task 3).
- Produces: `find_window_id(app_name: str) -> int | None` (via Quartz `CGWindowListCopyWindowInfo`, case-insensitive substring on owner name); `capture_window(window_id: int) -> Path` (`screencapture -l -x -o <wid>`, PNG to spool dir); `Watcher(app_name, spool_dir, detector, poll_hz=5)` with `events()` generator yielding `("settled", screenshot_path)`.

- [ ] **Step 1:** Implement `find_window_id` + `capture_window`; log every capture failure (window gone) as a retryable condition, never an exception.
- [ ] **Step 2:** Manual smoke: script prints window id and saves a capture for a named app; confirm pixels belong to the app's window.
- [ ] **Step 3:** Commit `feat(lab): window watcher`.

### Task 5: Adapter registry + floor defaults

**Files:**
- Create: `lab/bridge/spine/registry.py`, `lab/bridge/adapters/generic_modeler.py`
- Test: `lab/bridge/tests/test_registry.py`

**Interfaces:**
- Consumes: `Watcher.capture_window` (Task 4), `dhash` (Task 2).
- Produces: `Adapter` protocol with optional capabilities `capture_view() -> Path`, `pose_key() -> str`, `push_layers(artifacts: list[Path]) -> None`, `read_canvas() -> bytes | None`. `Registry.register(name: str, adapter: Adapter)` / `Registry.floor_modeler(watcher) -> Adapter` — floor `capture_view` = window capture, floor `pose_key` = `dhash` of the capture (the spec's hash fallback).

- [ ] **Step 1: Write the failing tests** — a registry with only floor adapters answers `capture_view`/`pose_key`; an adapter that omits `push_layers` raises `MissingCapability` on that call with the capability name in the message; domain words check: `grep -ri "blender\|klecks\|cad\|paint\|camera\|part" spine/` returns nothing except this test file.
- [ ] **Step 2–5:** RED → implement → GREEN → commit `feat(lab): adapter registry and floor modeler`.

### Task 6: Klecks adapter (bridge-owned browser)

**Files:**
- Create: `lab/bridge/adapters/klecks.py`
- Test: none (GUI-facing; manual verification in Task 9)

**Interfaces:**
- Consumes: `Ledger` (Task 1).
- Produces: `KlecksAdapter(spool_dir: Path, ledger: Ledger, port: int = 9333)` implementing `push_layers(list[Path])`, `read_canvas() -> bytes | None`, `pose_key()` from the last pushed artifact's dhash; `ensure_session() -> None` launches or reuses a Chrome window with `--remote-debugging-port=<port>` pointed at klecks.org and verifies CDP reachability.

- [ ] **Step 1:** Implement `ensure_session` (launch Chrome via `open -na "Google Chrome" --args ...`; poll `http://localhost:<port>/json` for the tab).
- [ ] **Step 2:** Implement CDP helpers over `websockets`: `Runtime.evaluate` for JS in the page.
- [ ] **Step 3:** Implement `push_layers`: base64 the PNG into the page, use Klecks' file-import path (builder explores `KL*` app object in the spike; fallback: synthetic drop event on the canvas) sized to the existing canvas, letterboxed, as a layer named `bridge-view`; repeated pushes replace that layer in place.
- [ ] **Step 4:** Implement `read_canvas` (export dataURL → bytes) and the idle snapshot: write canvas bytes to `spool_dir/snapshots/` on every push.
- [ ] **Step 5:** Manual smoke: launch, push one image twice, confirm one `bridge-view` layer updates in place and a snapshot file exists.
- [ ] **Step 6:** Commit `feat(lab): Klecks adapter`.

### Task 7: Spine loop

**Files:**
- Create: `lab/bridge/spine/main.py`
- Test: none (wiring; Task 9 verifies end to end)

**Interfaces:**
- Consumes: everything above. `run(modeler_app: str, paint_adapter: Adapter, spool_dir: Path, dwell_ms: int = 400, poll_hz: int = 5) -> None`.
- On each settled event: `capture_view()` → `transport.record` (ledger entry with bytes + latency) → `push_layers` → failure at any hop = ledger `status="failed"` and keep watching (no exception escapes the loop).

- [ ] **Step 1:** Implement `transport.record` helper (wraps capture/push with monotonic timing + ledger).
- [ ] **Step 2:** Implement the loop with the deferral rule: skip push while the paint app's window is changing (reuse `diff_ratio` on its window, idle = below epsilon for 2 s).
- [ ] **Step 3:** `python -m spine.main --modeler "Blender" --spool <dir>` entry point; Ctrl-C exits cleanly with the ledger flushed.
- [ ] **Step 4:** Commit `feat(lab): spine loop`.

### Task 8: README + requirements

**Files:**
- Create: `lab/bridge/requirements.txt`, `lab/bridge/README.md`

- [ ] **Step 1:** `requirements.txt`: `pillow`, `websockets` (exact pins from the venv that runs the spike).
- [ ] **Step 2:** README: what the spike proves, how to run (`python -m spine.main --modeler Blender`), the floor-tier honesty statement (single flattened layer; per-part decomposition arrives with the Blender adapter milestone), and the no-text constraint.
- [ ] **Step 3:** Commit `docs(lab): bridge spike readme`.

### Task 9: Manual spike verification

**Files:**
- Modify: `lab/bridge/README.md` (record results)

- [ ] **Step 1: Happy path** — open Blender (default cube), open the bridge-owned Klecks window, start the spine; orbit in Blender and stop; assert within ~2 s the Klecks canvas shows the new view as the `bridge-view` layer; orbit again; assert the layer updated in place (layer count unchanged) and the ledger has one entry per settle with bytes and latency.
- [ ] **Step 2: Review Focus #1** — cover the Blender window with another app and orbit blindly; capture must not contain the occluder; if it does, record the finding and gate pushes on mismatch.
- [ ] **Step 3: Review Focus #2** — close Blender mid-session; spine idles, ledger records the failure, restart Blender → watching resumes.
- [ ] **Step 4: Review Focus #3** — confirm a Retina capture lands at full resolution (pixel dims = point bounds × 2).
- [ ] **Step 4: Review Focus #4** — play an animation in the viewport; no settled event fires until it stops.
- [ ] **Step 5: Review Focus #5** — reload the Klecks tab; next settle re-establishes the session and the last snapshot in `spool_dir/snapshots/` matches the canvas.
- [ ] **Step 6:** Record measured settle→update latency from the ledger (median over ≥5 settles); append results to the README.
- [ ] **Step 7:** Commit `docs(lab): spike verification results`.

---

## Self-Review Notes

- Spec coverage: spike milestone only (floor loop + ledger + snapshot durability + pose-keyed archive groundwork via `pose_key`/`read_canvas` interfaces). Blender per-part adapter, paint archive restore, and Photoshop are later milestones, per the spec's milestone order.
- Proportion: test-bearing tasks are pure logic; GUI tasks carry manual checks because the spec's no-mock rule forbids faking the desktop.
- Framework-layer domain check is enforced by a test in Task 5, not by reviewer vigilance.
