# Bridge Studio — Walking Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the governed walking skeleton of Bridge Studio — orchestrator (ledger + phase machine + HTTP API), Krita adapter with the thin vertical slice (create document → push layers → save `.kra` → reopen readback), Blender pose plugin, and a containerized headless pipeline stub — so that the full lock→render→paint loop runs end-to-end on the reference machine and every later feature slots into governed surfaces.

**Architecture:** One local headless Python orchestrator owns the phase machine (adjustment/locked), the append-only JSONL ledger, a folder watcher, and the pipeline triggers; it exposes a stdlib HTTP/JSON API on 127.0.0.1. The Krita plugin (docker) and the Blender addon are thin stdlib-only clients of that API; all testable logic lives in pure adapter/orchestrator cores run under pytest. The pipeline stub runs pinned Blender 5.2.3 (linux/arm64) in Docker to import OBJ and render per-part PNGs (Cycles CPU) into a session layers dir.

**Tech Stack:** Python ≥3.11 (reference: 3.13.12) for orchestrator/pipeline host code; Python 3.10-compatible stdlib-only shims for Krita (bundled Python 3.10) and Blender 5.2.3 (bundled Python 3.11+); pytest (dev-only); Docker Desktop; Blender 5.2.3 LTS; Krita 5.3.4.

**Spec:** `docs/superpowers/specs/2026-11-22-bridge-studio-design.md` (approved 2026-11-22, input-route revision re-approved same day; read it first). Phase-gate context: `docs/superpowers/specs/2026-11-22-bridge-studio-pressure-gate.md`.

**Sequencing:** This is Plan 1 of 2+. It deliberately defers to Plan 2: pose history UI (browse/name/delete/recall), restore-on-save guard for the protected group, auto-export of outgoing work on snapshot change, STEP import, warm-container latency work, and the presentation-layer "stage" shell. Plan 1 already keys every document group, sidecar record, and scene snapshot by stable ids so Plan 2 needs no migration.

## Global Constraints

- macOS only; native-pixel honesty: renders and the Krita document are exactly the configured resolution; nothing rescales (spec: Constraints).
- Krita 5.3.4 stock, official Python API (`krita`/LibKis) only — no fork, no patch, no unsupported internals. Blender 5.2.3 LTS pinned likewise. Versions recorded in `studio/VERSIONS.md` at Task 1.
- Injector (PadMac/PadPad) is a pinned input dependency; Plan 1 code never touches it. Its operational lessons are in the spec's Risks.
- All product code lives under `studio/` on branch `dev-bridge-studio`; the superseded shell-canvas leftovers are removed in Task 1; spikes stay as history.
- Shim code (`adapter/krita_plugin/**`, `blender_addon/__init__.py`) is stdlib-only and Python 3.10-compatible: no `match`, no `tomllib`, no 3.11+ typing syntax.
- Orchestrator and pipeline host code: stdlib-only in Plan 1. Dev-only dependency: pytest (≥7).
- Every crossing between processes is ledger-recorded: `kind`, `summary`, `bytes_moved`, `latency_ms` (`studio/run/ledger.jsonl`). The orchestrator core treats payloads as opaque records; only adapter/pipeline code knows semantics.
- Test gate at every task boundary: `cd studio && python3 -m pytest tests/ -q` must pass. Docker-marked tests (`@pytest.mark.docker`) run only with `STUDIO_DOCKER=1`.
- `studio/run/` is gitignored runtime state (ledger, inbox, scene snapshots, session layers, archives).
- Complexity envelope ≤50 parts; the stub does not optimize (spec: Constraints).
- Commit at the end of every task with a conventional message.

## Review Focus

1. **Orchestrator unreachable** (not started / crashed): the Krita docker must show an "offline" state with disabled buttons and the Blender timer must stay silent — neither shim may raise inside the host app. Pinned in Tasks 7 and 8 (manual checklist steps).
2. **Pose submitted while locked**: the server must reject with 409 and leave the frozen session untouched. Pinned in Task 4.
3. **Lock with no pose ever streamed**: must fail with a clear 409 ("no current pose"); never silently render a default camera. Pinned in Tasks 3 and 4.
4. **Partially copied file in the inbox** (still being written) and **re-dropping the same filename**: the watcher imports only size-stable files, and every import produces a new `snap-NNNN` directory — nothing is ever overwritten. Pinned in Task 10.
5. **Resolution drift**: document size, render size, and PNG IHDR sizes must be identical to the configured resolution end-to-end; a mismatched layer must fail the push, not get rescaled. Pinned in Tasks 6 and 9.

---

### Task 1: Product scaffold and cleanup

**Files:**
- Delete: `studio/test/` (superseded shell-canvas scaffold)
- Create: `studio/pyproject.toml`, `studio/VERSIONS.md`, `studio/tests/test_scaffold.py`
- Create: `studio/{orchestrator,adapter/core,adapter/krita_plugin,blender_addon,pipeline/fixtures,tests}/` (empty `__init__.py` where packages)
- Modify: `studio/.gitignore` (add `run/`)

**Interfaces:**
- Consumes: nothing.
- Produces: pytest gate `cd studio && python3 -m pytest tests/ -q`; `studio/VERSIONS.md` pinning Krita 5.3.4, Blender 5.2.3 LTS, Python 3.13.12, PadMac/PadPad versions copied from `studio/spikes/sidecar-pressure/RESULTS.md`.

- [ ] **Step 1: Write the failing test**

`studio/tests/test_scaffold.py`:

```python
def test_gate_runs():
    assert 1 + 1 == 2
```

- [ ] **Step 2: Run to verify it fails (no pyproject/testpaths yet is fine; failure mode = path missing)**

Run: `cd studio && python3 -m pytest tests/ -q`
Expected: collection error or "no tests ran" before files exist; after creating the file but before pyproject, import layout unproven.

- [ ] **Step 3: Create layout, pyproject, VERSIONS, cleanup**

`pyproject.toml`: `[project]` name `bridge-studio`, version `0.1.0`; `[tool.pytest.ini_options]` with `pythonpath = ["."]`, `testpaths = ["tests"]`, and `markers = {docker: "requires a running Docker daemon (STUDIO_DOCKER=1)"}`; `tests/conftest.py` carries a skip hook that skips `docker`-marked tests unless env `STUDIO_DOCKER=1`. `VERSIONS.md` pins the four versions above. Delete `studio/test/`. Add `run/` to `studio/.gitignore`.

- [ ] **Step 4: Run gate to verify it passes**

Run: `cd studio && python3 -m pytest tests/ -q`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add studio && git commit -m "chore: scaffold studio product layout, drop shell-canvas leftovers"
```

### Task 2: Ledger

**Files:**
- Create: `studio/orchestrator/ledger.py`
- Test: `studio/tests/test_ledger.py`

**Interfaces:**
- Consumes: nothing.
- Produces: `class Ledger:` `__init__(self, path: pathlib.Path, now: collections.abc.Callable[[], float] = time.time)`; `record(self, kind: str, summary: str, bytes_moved: int = 0, latency_ms: float = 0.0, **fields) -> dict`; `read_all(self) -> list[dict]`. Each entry is one JSON object per line carrying `ts` (float, from `now`), `kind`, `summary`, `bytes_moved`, `latency_ms`, plus `**fields`.

- [ ] **Step 1: Write the failing tests**

```python
def test_record_and_read_back(tmp_path):
    led = Ledger(tmp_path / "ledger.jsonl", now=iter([1.0, 2.0]).__next__)
    e = led.record("lock", "session frozen", bytes_moved=128, latency_ms=3.5, session="s1")
    assert e["ts"] == 1.0 and e["session"] == "s1"
    led.record("unlock", "back to adjustment")
    entries = led.read_all()
    assert [x["kind"] for x in entries] == ["lock", "unlock"]
    assert entries[0]["bytes_moved"] == 128

def test_corrupt_line_is_skipped_with_default_fields(tmp_path):
    p = tmp_path / "ledger.jsonl"
    p.write_text('{"kind":"a","summary":"x"}\nnot json\n')
    entries = Ledger(p).read_all()
    assert len(entries) == 1 and entries[0]["bytes_moved"] == 0
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_ledger.py -q`
Expected: ImportError (`orchestrator.ledger` missing).

- [ ] **Step 3: Implement `Ledger`**

Append-open per `record` (flush + `os.fsync`); `read_all` parses line by line, skipping malformed lines and back-filling missing default fields. Parent dir created on init.

- [ ] **Step 4: Run to verify they pass**

Run: `cd studio && python3 -m pytest tests/test_ledger.py -q`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add studio/orchestrator/ledger.py studio/tests/test_ledger.py && git commit -m "feat(orchestrator): append-only JSONL ledger"
```

### Task 3: Phase machine and orchestrator facade

**Files:**
- Create: `studio/orchestrator/phase.py`, `studio/orchestrator/app.py`
- Create: `studio/orchestrator/pipeline_api.py` (protocol + data classes only)
- Test: `studio/tests/test_phase.py`, `studio/tests/test_app.py`

**Interfaces:**
- Consumes: Task 2 `Ledger`.
- Produces (used by Tasks 4, 6, 9, 10):
  - `class Phase(enum.Enum): ADJUSTMENT; LOCKED` and `class InvalidTransition(RuntimeError)`, `class NoPose(RuntimeError)` in `phase.py`.
  - `@dataclass LayerSpec: part: str; png_path: pathlib.Path` and `@dataclass Session: id: str; pose: dict; resolution: tuple[int, int]; snapshot_id: str; state: str  # "rendering" | "rendered" | "failed"` in `pipeline_api.py`.
  - `class Pipeline(typing.Protocol): render(self, session: Session, scene_dir: pathlib.Path, out_dir: pathlib.Path) -> list[LayerSpec]`.
  - `class Orchestrator:` `__init__(self, ledger: Ledger, pipeline: Pipeline, resolution: tuple[int, int], now: Callable[[], float] = time.time)`; `phase -> Phase`; `session -> Session | None`; `submit_pose(self, pose: dict) -> None` (raises `InvalidTransition` when locked); `lock(self) -> Session` (raises `NoPose` when no pose yet, `InvalidTransition` when already locked); `unlock(self) -> None`; `status(self) -> dict` returning `{"phase": str, "pose_age_ms": int | None, "session": asdict(Session) | None, "snapshot_id": str | None, "roster": {"adjustment": "Blender", "locked": "Krita"}}`.
  - Lock renders asynchronously: `lock()` sets state `"rendering"`, starts a `threading.Thread` running `pipeline.render`, then sets `state = "rendered"` and stores `self._layers: list[LayerSpec]` (or `"failed"` on exception). `layers(self) -> list[LayerSpec]` raises `InvalidTransition` unless state is `"rendered"`.
  - Pose ledger aggregation: `submit_pose` coalesces into 1-second windows (by injected `now`); each window produces one `record("pose_stream", ..., count=n, window_ms=1000)`. Lock/unlock record their own entries; render completion records `render_done` with real `latency_ms` and `bytes_moved` (sum of PNG sizes).

- [ ] **Step 1: Write the failing tests**

```python
class FakePipeline:
    def __init__(self, barrier=None):
        self.calls = []
        self.barrier = barrier
    def render(self, session, scene_dir, out_dir):
        self.calls.append(session)
        if self.barrier:
            self.barrier.wait(2)
        return [LayerSpec(part="cube_a", png_path=out_dir / "cube_a.png")]

def test_lock_freezes_pose_and_renders_async(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, FakePipeline(), resolution=(1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    assert s.state in ("rendering", "rendered")
    wait_until(lambda: app.session.state == "rendered")
    assert app.layers()[0].part == "cube_a"
    kinds = [e["kind"] for e in led.read_all()]
    assert "pose_stream" in kinds and "lock" in kinds and "render_done" in kinds

def test_lock_without_pose_raises(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    with pytest.raises(NoPose):
        app.lock()

def test_pose_rejected_while_locked(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    with pytest.raises(InvalidTransition):
        app.submit_pose({"matrix": [2.0] * 16, "location": [1, 1, 1]})

def test_pose_aggregation(tmp_path):
    clock = FakeClock()  # manual advance
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200), now=clock.now)
    for _ in range(25):
        app.submit_pose({"matrix": [0.0] * 16, "location": [0, 0, 0]})
    clock.advance(1.1)
    app.submit_pose({"matrix": [0.0] * 16, "location": [0, 0, 0]})
    entries = [e for e in (tmp_path / "l.jsonl").read_text().splitlines() if e]
    streams = [json.loads(e) for e in entries if '"pose_stream"' in e]
    assert len(streams) == 2 and streams[0]["count"] == 25

def test_unlock_returns_to_adjustment(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    wait_until(lambda: app.session is not None and app.session.state == "rendered")
    app.unlock()
    assert app.phase is Phase.ADJUSTMENT and app.session is None
```

(`wait_until` and `FakeClock` are small test helpers defined in `tests/conftest.py`.)

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_phase.py tests/test_app.py -q`
Expected: ImportError.

- [ ] **Step 3: Implement `phase.py`, `pipeline_api.py`, `app.py`**

State machine is two explicit transitions; `Orchestrator` owns the render thread (daemon) and guards shared state with a `threading.Lock`. Render `out_dir` is `studio/run/sessions/<session_id>/layers/`; `session.id` is `time.strftime("%Y%m%d-%H%M%S") + "-" + uuid4().hex[:6]`.

- [ ] **Step 4: Run to verify they pass**

Run: `cd studio && python3 -m pytest tests/test_phase.py tests/test_app.py -q`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add studio/orchestrator studio/tests && git commit -m "feat(orchestrator): phase machine + facade with async render and pose ledger"
```

### Task 4: HTTP API server

**Files:**
- Create: `studio/orchestrator/server.py`, `studio/orchestrator/__main__.py`
- Test: `studio/tests/test_server.py`

**Interfaces:**
- Consumes: Task 3 `Orchestrator`, `InvalidTransition`, `NoPose`.
- Produces: `def make_server(app: Orchestrator, host: str = "127.0.0.1", port: int = 8791) -> http.server.ThreadingHTTPServer`. JSON API (Content-Type `application/json`; errors `{"error": str}`):
  - `POST /api/pose` body `{"matrix": [16 floats], "location": [3 floats]}` → `200 {"ok": true}`; `409` when locked; `400` on malformed body.
  - `POST /api/lock` empty body → `200` session JSON (`asdict(Session)`); `409` on `NoPose`/`InvalidTransition`.
  - `POST /api/unlock` → `200 {"ok": true}`; `409` when not locked.
  - `GET /api/status` → `Orchestrator.status()` JSON.
  - `GET /api/session/layers` → `200 {"layers": [{"part": str, "png_path": str}]}`; `409 {"error": "rendering"}` while rendering; `409` when no session.
  - `python -m orchestrator` starts the server on 8791 with `DockerPipeline` once Task 9 lands (until then, a `--fake` flag wires `FakePipeline` from the test helpers copied into `orchestrator/fake_pipeline.py`; Task 9 flips the default).

- [ ] **Step 1: Write the failing tests**

End-to-end over real HTTP on an ephemeral port (`make_server(app, port=0)`, `threading.Thread(target=server.serve_forever)`, `http.client`):

```python
def test_full_http_cycle(running_server):
    c, app = running_server  # helper in conftest
    assert post(c, "/api/pose", {"matrix": [1.0] * 16, "location": [0, 0, 0]})[0] == 200
    code, session = post(c, "/api/lock", {})
    assert code == 200 and session["resolution"] == [1600, 1200]
    assert get(c, "/api/status")[1]["phase"] == "LOCKED"
    code, body = post(c, "/api/pose", {"matrix": [2.0] * 16, "location": [1, 1, 1]})
    assert code == 409
    wait_until(lambda: get(c, "/api/status")[1]["session"]["state"] == "rendered")
    code, layers = get(c, "/api/session/layers")
    assert code == 200 and layers["layers"][0]["part"] == "cube_a"
    assert post(c, "/api/unlock", {})[0] == 200

def test_lock_without_pose_is_409(running_server):
    c, _ = running_server
    code, body = post(c, "/api/lock", {})
    assert code == 409 and "no current pose" in body["error"]

def test_malformed_pose_is_400(running_server):
    c, _ = running_server
    assert post(c, "/api/pose", {"matrix": [1.0]})[0] == 400
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_server.py -q`
Expected: ImportError (`orchestrator.server`).

- [ ] **Step 3: Implement `server.py` and `__main__.py`**

One `BaseHTTPRequestHandler` subclass with a routing table; `Orchestrator` injected at construction; `log_message` silenced (ledger is the record). `__main__.py` parses `--port`, `--fake`, `--width/--height` (default 1600×1200).

- [ ] **Step 4: Run to verify they pass**

Run: `cd studio && python3 -m pytest tests/test_server.py -q`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add studio/orchestrator studio/tests/test_server.py && git commit -m "feat(orchestrator): stdlib HTTP/JSON API on 127.0.0.1:8791"
```

### Task 5: Adapter archive module (sidecar + .kra inspector)

**Files:**
- Create: `studio/adapter/core/archive.py`
- Test: `studio/tests/test_archive.py`

**Interfaces:**
- Consumes: nothing (pure stdlib).
- Produces:
  - `@dataclass PoseRecord: pose: dict; resolution: tuple[int, int]; snapshot_id: str; created: str`.
  - `write_sidecar(kra_path: pathlib.Path, poses: dict[str, PoseRecord]) -> pathlib.Path` — writes `<name>.bridge.json` next to the `.kra`; returns the sidecar path.
  - `read_sidecar(kra_path: pathlib.Path) -> dict[str, PoseRecord]` — `{}` when absent; `ValueError` on malformed JSON.
  - `@dataclass KraInfo: width: int; height: int; groups: dict[str, list[str]]` and `inspect_kra(kra_path: pathlib.Path) -> KraInfo` — parses the zip's `maindoc.xml` (std lib `zipfile` + `xml.etree`); `groups` maps group-layer name → direct child layer names, top-level paint layers under key `""`.

- [ ] **Step 1: Write the failing tests**

```python
MINIMAL_MAINDOC = """<?xml version="1.0" encoding="UTF-8"?>
<DOC image-width="1600" image-height="1200" name="t">
 <layers>
  <layer name="cube_a" node-type="paintlayer"/>
  <layer name="Model (protected)" node-type="grouplayer">
   <layer name="cube_b" node-type="paintlayer"/>
  </layer>
 </layers>
</DOC>"""

def make_kra(path):
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("maindoc.xml", MINIMAL_MAINDOC)

def test_inspect_kra(tmp_path):
    kra = tmp_path / "t.kra"
    make_kra(kra)
    info = inspect_kra(kra)
    assert (info.width, info.height) == (1600, 1200)
    assert info.groups == {"": ["cube_a"], "Model (protected)": ["cube_b"]}

def test_sidecar_roundtrip(tmp_path):
    kra = tmp_path / "t.kra"
    kra.touch()
    rec = PoseRecord(pose={"matrix": [1.0] * 16, "location": [0, 0, 0]},
                     resolution=(1600, 1200), snapshot_id="snap-0001",
                     created="2026-11-22T10:00:00")
    sidecar = write_sidecar(kra, {"s-1": rec})
    assert sidecar.name == "t.bridge.json"
    assert read_sidecar(kra) == {"s-1": rec}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_archive.py -q`
Expected: ImportError.

- [ ] **Step 3: Implement `archive.py`**

`PoseRecord` (de)serializes via `dataclasses.asdict` / explicit field parse (tuple for `resolution`). `inspect_kra` walks the `layers` element recursively one level per group; unknown node-types are ignored.

- [ ] **Step 4: Run to verify they pass**

Run: `cd studio && python3 -m pytest tests/test_archive.py -q`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add studio/adapter/core/archive.py studio/tests/test_archive.py && git commit -m "feat(adapter): .kra inspector and pose sidecar"
```

### Task 6: Adapter push logic against the Kis protocol (vertical slice core)

**Files:**
- Create: `studio/adapter/core/layout.py`, `studio/adapter/core/kis.py` (protocol), `studio/adapter/core/push.py`
- Create: `studio/tests/kisfake.py`
- Test: `studio/tests/test_push.py`

**Interfaces:**
- Consumes: Task 3 `LayerSpec`/session dict shape (as served by `/api/lock` + `/api/session/layers`), Task 5 `archive`.
- Produces:
  - `layout.py`: `MODEL_GROUP = "Model (protected)"`; `pose_group_name(pose_id: str) -> str` → `f"User — {pose_id}"`; `parse_pose_group(name: str) -> str | None`; `layer_name(part: str) -> str` → `part` verbatim.
  - `kis.py`: `class Kis(typing.Protocol): create_document(self, width: int, height: int, name: str) -> Document`; `class Document(typing.Protocol): group(self, name: str, parent=None) -> Node`; `layer_from_png(self, name: str, parent, png_path: pathlib.Path) -> Node`; `set_locked(self, node, locked: bool) -> None`; `save(self, path: pathlib.Path) -> None`.
  - `push.py`: `push_session(kis: Kis, session: dict, layers: list[LayerSpec], archives_dir: pathlib.Path) -> pathlib.Path` — creates the document at `session["resolution"]` (exact, no rescale; raises `ValueError` if a PNG's size ≠ resolution, using `pipeline/pngutil.read_size`), builds `MODEL_GROUP` at the bottom with one paint layer per `LayerSpec` (order = list order, first part = bottom), locks the group, creates the empty user pose group above it, saves to `archives_dir / f"pose_{session['id']}.kra"`, and writes the sidecar `{session["id"]: PoseRecord(...)}` via Task 5. Returns the `.kra` path.
  - `tests/kisfake.py`: in-memory `FakeKis`/`FakeDocument`/`FakeNode` implementing the protocol, recording structure for assertions.

- [ ] **Step 1: Write the failing tests**

```python
def test_push_builds_structure_and_archive(tmp_path):
    layers_dir = tmp_path / "layers"
    layers_dir.mkdir()
    pngutil.write_solid(layers_dir / "cube_a.png", 1600, 1200, (255, 0, 0))
    pngutil.write_solid(layers_dir / "cube_b.png", 1600, 1200, (0, 255, 0))
    session = {"id": "s-1", "pose": {"matrix": [1.0] * 16, "location": [0, 0, 0]},
               "resolution": [1600, 1200], "snapshot_id": "snap-0001", "state": "rendered"}
    layers = [LayerSpec("cube_a", layers_dir / "cube_a.png"),
              LayerSpec("cube_b", layers_dir / "cube_b.png")]
    kis = FakeKis()
    kra = push_session(kis, session, layers, tmp_path / "arch")
    doc = kis.documents[0]
    assert (doc.width, doc.height) == (1600, 1200)
    assert doc.top_level_names() == [MODEL_GROUP, pose_group_name("s-1")]
    assert doc.children_of(MODEL_GROUP) == ["cube_a", "cube_b"]
    assert doc.is_locked(MODEL_GROUP) is True
    assert read_sidecar(kra)["s-1"].snapshot_id == "snap-0001"

def test_push_rejects_size_mismatch(tmp_path):
    # a 800×600 layer into a 1600×1200 session must fail, never rescale
    ...  # assert pytest.raises(ValueError)
```

(`pngutil` is created in this task, not Task 9: `pipeline/pngutil.py` with `write_solid(path, w, h, rgb)` and `read_size(path) -> tuple[int, int]`, each with its own test in `tests/test_pngutil.py`; Task 9 then consumes it.)

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_push.py tests/test_pngutil.py -q`
Expected: ImportError.

- [ ] **Step 3: Implement `pngutil.py`, `layout.py`, `kis.py`, `push.py`, `tests/kisfake.py`**

`write_solid`: minimal truecolour-RGBA PNG via `zlib`+`struct` (signature, IHDR, IDAT with filter-0 rows, IEND). `read_size` parses IHDR only. `push_session` never creates intermediate directories outside `archives_dir` (created if missing).

- [ ] **Step 4: Run to verify they pass**

Run: `cd studio && python3 -m pytest tests/test_push.py tests/test_pngutil.py -q`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add studio/adapter studio/pipeline/pngutil.py studio/tests && git commit -m "feat(adapter): vertical-slice push logic against Kis protocol with fake-backed tests"
```

### Task 7: Krita docker shim + install + manual smoke

**Files:**
- Create: `studio/adapter/krita_plugin/bridge_studio/__init__.py`, `.../bridge_studio/docker.py`, `studio/adapter/krita_plugin/bridge_studio.desktop`
- Create: `studio/adapter/install.sh`
- Test: none automated (host-app shim); gate = Task 6 tests still green + manual checklist below.

**Interfaces:**
- Consumes: Task 4 HTTP API, Task 6 `push.push_session`, Task 3 session/layers JSON shapes.
- Produces: a Krita docker "Bridge" with: status label (polled `GET /api/status` every 500 ms via `QTimer`), Lock button (enabled only in adjustment with a live pose), Unlock button (enabled only while locked). Lock flow: `POST /api/lock` → poll status until `rendered` (or `failed` → label shows error) → `GET /api/session/layers` → convert the returned dicts to `LayerSpec(part, pathlib.Path(png_path))` → `push_session(real_kis, session, layers, run/archives/)` → the saved `.kra` is NOT auto-opened; the live document created by the push stays active. `real_kis` is the production `Kis` implementation wrapping `Krita.instance()` (active window `qWindow()` document creation), living in `docker.py` (stdlib + `krita` module + PyQt5 only).

- [ ] **Step 1: Write the shim**

`__init__.py` registers a `DockWidgetFactory` + `Krita` extension per Krita's pykrita convention; repo path resolution: `pathlib.Path(__file__).resolve().parents[3]` is `studio/` (install is a symlink); insert into `sys.path` before importing `adapter.*`; if resolution fails, the docker shows "install broken: not a symlink into the repo" and stays inert. All network calls via `urllib.request` with 1 s timeouts inside `try/except OSError` — any failure flips the docker to the offline state (buttons disabled, label "orchestrator offline"), never raises into Krita. Python 3.10-compatible.

- [ ] **Step 2: Install**

`install.sh`: `ln -sfn "$REPO/studio/adapter/krita_plugin/bridge_studio" "$HOME/Library/Application Support/krita/pykrita/bridge_studio"` and copy the `.desktop` next to it; prints next step (restart Krita, enable plugin in Settings → Configure Krita → Python Plugin Manager). Run it.

- [ ] **Step 3: Manual smoke checklist (record results in commit message)**

With Krita restarted and the plugin enabled:
1. Orchestrator NOT running: docker shows "orchestrator offline", buttons disabled, no errors in Krita's script console. (Review Focus 1.)
2. `cd studio && python3 -m orchestrator --fake`: within 1 s the docker shows `phase=ADJUSTMENT`.
3. `curl -X POST localhost:8791/api/pose -d '{"matrix":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"location":[0,0,0]}'`: status shows a live pose age.
4. Click Lock → within seconds status shows LOCKED then rendered; a new document 1600×1200 appears with groups `Model (protected)` (locked, layers `cube_a`,`cube_b`… whatever the fake emitted) and `User — <id>`; `run/archives/pose_<id>.kra` + `pose_<id>.bridge.json` exist on disk.

- [ ] **Step 4: Commit**

```bash
git add studio/adapter && git commit -m "feat(adapter): Krita Bridge docker shim with offline-safe polling (manual smoke: <one-line result>)"
```

### Task 8: Blender pose addon

**Files:**
- Create: `studio/blender_addon/pose.py`, `studio/blender_addon/__init__.py`
- Test: `studio/tests/test_pose.py`

**Interfaces:**
- Consumes: Task 4 `POST /api/pose`.
- Produces: `matrix_to_pose(m: collections.abc.Sequence[collections.abc.Sequence[float]]) -> dict` returning `{"matrix": [16 floats row-major], "location": [x, y, z]}`; `ValueError` unless the input is 4×4. The addon (`__init__.py`, Blender `bl_info`) registers a `bpy.app.timers` callback every 0.1 s: if the scene has a camera, POSTs `matrix_to_pose(list(map(list, camera.matrix_world)))` to `127.0.0.1:8791`; all `OSError`/`ValueError` swallowed (Review Focus 1); state transitions (online↔offline) print once to Blender's console. Install = Blender → Edit → Preferences → Add-ons → Install → select `studio/blender_addon/__init__.py` (documented in `VERSIONS.md`).

- [ ] **Step 1: Write the failing tests**

```python
def test_identity_matrix():
    pose = matrix_to_pose([[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]])
    assert pose["matrix"] == [1.0,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1]
    assert pose["location"] == [0.0, 0.0, 0.0]

def test_translation_extracted():
    pose = matrix_to_pose([[1,0,0,3.5],[0,1,0,-2],[0,0,1,7],[0,0,0,1]])
    assert pose["location"] == [3.5, -2.0, 7.0]

def test_rejects_bad_shape():
    with pytest.raises(ValueError):
        matrix_to_pose([[1,0,0],[0,1,0],[0,0,1]])
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_pose.py -q`
Expected: ImportError.

- [ ] **Step 3: Implement `pose.py` and the addon**

`matrix_to_pose` validates shape, flattens row-major, takes translation from column 4 rows 1–3. The addon timer keeps a module-level `_last_ok` flag for the one-line console transition messages.

- [ ] **Step 4: Run to verify they pass**

Run: `cd studio && python3 -m pytest tests/test_pose.py -q`
Expected: 3 passed.

- [ ] **Step 5: Manual smoke**

Start `python3 -m orchestrator --fake`; in Blender install+enable the addon, move the camera, then check `run/ledger.jsonl` shows `pose_stream` entries with growing counts. Stop Blender with the orchestrator off: no exceptions in Blender's console. Record one line in the commit message.

- [ ] **Step 6: Commit**

```bash
git add studio/blender_addon studio/tests/test_pose.py && git commit -m "feat(blender): pose-stream addon (manual smoke: <one-line result>)"
```

### Task 9: Container pipeline stub (import + per-part render)

**Files:**
- Create: `studio/pipeline/Dockerfile`, `studio/pipeline/import_scene.py`, `studio/pipeline/render_layers.py`
- Create: `studio/pipeline/docker_pipeline.py`
- Create: `studio/pipeline/fixtures/two_parts.obj`
- Test: `studio/tests/test_docker_pipeline.py` (marked `docker`)

**Interfaces:**
- Consumes: Task 3 `Pipeline` protocol, `Session`, `LayerSpec`; Task 6 `pngutil`.
- Produces:
  - `fixtures/two_parts.obj`: two unit cubes as objects `cube_a`, `cube_b` (hand-written, offset ±1 on X).
  - `Dockerfile` (build tag `bridge-studio-blender:5.2.3`): `FROM --platform=linux/arm64 ubuntu:24.04`; install pinned `blender-5.2.3-linux-arm64.tar.xz` from `https://download.blender.org/release/Blender5.2/` with checksum verified against the published `.sha256` at build time; `apt` runtime libs only as Blender requires; `ENTRYPOINT ["blender", "--background"]`.
  - `import_scene.py` (runs in container): argv `-- <input.obj> <out_dir>` → imports OBJ, saves `<out_dir>/scene.blend`, writes `<out_dir>/parts.json` (`{"parts": [<object names>]}`).
  - `render_layers.py` (in container): argv `-- <scene.blend> <parts.json> <pose.json> <out_dir> <width> <height>` → sets camera `matrix_world` from pose, engine CYCLES CPU, 8 samples, film transparent, then per part: solo-visible render to `<out_dir>/<part>.png` (RGBA).
  - `class DockerPipeline:` `__init__(self, image: str = "bridge-studio-blender:5.2.3")`; `render(self, session: Session, scene_dir: pathlib.Path, out_dir: pathlib.Path) -> list[LayerSpec]` — one `docker run` per phase (import happens in Task 10's watcher path via `import_asset(self, src: pathlib.Path, out_dir: pathlib.Path) -> None`), bind-mounting the run dirs, returning `[LayerSpec(part, out_dir / f"{part}.png")]` ordered as `parts.json`.

- [ ] **Step 1: Write the failing test**

```python
@pytest.mark.docker
def test_import_and_render_two_parts(tmp_path):
    pipe = DockerPipeline()
    scene_dir = tmp_path / "scene"
    pipe.import_asset(FIXTURE_OBJ, scene_dir)
    parts = json.loads((scene_dir / "parts.json").read_text())["parts"]
    assert parts == ["cube_a", "cube_b"]
    session = Session(id="t-1", pose={"matrix": TEST_POSE_4X4, "location": [0, 0, 0]},
                      resolution=(320, 240), snapshot_id="snap-0001", state="rendering")
    layers = pipe.render(session, scene_dir, tmp_path / "layers")
    assert [l.part for l in layers] == ["cube_a", "cube_b"]
    assert [pngutil.read_size(l.png_path) for l in layers] == [(320, 240), (320, 240)]
```

`TEST_POSE_4X4`: a camera at (0,-6,2) looking at origin (build with mathutils-free math in the test — a fixed literal matrix computed offline and recorded in the test file).

- [ ] **Step 2: Verify it fails / daemon up**

Run: `open -a Docker` (start Docker Desktop), wait for `docker version` to succeed; then `cd studio && STUDIO_DOCKER=1 python3 -m pytest tests/test_docker_pipeline.py -q`
Expected: FAIL (image missing / scripts missing). If Docker Desktop cannot start on this machine, STOP and escalate to the user — do not substitute a native run.

- [ ] **Step 3: Implement Dockerfile, scripts, `DockerPipeline`, fixture**

`docker_pipeline.py` builds `docker run --rm --platform linux/arm64 -v ...` command lines, captures stdout/stderr into the ledger via the caller (Orchestrator already records `render_done`; `DockerPipeline` raises `RuntimeError` with tail of stderr on non-zero exit).

- [ ] **Step 4: Run to verify it passes**

Run: `cd studio && STUDIO_DOCKER=1 python3 -m pytest tests/test_docker_pipeline.py -q`
Expected: 1 passed (first build takes minutes; record build + render wall time in the commit message).

- [ ] **Step 5: Flip `__main__.py` default to `DockerPipeline`, re-run full gate**

Run: `cd studio && python3 -m pytest tests/ -q`
Expected: all pass (docker test skipped without the env var).

- [ ] **Step 6: Commit**

```bash
git add studio/pipeline studio/orchestrator/__main__.py studio/tests && git commit -m "feat(pipeline): containerized Blender import + per-part Cycles render stub (build Xs, render Ys)"
```

### Task 10: Folder watcher and scene snapshots

**Files:**
- Create: `studio/orchestrator/watch.py`, `studio/orchestrator/scenes.py`
- Modify: `studio/orchestrator/app.py` (Orchestrator gains optional kwarg `scenes: SceneStore | None = None`; `status()["snapshot_id"]` becomes live when a store is wired; watcher thread start/stop)
- Test: `studio/tests/test_watch.py`

**Interfaces:**
- Consumes: Task 9 `DockerPipeline.import_asset` (injectable for tests), Task 3 `Orchestrator`, Task 2 `Ledger`.
- Produces:
  - `@dataclass Snapshot: id: str; dir: pathlib.Path; parts: list[str]`; `class SceneStore: __init__(self, root: pathlib.Path)`; `current -> Snapshot | None`; `new_snapshot(self) -> Snapshot` — creates `root/snap-NNNN/` (zero-padded, incrementing, never reusing) with empty `parts`.
  - `class Watcher: __init__(self, inbox: pathlib.Path, store: SceneStore, import_asset: Callable[[pathlib.Path, pathlib.Path], None], ledger: Ledger, poll_s: float = 1.0, settle_s: float = 0.5)`; `start(self)`, `stop(self)`. On a size-stable `.obj`/`.stp`/`.step` file in the inbox: create the snapshot, move the file into its `source/`, call `import_asset`, fill `snapshot.parts` from the produced `parts.json`, set `store.current`, record `file_drop` and `import_done` (with `bytes_moved`, `latency_ms`).
  - `Orchestrator.lock()` binds the session to `store.current.id`. With no store wired (`scenes=None`, as in Task 3's tests) nothing changes; with a store wired but empty, lock raises `InvalidTransition("no scene imported")`.

- [ ] **Step 1: Write the failing tests**

```python
def test_stable_file_triggers_import(tmp_path):
    store = SceneStore(tmp_path / "scene")
    imports = []
    w = Watcher(tmp_path / "inbox", store, lambda src, out: imports.append((src, out)),
                Ledger(tmp_path / "l.jsonl"), poll_s=0.05, settle_s=0.05)
    w.start()
    (tmp_path / "inbox" / "two_parts.obj").write_text("o cube_a\n")
    wait_until(lambda: len(imports) == 1, timeout=3)
    w.stop()
    assert store.current is not None and store.current.id == "snap-0001"
    src = imports[0][0]
    assert src.parent.name == "source" and src.exists()  # moved, not copied

def test_partial_file_waits_for_stability(tmp_path):
    # writer appends between the watcher's two size checks -> no import yet
    ...  # assert imports == [] while appends continue; import fires once stable

def test_redrop_creates_new_snapshot_never_overwrites(tmp_path):
    ...  # two drops -> snap-0001, snap-0002; both source files exist

def test_lock_without_scene_raises(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       scenes=SceneStore(tmp_path / "scene"))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    with pytest.raises(InvalidTransition):
        app.lock()
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd studio && python3 -m pytest tests/test_watch.py -q`
Expected: ImportError / TypeError (new `scenes` kwarg).

- [ ] **Step 3: Implement `scenes.py`, `watch.py`, wire into `app.py` and `__main__.py`**

Watcher is a daemon thread polling the inbox; "size-stable" = identical size across `settle_s`. `__main__.py` gains `--inbox studio/run/inbox --scene-root studio/run/scene`; watcher starts with the server.

- [ ] **Step 4: Run to verify they pass, then full gate**

Run: `cd studio && python3 -m pytest tests/ -q`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add studio/orchestrator studio/tests/test_watch.py && git commit -m "feat(orchestrator): inbox watcher with snapshot versioning"
```

### Task 11: End-to-end validation on the reference machine

**Files:**
- Create: `studio/validation/2026-11-22-walking-skeleton.md`
- Test: none new; this task is the recorded live run.

**Interfaces:**
- Consumes: everything above.
- Produces: the recorded evidence closing the spec's "Krita API surface" risk and proving the walking skeleton.

- [ ] **Step 1: Full live run**

Docker Desktop running; `cd studio && python3 -m orchestrator` (real pipeline). Drop `pipeline/fixtures/two_parts.obj` into `studio/run/inbox/` → `snap-0001` appears with `scene.blend` + `parts.json`. Open the scene in Blender (addon enabled) → orbit → ledger shows `pose_stream`. In Krita (PadMac/PadPad input path up per the gate doc), docker shows adjustment + pose age → click Lock → container renders → document appears: `Model (protected)` locked group with `cube_a`, `cube_b` layers at 1600×1200, `User — <id>` group above.

- [ ] **Step 2: Paint and archive readback**

User paints one pressure stroke on a new layer inside the user group (injector path). Save. Close the document. Reopen `run/archives/pose_<id>.kra`; verify with the adapter's own tool: `cd studio && python3 -c "from adapter.core.archive import inspect_kra, read_sidecar; import pathlib; p = pathlib.Path('run/archives/pose_<id>.kra'); print(inspect_kra(p)); print(read_sidecar(p))"` — groups, layer names, 1600×1200, and the pose record must print intact.

- [ ] **Step 3: Record evidence**

`studio/validation/2026-11-22-walking-skeleton.md`: checklist of Steps 1–2 with observed outcomes; ledger excerpt (`render_done` latency, byte counts); statement closing the spec's "Krita API surface" risk; any deviation recorded as data (not silently fixed).

- [ ] **Step 4: Commit**

```bash
git add studio/validation && git commit -m "validation: walking skeleton end-to-end on reference machine"
```

---

## Self-review notes (author's checklist, delete nothing)

- Spec coverage: phase machine T3/4; docker UI (lock/unlock/status) T7; per-pose user groups T5/6 (restore-on-return + history UI → Plan 2, stated in Sequencing); protected group T6 (restore-on-save guard → Plan 2); adjustment streaming T3/4/8; watch folder + containers T9/10; fixed resolution T6/9; ledger everywhere T2+; presentation opening = `status()["roster"]` T3/4; archive scoping: snapshot versioning T10 (auto-export → Plan 2). Deferred items are named in Sequencing, not dropped silently.
- Type consistency: `LayerSpec`, `Session`, `PoseRecord`, `KraInfo`, `Snapshot`, `Kis`/`Document` protocol names are identical across Tasks 3–11; `/api/session/layers` (no id path param — single active session in Plan 1).
- Review Focus items 1–5 are pinned in T7, T4, T3/T4, T10, T6/T9 respectively.
