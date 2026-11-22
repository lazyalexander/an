# Walking skeleton — end-to-end validation (reference machine)

Date: 2026-11-22 → 2026-11-23 (Asia/Shanghai). Branch: `dev-bridge-studio`.
Machine: Apple Silicon (arm64, 18 cores), macOS 27.2.1; Apple Container 1.7.1
(`--platform linux/amd64 --rosetta`), Blender 5.2.3 LTS, Krita 5.3.4, Python 3.13.12.

Runtime note: the plan text says "Docker Desktop running"; the runtime is Apple
Container per the Task 9 ledger ruling (amd64 containers wedge under Docker
Desktop on this host). All container steps below ran on Apple Container.

## Step 1 — full live run: checklist

| Step | Observed outcome |
|---|---|
| `cd studio && python3 -m orchestrator` (real pipeline) | Started on 127.0.0.1:8791; `/api/status` → `phase=ADJUSTMENT`, roster `{adjustment: Blender, locked: Krita}`. |
| Drop `pipeline/fixtures/two_parts.obj` into `studio/run/inbox/` | Watcher moved it to `run/scene/snap-*/source/`, container import produced `scene.blend` (87 KB) + `parts.json` (`["cube_a","cube_b"]`). Four imports observed: snap-0001/0002/0003/0004, `import_done` latency 2689 / 2694 / 2808 / 2862 ms. |
| Open scene in Blender (addon enabled) → orbit → `pose_stream` | Opened `snap-0002/scene.blend` in Blender GUI with addon enabled. Ledger recorded `pose_stream` aggregates "10 poses in 1s window" continuously (214 records over the session). Orbited the camera 60° around Z via the GUI Python console: console printed `ORBITED [4.307, 1.154, 2.207]` (import default was `[3.15, -3.15, 2.21]`). |
| Krita docker shows adjustment + pose age | Bridge docker showed `phase=ADJUSTMENT pose_age=49ms…94ms` while streaming. |
| Click Lock → container renders | Session `20261123-014105-8b8ddd` frozen at the streamed pose — `/api/status` pose.location `[4.3067, 1.1540, 2.2069]`, exactly the orbited camera. First attempt (session `…-dcf857`) failed `exit 137` (SIGKILL, container default memory too small for 1600×1200 Cycles); fixed by constructing `ContainerPipeline(memory="4g", cpus=4)` — see Deviations. Retry rendered: `render_done … 2 layers, bytes_moved=1337197, latency_ms=56553`. |
| Document appears | Krita opened `pose_20261123-014105-8b8ddd.kra` (29.7 MiB) at 1600×1200: `User — 20261123-014105-8b8ddd` group above, `Model (protected)` group locked (padlock) with `cube_b` over `cube_a` (first part at bottom). Layer PNGs verified on disk: `cube_a` 217,475 opaque px in bbox (651,186,1127,718); `cube_b` 466,975 opaque px in bbox (257,453,988,1200) — both parts rendered at 1600×1200 RGBA. |

## Step 2 — paint and archive readback

- User painted one pressure stroke on a new `Paint Layer 1` inside the
  `User — 20261123-014105-8b8ddd` group (injector path, per the pressure-gate
  doc), then saved (Cmd+S). `.kra` on disk: 3,708,967 bytes, mtime 01:59.
- Document closed and reopened from `run/archives/pose_20261123-014105-8b8ddd.kra`:
  layer tree intact — `User …` group with `Paint Layer 1` (stroke visible in
  thumbnail), `Model (protected)` locked with `cube_b`, `cube_a`.
- Adapter's own tool on the saved file:

```
inspect_kra → KraInfo(width=1600, height=1200,
  groups={'': [], 'User — 20261123-014105-8b8ddd': ['Paint Layer 1'],
          'Model (protected)': ['cube_b', 'cube_a']},
  locked_groups=frozenset({'Model (protected)'}))
read_sidecar → {'20261123-014105-8b8ddd': PoseRecord(
  pose={'matrix': [-0.258819…, …], 'location': [4.3067, 1.1540, 2.2069]},
  resolution=(1600, 1200), snapshot_id='snap-0004',
  created='2026-11-22T17:42:02+00:00')}
```

  Groups, layer names, 1600×1200, locked group, and the pose record all print
  intact — the frozen pose in the archive is byte-identical to the orbited
  camera.

## Ledger excerpt (kinds counted this run)

`pose_stream` 214, `lock` 4, `render_done` 3, `unlock` 1, `file_drop` 4,
`import_done` 4, `render_failed` 1. Every boundary crossing carries
bytes/latency: imports ~2.7–2.9 s @ 489 B, render 56.55 s @ 1,337,197 B,
pose stream 10 poses/s @ 0 B (in-process aggregate).

## "Krita API surface" risk — closed

The spec's risk ("document/layer CRUD, pixel transfer, and docker UI are
documented and sufficient **on paper**") is now exercised end to end on the
reference machine: the adapter created a document from rendered PNGs,
structured locked/unlocked groups, pushed pixels at 1600×1200, persisted
pose metadata in a sidecar inside the `.kra`, survived save → close →
reopen with user edits, and read everything back with its own parser. The
API surface is sufficient **in fact** for the walking skeleton. (CI headless
operation remains limited by Krita itself — that is why the testable core /
thin shim split exists; unchanged.)

## Deviations recorded as data

1. **`orchestrator/__main__.py` crashed on startup** (`NameError: Path`).
   Plan's Task 10 tests never exercised the argparse defaults. Fixed with a
   CLI `--help` subprocess smoke test (`tests/test_main.py`), RED→GREEN.
2. **Imported scenes had no camera**, so the pose stream could never start —
   spec assumes the user opens the generated scene and the camera pose
   streams. `import_scene.py` now adds a default `BridgeView` camera framing
   the imported meshes (bounding-box center, 3× radius). Container probe
   test guards it.
3. **The camera probe test first passed vacuously**: Blender exits 0 when
   `--python-expr` raises, unless `--python-exit-code 1` is passed. Test
   fixed and watched RED before the implementation fix. Recorded as a
   standing lesson for Blender-in-container assertions.
4. **Full-resolution render OOM** (`exit 137`) at the container default
   memory; `ContainerPipeline` now takes `memory="4g", cpus=4` constructor
   parameters (host capacity as configuration, not a buried constant). A
   production-resolution render test (1600×1200) joins the container gate.
5. `render_layers.py` line 44 emits `DeprecationWarning: 'World.use_nodes'
   is expected to be removed in Blender 6.0` — recorded, not fixed (pinned
   Blender is 5.2.3 LTS).
6. At the validated orbited pose, `cube_b`'s render extends to the bottom
   frame edge (bbox y=1200). Geometry is correct per-part; framing is the
   artist's camera, recorded as data.
