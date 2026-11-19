# Turn permissions

The harness owns the An state directory, Apple Container service and CLI. Expose only task MCP to the agent. Running arbitrary host commands under the same OS identity as the controller defeats this boundary. Keep the control interface out of the model's tool catalog.

For each user message, run `an permission begin --task TASK`. Pass the returned turn ID as AN_TURN in a new An MCP process for that task. For a bound workspace, obtain its task from an_state first. Keep this environment in the controller for decisions. Beginning another turn replaces the old one. On cancellation or completion run `an permission end --task TASK` with the original AN_TURN. Disconnect that MCP process.

A boundary action returns permission_required and request_id. The controller reads `an permission pending --task TASK`, displays the operation, scope, and details to the user and invokes `an permission decide --task TASK --request-id ID --decision once|turn|deny`. Retry the exact action with request_id (CLI --request-id, downstream MCP _an_request_id). A turn grant authorizes subsequent matching requests including later versions of the same output paths. Never automatically replay a downstream call with uncertain outcome.

Poll an_status after an_run. For awaiting_publication, call an_publish with the run ID. The controller sees the immutable manifest of names, byte lengths and SHA-256 digests. Approved runs become delivered. Output-free runs finish as completed. Private candidates can be published in a later turn. Input, log, publication, local export and downstream tool grants are separate.
