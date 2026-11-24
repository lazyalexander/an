# Incremental tasks in An Studio

An Studio serves a task on loopback. The browser hosts its generated panels. Apple Container runs every business operation in a pinned spool, including document editing, save verification, model generation and export. The Rust host owns scheduling, versions and authorization. Python serves the host projection and the browser never receives the harness turn credential.

## Run an experiment

With Apple Container running, build the spool and start the Bridge experiment from the repository root.

```sh
cargo build --locked
container build --tag an-graph-studio:headless examples/graph_studio/spool
python3 examples/graph_studio/demo.py bridge
```

Open the printed Studio URL. Approve the preview requests, select a reference and click the location to mark. A second headless driver reads the frozen response and proposes the processing node. The initial driver has already exited. The spool writes a layered OpenRaster document, reopens its archive to check the saved layers and returns its bytes. The surface completes when An captures that output. The report includes the painting, annotated SVG and choice. Its **交付文件** section downloads the fixed versions.

Run the same command with `cad` to set the rotation period of a chamfered plate and deliver a binary STL and animated SVG. Both examples unload their instances and close their task after publication. The page stays available until interrupted. The same task can be reopened later with its ID.

```sh
(cd studio && python3 -m orchestrator --task TASK_ID --an ../target/debug/an)
```

A fresh workspace task can open an earlier delivery. Studio's **从此版本新建任务** action and `an task resume DELIVERY_ID` make the published files available as the new task's `original` input. Set `AN_HOME` consistently across processes when using a separate registry. Supply the current harness-issued `AN_TURN` to Studio for live decisions and private evidence reads. Finished tasks serve their published packages without a running spool.

## Agent contract

Call `an_graph_submit` with `base_revision`, `idempotency_key` and a `nodes` array. Each node has an immutable `id`, optional display `title`, a `kind` and an `outputs` list. Use `after` for ordering and `inputs` for data bindings. A binding is either `{"input":"task-input-name"}` or `{"node":"upstream-id","port":"output-name"}`.

| Kind | Fields and behavior |
| --- | --- |
| `run` | Supply a granted digest-pinned `spool`, `argv` and output paths relative to `/output`. Input names match the admitted manifest. Each port receives its bound revision under `/input/<port>/`. Read-only spools support concurrent instances. Editable work is serialized. |
| `expand` | Runs a pinned, read-only spool in a dedicated short-lived instance with the same input authorization as a run. Declare the single output `subgraph.json`. Write an object with a nonempty `nodes` array there. After confirmed driver cleanup, An requires output authorization for the exact proposal revision before copying its contents into graph metadata. The workspace flow is `["publish", driverSpool, "graph-metadata"]`. Accepted proposals can add up to 256 nodes cumulatively per task. |
| `surface` | `package` names an input binding. `response` declares an object schema with declared properties, array items, scalar types, optional enumerations and required keys. Declare the `response` output. For a verified save, add a `document` output and `save: {"node":"save-run","port":"file.ora"}`. The designated run can consume the provisional response. To generate the save run after the user responds, set `continuation` to a declared expand node bound to this response. The saver ID is reserved for this continuation, whose proposal must include it. Other consumers wait for the captured save. |
| `delivery` | Bind the files to publish with `inputs`, use an empty `outputs` list and optionally name the interactive `package` binding. The group is published after authorization for all its members. |

A repeated identical submission returns its original receipt. A stale base returns `status: conflict` and the current revision. A submission can include `retry` for failed or cancelled node IDs and `supersede` for unconsumed queued nodes or waiting surfaces. Submit replacement nodes under new IDs. An unknown run is reconciled by its reserved ID and kept for investigation.

Read `an_graph_state` with `after` set to the last event cursor. The result includes the current graph, activity, revision, cursor and later events. A container exit, an approval, a response or reconnection wakes scheduling. Task and SQLite locks serialize concurrent updates. Business responses and harness permission decisions use separate APIs.

## Person controls

The person controls a task through the host, never through the agent. Pass `{"action":"pause"}`, `{"action":"resume"}` or `{"action":"stop"}` to `an graph --task TASK control` with the current `AN_TURN`; Studio shows the same actions as header buttons. Controls are not part of the agent's MCP tools.

`pause` starts no new work: queued nodes and pending permission requests wait, while running containers are still observed until they settle. The agent may keep submitting; new nodes queue. `resume` releases the pause. `stop` cancels every unsettled node, requests cancellation of running work, unloads idle drivers and leaves the task paused. After `resume`, the agent can `retry` cancelled nodes. Each control is recorded as a `user` event (`paused`, `resumed`, `stopped`), and `an_graph_state` and the host view report `control`.

## Instructions

The person can type to the agent. Pass `{"text": ..., "revision": N}` to `an graph --task TASK instruct` with the current `AN_TURN`, where `N` is the graph revision the person was looking at; Studio sends it from the box under the work area. An relays the text unchanged and never interprets it. The agent reads it from `an_graph_state` as an event with `actor: "user"`, `kind: "instruction"` and data `text`, `seen_revision`, `revision` and `stale`. `stale` is true when the graph moved on after the person looked. An instruction never answers a waiting surface; the agent may supersede that surface instead. Controls stay separate buttons. The size limit is `instruction_max_bytes` (see [Configuration](configuration.md)).

## Panel packages

A package is a single JSON file produced by a run or imported as a task input. This package presents one confirmation button.

```json
{
  "version": 1,
  "entry": "index.html",
  "files": {
    "index.html": "<button id='accept'>Accept</button><script src='panel.js'></script>",
    "panel.js": "addEventListener('an-ready', () => { document.querySelector('#accept').onclick = () => an.respond({accepted:true}) })"
  },
  "bindings": [],
  "response": {
    "type": "object",
    "properties": {"accepted": {"type": "boolean"}},
    "required": ["accepted"]
  }
}
```

The entry can include inline HTML, classic JavaScript and CSS, or name scripts and stylesheets in `files`. Declare each data binding except the package itself in `bindings`. Interactive deliveries use the same format and set `response` to null.

The shell creates an opaque-origin iframe, connects a private message port, then delivers the package. On `an-ready`, `an.files` contains only the named file sets, with base64 file entries. Use `an.draft`, `an.saveDraft(value)` and `an.respond(value)` for a waiting surface. Polling preserves the mounted frame. A new request ID binds a new edit session. The host validates response shape, current authority and request identity and records successful responses once.

Workspace flow rules use `["preview",origin,"studio"]` for viewing and `["publish",origin,workspace]` for delivery. Origins include image references, `response` and `task-input:<name>`. Container inputs from upstream runs require handoff permission. Code snapshots, inputs and log reads use their own host decisions. Evidence for older admissions can lack a code preview. Large image code files are identified as omissions alongside the immutable image digest.

Use `an export DELIVERY_ID DESTINATION` to export the published revision. For a closed graph task, pass `--task ACTIVE_TASK_ID` with a current turn in the same workspace. Workspace policies must permit `["export",origin,destination]` for every constituent origin. The host approval covers the complete delivery and its destination.

The legacy instance-mode Studio reads its original ledger and archives. It is frozen as the reference for native surfaces; see [Architecture](architecture.md). Use task mode for the headless graph workflow.


## Reproduce acceptance

Run `python3 tests/graph_studio_smoke.py --experiment bridge` and repeat with `--experiment cad` after building the example image. Each test creates an isolated registry. The Bridge test checks overlapping execution intervals on distinct candidate instances and verifies that the second driver generates the processing node after the human response. Use `--choice A` to cover denied handoff and retry. Both tests close the task and check the published files and authorized evidence from a fresh workspace task.

Pass `--browser` with Chrome and `playwright-core` installed to exercise the macro UI. Set `PLAYWRIGHT_CORE` to the module directory if it is outside Node's search path. Use `--directory` to retain the test registry and desktop/mobile screenshots. `cargo test` also covers a seeded indeterminate run, denied access, damaged package data, duplicate responses and expired requests.

To add a second mark to a Bridge V1 made with the revision-capable image, run
`python3 examples/graph_studio/demo.py bridge --resume V1_DELIVERY_ID` with the same registry and image.
Approve input access, click a new point on the V1 preview, and save. The new delivery contains the SVG with both marks and an OpenRaster painting with separate annotation layers. The original delivery remains available under its own ID.
