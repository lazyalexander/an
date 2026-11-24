# Architecture

An lets an agent run industrial work for one person. An is the only hub: the person, the agents and any presentation layer are its clients. The person has one input, instructions and controls sent to An, and one output, the deliveries An publishes. Everything between them is planned by agents, executed in Apple Container and recorded by An. Painting, CAD and animation are instances of this shape, not separate products.

## Final form

1. **An agent and its subagents drive the work.** The main agent runs in the harness and talks to An through task-scoped MCP. Subagents run as drivers inside containers. A subagent never holds more authority than the agent that started it: it sees only its bound inputs, its granted spools and its task namespace.
2. **Every action passes through An.** The person instructs An, and An relays the instruction to the agent. The agent acts only through An: graph submissions, transfers and model calls are An operations. Nothing addresses an agent, a container or another agent out of band.
3. **Each part of the work runs in its own container.** Temporary code, temporary artifacts and secrets stay inside the container that needs them. Every transfer between containers crosses an An permission check.
4. **No one draws a pipeline.** There is no predefined graph and no template. Agents decide the next step from the last result. A container may use a fixed internal procedure. Between containers, the graph grows as work happens: it is the execution record and the provenance, not a plan.
5. **Presentation is per pipeline.** A pipeline may use the macro UI, the CLI or the harness conversation. A headless batch pipeline needs no page. Intervention is mandatory in every pipeline; the macro UI is only one way to present it.

## How the parts fit

```text
person ── instructions / controls ──┐
                                    ▼
 presentation (macro UI | CLI | harness) ── An host (Rust: graph, versions,
                                    ▲         permissions, ledger, relay)
                                    │
          harness agent ── an_graph_submit / an_graph_state / instructions
                                    │
             ┌──────────────────────┼─────────────────────┐
          run nodes            expand nodes          surface / delivery nodes
          (spools)             (subagent drivers,    (panels for the person,
                                model calls via An)   published results)
```

- **Decisions are recorded before they run.** An agent's choice becomes a graph submission with an idempotency key and a base revision. Recovery reads the record and never asks the model again.
- **Drivers are staged, not resident.** An `expand` node runs a driver to completion in a dedicated short-lived instance and returns `subgraph.json`. A surface can name a `continuation` expand node that consumes the person's response. Waiting for the person holds no container. State a driver wants to keep is an output bound to the next driver. A resident driver may later serve as a cache, but correctness depends only on the staged record.
- **The graph is append-only.** Nodes keep immutable IDs. `retry` re-runs failed or cancelled attempts. `supersede` replaces queued work or waiting surfaces. Earlier deliveries can be resumed as a new task's `original` input.
- **Instances are reclaimed by the host.** Settled runs and drivers are unloaded by the scheduler, not by agent discretion.

See [Incremental tasks and Studio](graph-studio.md) for the submission contract and panel packages, and [Permissions](permissions.md) for turns, flows and approvals.

## Instructions and controls

An separates two channels from the person:

- **Controls** are executed by An directly and never relayed: stop or pause a task, cancel work, deny or approve a permission request. They must work even when an agent is stuck or unwilling.
- **Instructions** express intent ("use B", "rotate slower") and are relayed to the agent, which decides what to do. An relays the text unchanged. It adds a record: sender, time, turn and the graph revision the person was looking at. An agent that receives an instruction against an older revision can detect that it is stale. An never interprets or rewrites an instruction; doing so would make it a second agent.

## Agent boundary

- The harness exposes only task-scoped An MCP to the agent. A general shell under the controller's identity defeats the boundary and is not permitted.
- Container drivers have no network. A subagent that needs a model calls it through An: credentials stay on the host, and each call is budgeted and recorded per task. No model credential enters a container.

## Presentation

The macro UI, when a pipeline uses it, has a fixed shell whatever an agent generates:

- surfaces waiting for the person, with their panels;
- the current output and published deliveries;
- failures and pending permission requests;
- controls and an instruction entry.

Agents fill the rest by submitting surfaces: a panel package and a response schema. The page never exposes prompts, tool calls or the agent loop. The rules for generating panel content are still open.

A pipeline without the macro UI still needs somewhere to present surfaces and permission requests. Either it contains no surface nodes, or its harness renders them. Controls and deliveries remain available through the CLI.

## Status

| Part | State |
| --- | --- |
| Incremental graph with run, surface, delivery | Implemented |
| Staged container drivers (`expand`, `continuation`) | Implemented |
| Delivery resume as a new task | Implemented |
| Host-side instance reclamation | Implemented for settled runs and drivers |
| Macro UI shell with panels, permissions, deliveries | Implemented; no controls or instruction entry yet |
| Control channel (stop, pause, cancel) | Not started |
| Instruction relay with revision binding | Not started |
| Model calls through An for container subagents | Not started |
| Subagent authority as a strict subset of its parent | Partial: drivers see bound inputs and granted spools; budgets depend on model calls through An |
| Model-backed subagents inside drivers | Not started: example drivers are deterministic programs |
| Presentation without the macro UI (CLI / harness surfaces) | Not started |
| Native surfaces | Not started (see below) |
| Turn-level policy to avoid approval fatigue | Not started |
| Reclamation of unreferenced intermediate artifacts | Not started |

## Native surfaces

Pressure painting cannot run in a web panel. Measured on the reference machine: synthetic input reaches the canvas with zero pressure, and macOS delivers the Sidecar Pencil to ordinary applications as a plain mouse. A driver-level injector (Codonic Pad) delivers genuine tablet events to Krita. Pinned versions are in [studio/VERSIONS.md](../studio/VERSIONS.md); the probe evidence is under `studio/spikes/`.

A native surface will therefore dispatch a document to an adapter inside the native application and wait for a save receipt as its response. The legacy instance-mode Studio (`python3 -m orchestrator` without `--task`) already implements this receipt: adopt, re-render and unlock are queued, the Krita adapter saves the live document over its archive, and only its acknowledgement advances state. That mode is frozen and kept only as the reference for the native surface.
