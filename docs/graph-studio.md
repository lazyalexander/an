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

For the proposed reviewer interface, including how a harness resumes work from receipts and supplies frozen materials to a subagent, see [How agents access An (中文)](agent-access.zh-CN.md).

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

## Starting from a sentence

Run Studio for a workspace instead of a task: `(cd studio && python3 -m orchestrator --workspace WORKSPACE_ID --an ../target/debug/an)`. The page opens on a text box. Typing a sentence binds a fresh session, creates the task, begins its turn and records the sentence as the task's first instruction. A detached local controller owns the turn, MCP configuration and runner. Restarting Studio reconnects to the controller for the same registry, workspace and harness. From the `studio` directory, the operator can also use `python3 -m an_host --workspace WORKSPACE_ID --an ../target/debug/an serve` to launch it headless, `start "instruction"` to create a task, `agent TASK_ID` to print its private MCP configuration, and `stop` to shut down that controller. Use the same binary and agent configuration across clients. Workspace task credentials are stored under `$AN_HOME/controllers/` and the browser receives only the task ID. Use **新任务** to return to the text box.

## Image model nodes

An image alias in `model_aliases` (see [Configuration](configuration.md)) makes a granted task's `model` nodes produce images: `an task new --model sketch-local` or `--model sketch-remote`. Containers have no GPU, so inference stays on the host like text models, under the same authorization, quota, at-most-once sending and evidence.

An image node binds `request` (`request.json` with `prompt` and optional `size` of `1024x1024`, `1536x1024` or `1024x1536`) and optionally `image` (one PNG, for example a sketch rendered as dark lines on white), and declares the output `result.png`. All bound ports are approved together.

- **`local`** calls a loopback image service. `services/image_local.py` keeps SDXL base 1.0 with the xinsir ControlNet scribble model resident on Apple Silicon; run it with a Python environment that has torch (MPS), diffusers, transformers and accelerate and the weights in the Hugging Face cache. An unreachable service fails the node without sending.
- **`openai`** sends the request to the OpenAI Images API (`edits` with an image, `generations` without). An reads the key from the macOS Keychain entry named by `keychain` (account `an`) at send time and passes it to curl through a private 0600 header file; it never enters the registry, logs, process arguments or containers. Store it with `security add-generic-password -U -a an -s an.openai -w KEY`.

The sketch experiment uses this path: `driver.py sketch SPOOL --model=ALIAS` adds a container step that renders the strokes to `control.png` and writes the prompt from the person's note, then the image node, and delivers the sketch, the line art and the painting. `python3 tests/sketch_smoke.py --model sketch-local` runs it end to end.

## Agent runner

The local controller wakes the task's agent when `agent.command` is set in its configuration (see [Configuration](configuration.md)). Instructions, surface answers and task resumes can wake the agent. If the agent proposed a scope and returned before submitting the graph, deciding the scope also wakes it. The `intent_decided` event carries the original `instruction_cursor`, intent ID and decisions. A submission for that instruction or a newer instruction makes that continuation unnecessary. The runner collects new events while the current wake is running.

The command is harness-neutral. It starts in the task's private state directory under `$AN_HOME/agents/TASK`, which persists across wakes so the agent can resume its session. Its stdin is one JSON object with `task`, `cursor`, the triggering `events`, `mcp_servers` (An MCP for this task, including its scoped agent credential) and `state_dir`. Its environment holds only `HOME`, `PATH`, `TMPDIR`, the names in `agent.env`, and `AN_HOME`, `AN_AGENT_TOKEN`, `AN_TASK`, `AN_BINARY`. Exit 0 means the wake was handled.

A failed process start leaves the instruction queued. The controller retries after 1 second and doubles the delay up to 60 seconds. The checkpoint survives restart. Once the command has started it may already have acted. A command failure or a lost exit status appears in Studio, and the next instruction can continue the task. A surviving harness holds a lease until exit so a restarted controller waits before launching another wake. While paused, the runner holds events until resume. Stop also ends the current wake.

`examples/agents/codex_agent.py` resumes one Codex thread per task. Its An tools cover catalog discovery, state and skill lookup, scope proposals, graph submissions and messages.

```json
{"agent": {"command": ["python3", "/path/to/An/examples/agents/codex_agent.py"], "env": ["CODEX_HOME"]}}
```

## Agent messages

The agent answers the person with `an_graph_say`: `text`, an `idempotency_key`, an optional `purpose` (`progress`, `question` or `result`) and an optional `reply_to` naming an instruction's event cursor. An records an `agent`/`message` event with the graph revision, and the timeline shows it next to the instruction. A repeated key returns the original receipt. Messages never carry decisions; ask with a surface. The size limit is `agent_message_max_bytes`.

## Instructions

The person can type to the agent. Pass `{"text": ..., "revision": N}` to `an graph --task TASK instruct` with the current `AN_TURN`, where `N` is the graph revision the person was looking at; Studio sends it from the box under the work area. An relays the text unchanged and never interprets it. The agent reads it from `an_graph_state` as an event with `actor: "user"`, `kind: "instruction"` and data `text`, `seen_revision`, `revision` and `stale`. `stale` is true when the graph moved on after the person looked. An instruction never answers a waiting surface; the agent may supersede that surface instead. Controls stay separate buttons. The size limit is `instruction_max_bytes` (see [Configuration](configuration.md)).

The proposed interaction when someone refers to a selected document block or asks how it was produced is described in [Natural interaction and the role of models (中文)](natural-interaction.zh-CN.md).

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



## Reproduce acceptance

Run `python3 tests/graph_studio_smoke.py --experiment bridge` and repeat with `--experiment cad` after building the example image. Each test creates an isolated registry. The Bridge test checks overlapping execution intervals on distinct candidate instances and verifies that the second driver generates the processing node after the human response. Use `--choice A` to cover denied handoff and retry. Both tests close the task and check the published files and authorized evidence from a fresh workspace task.

Pass `--browser` with Chrome and `playwright-core` installed to exercise the macro UI. Set `PLAYWRIGHT_CORE` to the module directory if it is outside Node's search path. Use `--directory` to retain the test registry and desktop/mobile screenshots. `cargo test` also covers a seeded indeterminate run, denied access, damaged package data, duplicate responses and expired requests.

To add a second mark to a Bridge V1 made with the revision-capable image, run
`python3 examples/graph_studio/demo.py bridge --resume V1_DELIVERY_ID` with the same registry and image.
Approve input access, click a new point on the V1 preview, and save. The new delivery contains the SVG with both marks and an OpenRaster painting with separate annotation layers. The original delivery remains available under its own ID.

## Sketch to painting

Run `python3 examples/graph_studio/demo.py sketch` (or `python3 tests/sketch_smoke.py --browser`) with the example image. The person draws on the sketch canvas with a pen; a container step turns the strokes into pressure-width SVG line art. With an image model grant, a further step renders the strokes as a control image and the host image model node produces the finished painting; see [Image model nodes](#image-model-nodes).

### Markdown review

Build the example with `container build -t an-markdown-review:headless examples/markdown_review/spool`.
Run `python3 examples/graph_studio/demo.py markdown --review-case conflict` and open the Studio URL.
The fixture supplies the original Markdown and two paragraph replacement suggestions. The local model classifies the pair. Conflicting replacements open a declared choice with both edits and reasons. The compatible fixture proceeds directly to delivery. Both paths publish the revised Markdown and a unified diff.

The central timeline shows instructions, agent messages, stages, decisions, deliveries, failures and controls in host event order. A stage folds the work nodes one submission or driver expansion added into titled steps with progress; commands, inputs and tool calls stay behind the sources view. A failure carries a reason category (`denied`, `authority`, `exit`, `output`, `budget`, `declaration`, `unknown`, `other`) that the page shows as readable text; the raw error remains in the node's sources. Select a waiting decision to answer inline. Inspect execution details through the graph and sources controls. After the task closes its published delivery remains accessible.

Run `python3 tests/markdown_review_smoke.py --browser` for the controlled backend and desktop/mobile Chrome checks. Add `--real` to use the configured local Ollama model. Both modes exercise the compatible case and both conflict answers, and apply each diff to verify the delivered bytes.

### Text workflow authorization experiments

For interactive Markdown review use `python3 examples/graph_studio/demo.py markdown --intent`. Studio shows the turn authorization card before the first protected transfer. A conflict subsequently opens the separate review decision. Build the minutes experiment's two spool images with the following commands, then open the URL printed by the demo:

```sh
container build -t an-minutes-organize:test examples/minutes/organize
container build -t an-minutes-format:test examples/minutes/format
python3 examples/minutes/demo.py --mode studio
```

Headless comparisons use `python3 tests/markdown_review_smoke.py` and the same command with `--intent`. Add `--real --browser` for local Ollama and Chrome. For minutes use `--mode baseline`, `--mode intent`, and `--mode intent --attack`. Each case uses an isolated registry. Counts below are unique individual approval requests in baseline mode versus human confirmations in intent mode (one card, zero additional approvals). Business decisions are counted separately.

| Case | Baseline approvals | Intent confirmations | Business decisions | Result |
| --- | ---: | ---: | ---: | --- |
| Compatible Markdown edits | 12 | 1 | 0 | Both edits, patch reproduces draft |
| Markdown conflict A | 15 | 1 | 1 | First edit, patch reproduces draft |
| Markdown conflict B | 15 | 1 | 1 | Second edit, patch reproduces draft |
| Minutes to announcement | 3 | 1 | 0 | Only public items reach the draft |

In the negative minutes case the graph attempts to bind the original minutes to the formatter. Its workspace permits only organizer-to-formatter handoff, and the attempted run fails before the input is mounted. The model denial case checks that no backend generation or quota reservation occurs.

### Redirect a live Markdown review

Build the CLI with `cargo build --locked` and the example with
`container build --tag an-markdown-review:replan examples/markdown_review/spool`.
Run `python3 examples/markdown_review/replan.py` with a signed-in Codex CLI and
the configured local Ollama model. Open the printed Studio URL, confirm the
initial routes, then enter a new instruction when the conflicting edits appear.
The harness reconnects to the same Codex thread and reads the instruction by
event cursor. Its graph submission can cite that cursor with
`instruction_cursor`; delivery sources retain the submission event.
The experiment uses public repository fixtures. The evidence directory printed
at startup contains the tool inventory and private session records.

Pass `--browser-test allow` or `--browser-test deny` to exercise the published
or refused delivery with headless Chrome and playwright-core. Set
`PLAYWRIGHT_CORE` if the module lives outside Node's search path.
Pause blocks new direct loads and runs, including implicit instance selection,
while allowing running work to settle. Stop also cancels active direct runs.

Controller task records contain live turn credentials in plaintext under a private same-user directory. Processes with access as that user can read them. Closing the task or ending its turn removes the active record. A credential-free marker permits access to the published task view. On macOS the runner watches registry writes and reconciles durable event cursors; idle tasks do not launch status subprocesses.
