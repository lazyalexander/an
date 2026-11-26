# Turn permissions

The harness owns the An state directory, Apple Container service and CLI. Expose only task MCP to the agent. Running arbitrary host commands under the same OS identity as the controller defeats this boundary. Keep the control interface out of the model's tool catalog.

For each user message, run `an permission begin --task TASK`. Pass the returned turn ID as AN_TURN in a new An MCP process for that task. For a bound workspace, obtain its task from an_state first. Keep this environment in the controller for decisions. Beginning another turn replaces the old one. On cancellation or completion run `an permission end --task TASK` with the original AN_TURN. Disconnect that MCP process.

A boundary action returns permission_required and request_id. The controller reads `an permission pending --task TASK`, displays the operation, scope, and details to the user and invokes `an permission decide --task TASK --request-id ID --decision once|turn|deny`. Retry the exact action with request_id (CLI --request-id, downstream MCP _an_request_id). A turn grant authorizes subsequent matching requests including later versions of the same output paths. Never automatically replay a downstream call with uncertain outcome.

Poll an_status after an_run. For awaiting_publication, call an_publish with the run ID. The controller sees the immutable manifest of names, byte lengths and SHA-256 digests. Approved runs become delivered. Output-free runs finish as completed. Private candidates can be published in a later turn. Input, log, publication, local export and downstream tool grants are separate.

## Coordinating host policy and spool flows

Install a JSON array of permitted directional flows with `an workspace policy set WORKSPACE --file flows.json`. Read it with `an workspace policy show WORKSPACE`. An empty array closes all four data routes. Rows are ["input", inputName, recipientImage], ["handoff", senderImage, recipientImage], ["publish", senderImage, workspaceID], or ["export", senderImage, absoluteDestination]. Images use their registered digest-pinned references. Existing public deliveries also require a permitted sender-to-recipient flow to enter another spool.

The trusted harness can atomically begin a turn with `an permission begin --task TASK --file host.json`. The JSON object contains a default (ask, deny or unavailable) and an effects array. Each effect matches the exact operation, scope and details from the controller's request, with decision allow, deny, ask or unavailable. Use unavailable when the harness cannot evaluate the effect. Without a host policy An uses interactive approval. A generic shell or MCP permission does not match an artifact transfer.

Replace the entire turn policy with `an permission policy --task TASK --file host.json`. Every replacement invalidates previous requests, grants and unconsumed handoffs. Retry with fresh arguments and no request ID after a policy change. examples/harness_policy.py demonstrates a trusted adapter installing a single exact effect. For a user approval of a pending request, use the existing decide command instead.

Use an_handoff(run, recipient_spool) for a private candidate. Pass the returned handoff ID as an_run input. It belongs to one task, turn, recipient and frozen revision. The public delivery list and host export accept only published deliveries.

The dev artifact bridge adds scoped bridge-to-spool and spool-to-bridge flows, and named input approvals. See [private bridge artifacts](bridge-artifacts.md).

### Local model decisions

Only the trusted host can create a task with `--model ALIAS` and a call quota. A workspace flow for inference has the form `["model", inputOrigin, "model:ALIAS"]`. The turn decision identifies the model, input revision and node attempt. A denied request makes no inference call and consumes no slot. Result handoff to a spool requires a second boundary decision. Model input and output evidence reads require preview authorization for their respective origins. See [Host model calls](graph-studio.md#host-model-calls) for the recovery contract.

## Turn intent for task graphs

The trusted controller can declare a set of directional routes before submitting a graph. Start a turn, then use the following commands with that turn's controller environment:

```sh
an permission intent-propose --task TASK --file routes.json
an permission intent-show --task TASK
an permission intent-decide --task TASK --request-id CARD_ID --decision allow
```

The decision can also be `deny` or an ordered JSON array such as `["allow","ask","deny"]`. Each entry corresponds to one displayed route. Studio presents the same choices in a single form. The proposal and decision commands are available only to the controller. A decision cannot be changed or applied to another card ID.

Each route requires four fields: `operation`, `source`, `origin`, and `destination`. Use a graph input reference (`{"input":"minutes.json"}`) or a producer output (`{"node":"organize","port":"summary.json"}`). Task inputs have origin `task-input:<name>`; An fills in their immutable version. For a future output supply the producer origin (registered image digest, model alias origin, or `response`). To preapprove a read-only spool output, also declare `producer_argv` with the complete command and arguments. An compares it with the recorded invocation that produced the bound file. A missing or different command, an editable producer, or a change from the first authorized revision requires an individual decision for the actual revision. Destinations are task spool references, `model:<alias>`, `studio`, the task's delivery destination or `graph-metadata`. Proposal publication is a distinct route from publishing a delivery. See the trusted declarations in the Markdown and minutes examples.

The existing workspace and host policy checks run first. A matching intent approval can satisfy an interactive check inside the graph scheduler, including a dynamically proposed child. Unlisted flows follow normal permission handling. An explicit `ask` requires individual confirmation even when a host allowance or turn grant exists. A matching denial fails the flow. Studio keeps unrelated requests, including evidence reads, available while the card is pending. Confirming the card settles covered approvals and denials even when a multi-input node still needs another decision. Model quota checks remain in effect. Replacing a turn or either policy revision invalidates the card. Intent matches record the card ID, route, actual revision and operation details in the effect ledger. The declared output origin identifies the trusted producer; permission matching does not inspect document semantics.

### Turn scopes for changing graphs

The controller may pass an object to `intent-propose` in place of the exact-route array. The operator reviews the rules together in Studio. Replace spool names with the task's admitted spool identifiers.

```json
{"turn_scope":[
  {"operation":"input","origin":"task-input:original","source":{"input":"original"},"destination":"converter"},
  {"operation":"handoff","origin":"converter","producer_argv":["convert"],"source":{"producer_port":"output"},"destination":"viewer"}
]}
```

The first rule pins the named input revision. The second covers every version emitted on that port by that immutable spool command during the turn, regardless of graph node ID. It grants the output itself, including results produced after a separately authorized input. This applies to text, images, scene files and UI preview artifacts alike. A preview uses destination `studio`; a publication names the workspace. The agent may propose these routes with `an_scope_propose`. A model route names the task’s granted alias; the card displays the resolved backend and task-wide call budget. A backend change needs separate approval. A driver may publish its subgraph.json port to graph-metadata; each resulting node is independently checked. Downstream MCP calls still require individual authorization under a closed scope. Ordinary evidence, logs and export use existing host policy. Workspace and host denials still apply. The scope expires when its turn or policy stamp changes.
