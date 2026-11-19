# Private bridge artifacts and named spool inputs (dev)

A host can register an MCP tool that imports a file, consumes a candidate run, or performs both operations. The agent chooses the transfer and receives metadata. The host controls registration, workspace flow policy and each turn. The spool manifest supplies the input port names.

```text
local bridge → An artifact ID ──────┐
                                    ├→ named inputs → spool → private candidate
other spool → An handoff ticket ────┘                              │
                                              An approval → local bridge
```

## Bridge contract

Register the trusted local process with `an server register io --artifact-tool receive --artifact-tool send -- /path/to/server`. Task or workspace grants still name individual tools, such as `io/receive`. Re-registering a changed command or artifact tool set invalidates the old server grant.

Each opted-in downstream tool declares an object argument named `an_transfer`. In the agent-facing schema An substitutes an optional string `an_source`. Supply the ID of a private candidate run to send its frozen files. An authorizes the exact MCP arguments and candidate revision, creates an empty or populated input tree, and supplies the downstream tool with two absolute paths.

| Bridge argument | Contract |
| --- | --- |
| `an_transfer.input` | Per-call directory containing the candidate's relative files. Empty for a call without a source. |
| `an_transfer.output` | Existing regular file. Write up to 32 MiB to this file in place to return bytes. Leave it empty for a receipt-only call. |

On success An imports the original file descriptor, rechecks the task turn and bridge grant, and returns `artifact`, `revision`, `file` (currently `data`), `bytes` and `sha256`. A receipt-only call returns `{"status":"completed"}`. An ignores downstream result content for these tools. The transfer area lasts one call. A bridge executes as a trusted host program with the privileges of its OS account.

Private artifacts retain their task, source call and server revision across turns. Each subsequent spool input requires current authorization. A task-private candidate may be sent directly to a bridge. Configure the two directions explicitly in workspace flow policy, for example:

```json
[
  ["bridge_input", "io/receive", "recipient@sha256:…"],
  ["bridge_send", "sender@sha256:…", "io/send"]
]
```

For an outbound pending request, the trusted host can run `an permission preview --task TASK --request-id ID` to read the frozen revision and its file map for a confirmation UI. Values are base64 file entries in An revision format. The CLI checks the current turn, policy, grant and candidate. Keep the command output private to the controller.

Per-call records in `an_state.calls` identify the tool, candidate source and attempted revision. A completed transfer retains its result metadata so a reconnecting client can recover the artifact reference without repeating the call. The gateway reclaims abandoned transfer directories on startup and preserves active calls. A timeout records an unknown outcome. An cannot undo an external effect after dispatch. A bridge with external side effects should track a stable operation identifier and handle duplicate attempts.

## Running with more than one input

Pass a JSON object to `an_run.inputs` or `an run --inputs`. Keys must match the admitted manifest, including every required port for a named invocation. Values are immutable bridge artifact IDs, handoff tickets, task input names or published delivery IDs. Each revision retains its relative files under `/input/<port>/`. For example:

```json
{
  "spool": "recipient@sha256:…",
  "inputs": {"data": "artifact-…", "instructions": "handoff-…"},
  "argv": ["/spool/code/process"],
  "outputs": ["result"]
}
```

A pending input returns its `port` and `request_id`. The trusted harness decides the request and retries the same invocation with `input_requests` mapping that port to its ID. Retain previously approved IDs while resolving subsequent ports. All input decisions and handoffs commit together with the instance reservation. If launching the worker fails, the reservation, tickets and submitted once-only decisions are released. The worker rechecks input authority before staging files.

Single-source callers may continue using `input` with the existing flat layout and `request_id`. Choose one input form for each invocation.
