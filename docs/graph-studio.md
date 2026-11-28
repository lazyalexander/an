# Incremental tasks in An Studio

Studio is a loopback client for an An task. It presents authorized choices, evidence and published deliveries. The Rust host records decisions and artifact versions, checks permissions, schedules spools on Apple Container and handles local model calls and exports. Python serves the task view to the browser. The controller keeps the turn credential. For the headless workflow, see the [minutes example](../examples/minutes/provenance.py).

## Run Markdown review

Start Apple Container and serve the configured local text model in Ollama. From the repository root run

```sh
cargo build --locked
container build --tag an-markdown-review:headless examples/markdown_review/spool
python3 examples/markdown_review/demo.py --intent
```

Open the printed Studio URL, decide the authorization card and choose a revision if the two suggestions conflict. Download `revised.md` and `changes.diff` from the delivery. Studio stays open until you interrupt the demo. To reopen the published files later, run

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
| `surface` | Use `spool` and `panel` to select an admitted decision. Bind the declared input names. Declare the `response` output. For a verified save, add a `document` output and `save: {"node":"save-run","port":"revised.md"}`. The designated run can consume the provisional response. To generate the save run after the user responds, set `continuation` to a declared expand node bound to this response. The saver ID is reserved for this continuation, whose proposal must include it. Other consumers wait for the captured save. |
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

Run `python3 tests/model_node_smoke.py` to check the call ledger and recovery with a counted backend. Run `python3 tests/markdown_review_smoke.py --real` to exercise the document workflow with the configured local Ollama model.

## Person controls

The person controls a task through the host, never through the agent. Pass `{"action":"pause"}`, `{"action":"resume"}` or `{"action":"stop"}` to `an graph --task TASK control` with the current `AN_TURN`; Studio shows the same actions as header buttons. Controls are not part of the agent's MCP tools.

`pause` starts no new work: queued nodes and pending permission requests wait, while running containers are still observed until they settle. The agent may keep submitting; new nodes queue. `resume` releases the pause. `stop` cancels every unsettled node, requests cancellation of running work, unloads idle drivers and leaves the task paused. After `resume`, the agent can `retry` cancelled nodes. Each control is recorded as a `user` event (`paused`, `resumed`, `stopped`), and `an_graph_state` and the host view report `control`.

## Starting from a sentence

Run Studio for a workspace instead of a task: `(cd studio && python3 -m orchestrator --workspace WORKSPACE_ID --an ../target/debug/an)`. The page opens on a text box. Typing a sentence binds a fresh session, creates the task, begins its turn and records the sentence as the task's first instruction. A detached local controller owns the turn, MCP configuration and runner. Restarting Studio reconnects to the controller for the same registry, workspace and harness. From the `studio` directory, the operator can also use `python3 -m an_host --workspace WORKSPACE_ID --an ../target/debug/an serve` to launch it headless, `start "instruction"` to create a task, `agent TASK_ID` to print its private MCP configuration, and `stop` to shut down that controller. Use the same binary and agent configuration across clients. Workspace task credentials are stored under `$AN_HOME/controllers/` and the browser receives only the task ID. Use **新任务** to return to the text box.

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

Studio opens the basic form and offers the admitted custom view through **专用面板**. For example, choose suggestion B in the Markdown conflict form or answer the same waiting request from a terminal with the active task turn:

```sh
an decision list --task "$task"
an decision answer --task "$task" --node resolve
printf '%s\n' '{"choice":"B"}' | an decision answer --task "$task" --node resolve --request "$request" --json -
```

The list includes the attempt ID, permitted fields and bound artifact versions. Pass the listed request ID when answering a previously inspected decision. Both input modes record the same viewed and responded events. Preview and handoff approvals still use the normal permission commands.

The shell creates an opaque-origin iframe, connects a private message port, then delivers the package. On `an-ready`, `an.files` contains only the named file sets, with base64 file entries. Use `an.draft`, `an.saveDraft(value)` and `an.respond(value)` for a waiting surface. Polling preserves the mounted frame. A new request ID binds a new edit session. The host validates response shape, current authority and request identity and records successful responses once.

Workspace flow rules use `["preview",origin,"studio"]` for viewing and `["publish",origin,workspace]` for delivery. Origins include image references, `response` and `task-input:<name>`. Container inputs from upstream runs require handoff permission. Code snapshots, inputs and log reads use their own host decisions. Evidence for older admissions can lack a code preview. Large image code files are identified as omissions alongside the immutable image digest.

Use `an export DELIVERY_ID DESTINATION` to export the published revision. For a closed graph task, pass `--task ACTIVE_TASK_ID` with a current turn in the same workspace. Workspace policies must permit `["export",origin,destination]` for every constituent origin. The host approval covers the complete delivery and its destination.



## Markdown review acceptance

Build the example with `container build -t an-markdown-review:headless examples/markdown_review/spool`.
Run `python3 examples/markdown_review/demo.py --review-case conflict` and open the Studio URL.
The fixture supplies the original Markdown and two paragraph replacement suggestions. The local model classifies the pair. Conflicting replacements open a declared choice with both edits and reasons. The compatible fixture proceeds directly to delivery. Both paths publish the revised Markdown and a unified diff.

The central timeline shows instructions, agent messages, stages, decisions, deliveries, failures and controls in host event order. A stage folds the work nodes one submission or driver expansion added into titled steps with progress; commands, inputs and tool calls stay behind the sources view. A failure carries a reason category (`denied`, `authority`, `exit`, `output`, `budget`, `declaration`, `unknown`, `other`) that the page shows as readable text; the raw error remains in the node's sources. Select a waiting decision to answer inline. Inspect execution details through the graph and sources controls. After the task closes its published delivery remains accessible.

Run `python3 tests/markdown_review_smoke.py --browser` for the controlled backend and desktop/mobile Chrome checks. Add `--real` to use the configured local Ollama model. Both modes exercise the compatible case and both conflict answers, and apply each diff to verify the delivered bytes.

### Text workflow authorization experiments

For interactive Markdown review use `python3 examples/markdown_review/demo.py --intent`. Studio shows the turn authorization card before the first protected transfer. A conflict subsequently opens the separate review decision. Build the minutes experiment's two spool images with the following commands, then open the URL printed by the demo:

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
