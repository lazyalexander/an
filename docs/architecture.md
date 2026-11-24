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

See [Incremental tasks and Studio](graph-studio.md) for the submission contract and panel packages, [Permissions](permissions.md) for turns, flows and approvals, and [Configuration](configuration.md) for settings and text.

## Instructions and controls

An separates two channels from the person:

- **Controls** are executed by An directly and never relayed: stop or pause a task, cancel work, deny or approve a permission request. They must work even when an agent is stuck or unwilling.
- **Instructions** express intent ("use B", "rotate slower") and are relayed to the agent, which decides what to do. An relays the text unchanged. It adds a record: sender, time, turn and the graph revision the person was looking at. An agent that receives an instruction against an older revision can detect that it is stale. An never interprets or rewrites an instruction; doing so would make it a second agent.

## Agent boundary

- The harness exposes only task-scoped An MCP to the agent. A general shell under the controller's identity defeats the boundary and is not permitted.
- Container drivers have no network. A subagent that needs a model calls it through An: credentials stay on the host, and each call is budgeted and recorded per task. No model credential enters a container.

## Presentation

### Spools declare their UI

A spool declares the interaction it needs in its manifest, next to its inputs and outputs. The declaration is admitted, digest-pinned and versioned with the image, so every panel a person sees was reviewed at admission. An agent chooses a spool and fills its declared surface with parameters: which artifacts to show, defaults, and a one-line recommendation. An agent never invents a panel.

A declaration uses a small set of decision types: choose among artifacts, locate a point or region on an artifact, adjust a value or enumeration, confirm or reject, and compare versions. The response schema follows from the declaration. A spool may also ship a custom panel package for the macro UI, but it must declare one of these types as a fallback so the CLI and the harness can present the same decision.

Ask the person only when the agent should not decide alone: an irreversible step, a matter of taste, ambiguous intent or a costly branch. One surface asks one decision and always offers "let the agent decide". Several waiting surfaces are queued by priority, not shown at once. Responses are recorded so later runs can ask less.

This needs a manifest schema after `an.spool/v1`, whose reader rejects unknown fields.

### Macro UI layout

The macro UI follows a harness layout:

- **Timeline, center.** Only An-level records: the person's instructions, the agent's stage summaries, decisions, failures and deliveries. No prompts, tool calls, agent loop or scrolling logs.
- **Side panel.** The surfaces waiting for the person and the outputs to check. It widens or switches to a focus view when an image or model needs room. Pressure painting stays in the native application.
- **Instruction box, bottom.** Free text is relayed unchanged to the agent with the current graph revision. Text never answers a waiting surface; the agent may supersede that surface in response.
- **Controls.** Stop, pause, approve and deny are separate buttons executed by An. They are never parsed from text.

A pipeline without the macro UI still needs somewhere to present surfaces and permission requests. Either it contains no surface nodes, or its harness renders the declared decision types. Controls and deliveries remain available through the CLI.

## Status

| Part | State |
| --- | --- |
| Incremental graph with run, surface, delivery | Implemented |
| Staged container drivers (`expand`, `continuation`) | Implemented |
| Delivery resume as a new task | Implemented |
| Host-side instance reclamation | Implemented for settled runs and drivers |
| Macro UI with panels, permissions, deliveries | Implemented, with pause/resume/stop buttons and an instruction box; harness layout not started |
| Spool-declared UI and decision types (manifest after v1) | Not started; example panels are HTML packages generated by driver code |
| Control channel (pause, resume, stop) | Implemented: host-executed, turn-gated, recorded as user events; not exposed to agents |
| Instruction relay with revision binding | Implemented: unchanged text, sender turn, seen and current revision, stale flag |
| Model calls through An for container subagents | Implemented for host-local Ollama via model nodes |
| Subagent authority as a strict subset of its parent | Partial: drivers see bound inputs and granted spools; model aliases and call quotas are task-scoped |
| Model-backed subagents inside drivers | Bridge model mode uses a host-mediated inference result to choose the next subgraph |
| Presentation without the macro UI (CLI / harness rendering of decision types) | Not started |
| Native surfaces | Not started (see below) |
| Turn-level policy to avoid approval fatigue | Not started |
| Reclamation of unreferenced intermediate artifacts | Not started |

## Native surfaces

Pressure painting cannot run in a web panel. Measured on the reference machine: synthetic input reaches the canvas with zero pressure, and macOS delivers the Sidecar Pencil to ordinary applications as a plain mouse. A driver-level injector (Codonic Pad) delivers genuine tablet events to Krita. Pinned versions are in [studio/VERSIONS.md](../studio/VERSIONS.md); the probe evidence is under `studio/spikes/`.

A native surface will therefore dispatch a document to an adapter inside the native application and wait for a save receipt as its response. The legacy instance-mode Studio (`python3 -m orchestrator` without `--task`) already implements this receipt: adopt, re-render and unlock are queued, the Krita adapter saves the live document over its archive, and only its acknowledgement advances state. That mode is frozen and kept only as the reference for the native surface.
