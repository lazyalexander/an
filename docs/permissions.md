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
