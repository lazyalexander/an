# Incremental tasks in An Studio

An Studio serves a task on loopback. The browser hosts its admitted panels. Apple Container runs every business operation in a pinned spool, including document editing, save verification, model generation and export. The Rust host owns scheduling, versions and authorization. Python serves the host projection and the browser never receives the harness turn credential.

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
| `expand` | Runs a pinned, read-only spool in a dedicated short-lived instance with the same input authorization as a run. Declare the single output `subgraph.json`. Write an object with a nonempty `nodes` array there. After confirmed driver cleanup, An requires output authorization for the exact proposal revision before copying its contents into graph metadata. The workspace flow is `["publish", driverSpool, "graph-metadata"]`. The cumulative per-task limit is configurable and defaults to 256 nodes. |
| `model` | A granted model alias, one `request` binding containing `request.json`, and one output `result.json`. An runs local inference on the host. |
| `surface` | Use `spool` and `panel` to select an admitted decision. Bind the declared input names. Declare the `response` output. For a verified save, add a `document` output and `save: {"node":"save-run","port":"file.ora"}`. The designated run can consume the provisional response. To generate the save run after the user responds, set `continuation` to a declared expand node bound to this response. The saver ID is reserved for this continuation, whose proposal must include it. Other consumers wait for the captured save. |
| `delivery` | Bind the files to publish with `inputs`, use an empty `outputs` list and optionally select a declared presentation using `spool` and `panel`. The group is published after authorization for all its members. |

A repeated identical submission returns its original receipt. A stale base returns `status: conflict` and the current revision. A submission can include `retry` for failed or cancelled node IDs and `supersede` for unconsumed queued nodes or waiting surfaces. Submit replacement nodes under new IDs. An unknown run is reconciled by its reserved ID and kept for investigation.

Read `an_graph_state` with `after` set to the last event cursor. The result includes the current graph, activity, revision, cursor and later events. A container exit, an approval, a response or reconnection wakes scheduling. Task and SQLite locks serialize concurrent updates. Business responses and harness permission decisions use separate APIs.

## Host model calls

Grant an installed local model when creating a task, for example `an task new --model local --model-calls 4 --input request.json`.
The task snapshots its alias, model name and quota. Existing tasks and tasks created without a grant have no model access.
A driver can propose the following node and bind its result to a subsequent expand node:

```json
{"id":"decision","kind":"model","model":"local","inputs":{"request":{"node":"prompt","port":"request.json"}},"outputs":["result.json"]}
```

The request file contains one `messages` array of text messages with `role` and `content`. The result contains a `text` string. The host checks the resolved model with a metadata-only Ollama show request before sending any prompt. It requires local weight metadata, rejects remote models, and uses Ollama’s explicit `:local` routing suffix on the generation request. The result and usage are committed before the optional running-model digest lookup. For a workspace with explicit flow rules, permit `["model", inputOrigin, "model:local"]` for inference and `["handoff","model:local",recipientSpool]` for the downstream container.

Each authorized attempt reserves a task slot before sending. The graph state includes a `model_calls` ledger with attempts, revision IDs, send and settlement timestamps, the model name, digest when observed in the running model list, and reported token counts. The evidence API supports separately authorized reads of model input and output. A sending attempt whose worker disappeared or whose response could not be confirmed becomes `unknown`; usage remains unknown. Explicit retry uses a fresh attempt and slot. A reserved attempt can resume before sending. Settled attempts reuse the captured result. Stop and turn expiry suppress downstream work; a request already sent may still complete on the host.

With Ollama serving the configured local model, run `python3 examples/graph_studio/demo.py bridge-model` with the example image. Select the painting and enter an instruction such as “attach a separate annotation text file” or “only deliver the painting”. A follow-up driver reads the authorized model decision and selects the delivery contents. Run `python3 tests/model_node_smoke.py` for counted backend recovery tests and `python3 tests/graph_studio_smoke.py --experiment bridge-model --browser` for the real model path.

## Person controls

The person controls a task through the host, never through the agent. Pass `{"action":"pause"}`, `{"action":"resume"}` or `{"action":"stop"}` to `an graph --task TASK control` with the current `AN_TURN`; Studio shows the same actions as header buttons. Controls are not part of the agent's MCP tools.

`pause` starts no new work: queued nodes and pending permission requests wait, while running containers are still observed until they settle. The agent may keep submitting; new nodes queue. `resume` releases the pause. `stop` cancels every unsettled node, requests cancellation of running work, unloads idle drivers and leaves the task paused. After `resume`, the agent can `retry` cancelled nodes. Each control is recorded as a `user` event (`paused`, `resumed`, `stopped`), and `an_graph_state` and the host view report `control`.

## Instructions

The person can type to the agent. Pass `{"text": ..., "revision": N}` to `an graph --task TASK instruct` with the current `AN_TURN`, where `N` is the graph revision the person was looking at; Studio sends it from the box under the work area. An relays the text unchanged and never interprets it. The agent reads it from `an_graph_state` as an event with `actor: "user"`, `kind: "instruction"` and data `text`, `seen_revision`, `revision` and `stale`. `stale` is true when the graph moved on after the person looked. An instruction never answers a waiting surface; the agent may supersede that surface instead. Controls stay separate buttons. The size limit is `instruction_max_bytes` (see [Configuration](configuration.md)).

## Declared panels and terminal decisions

For a new surface submit a pinned spool reference, a panel ID and artifact bindings. The host derives the response contract when accepting the graph. A read-only delivery can select a declared delivery panel or publish files directly. Previously stored graphs and deliveries retain their original package and request identity.

Studio opens the basic form and offers the admitted custom view through **专用面板**. For example, choose 2 in the CAD timing form or answer the same waiting request from a terminal with the active task turn:

```sh
an decision list --task "$task"
an decision answer --task "$task" --node timing
printf '%s\n' '{"seconds":2}' | an decision answer --task "$task" --node timing --request "$request" --json -
```

The list includes the attempt ID, permitted fields and bound artifact versions. Pass the listed request ID when answering a previously inspected decision. Both input modes record the same viewed and responded events. Preview and handoff approvals still use the normal permission commands.

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

## Krita native painting

Build the example spool as described above, install the Krita adapter with `examples/legacy_studio/adapter/install.sh` and enable the Bridge Studio Python plugin in Krita. Open the **An · 原生绘画** docker. Run the demo with `bridge-native` and select the printed `connect.json` using **连接任务…**. The example follows the Bridge comparison with a native paint step and publishes the returned KRA.

A task can also run its adapter independently of Studio:

```sh
an native serve --task "$task"
an decision list --task "$task"
```

The channel uses a private same-user Unix socket and the server's current task authority. A declaration of `kind: native` requires `adapter: krita` and `bindings: [document]`. Its graph node uses `kind: native`, the admitted spool and panel ID, and `outputs: ["document"]`. The input revision must contain one ORA or KRA. Authorize preview to `native:krita`; delivery publication and export use `native:krita` as the saved file's origin.

Each attempt has a separate working directory under the An registry. Ordinary saves stay there. **完成并交付** only acts on that attempt's active document. A transport interruption can resend its snapshot. Stop closes the request and preserves the working copy. Restart the channel under the same task and active turn to reconnect.

For automated end-to-end validation run `python3 tests/native_paint_smoke.py --image IMAGE` on the reference machine. This uses a fresh synthetic workspace and Krita's bundled script runner. For the physical gate add `--pencil --directory NEW_EVIDENCE_DIR`. Connect the iPad through Codonic Pad and draw one horizontal light-to-heavy stroke on the prepared transparent Pencil layer. Use ordinary Save, pause briefly, then **完成并交付**. The gate records Qt tablet pressure for this attempt and reopens the exported KRA to read the independent layer's pixels, column widths and digest. Keep the evidence directory for verification.

### Markdown review

Build the example with `container build -t an-markdown-review:headless examples/markdown_review/spool`.
Run `python3 examples/graph_studio/demo.py markdown --review-case conflict` and open the Studio URL.
The fixture supplies the original Markdown and two paragraph replacement suggestions. The local model classifies the pair. Conflicting replacements open a declared choice with both edits and reasons. The compatible fixture proceeds directly to delivery. Both paths publish the revised Markdown and a unified diff.

The central timeline shows instructions, decisions, deliveries, failures and controls in host event order. Select a waiting decision to answer inline. Inspect execution details through the graph and sources controls. After the task closes its published delivery remains accessible.

Run `python3 tests/markdown_review_smoke.py --browser` for the controlled backend and desktop/mobile Chrome checks. Add `--real` to use the configured local Ollama model. Both modes exercise the compatible case and both conflict answers, and apply each diff to verify the delivered bytes.
