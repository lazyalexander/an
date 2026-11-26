# Architecture

An lets an agent run industrial work for one person. An is the only hub: the person, the agents and any presentation layer are its clients. It is the layer between the person and the agent: it curates what passes in each direction, and it chains isolated container programs and agents into a recorded pipeline. The person has one input, instructions and controls sent to An, and one output, the deliveries An publishes. Everything between them is planned by agents, executed in Apple Container and recorded by An. Painting, CAD and animation are instances of this shape, not separate products.

## Final form

1. **An agent and its subagents drive the work.** The main agent runs in the harness and talks to An through task-scoped MCP. Subagents run as drivers inside containers. A subagent never holds more authority than the agent that started it: it sees only its bound inputs, its granted spools and its task namespace.
2. **Every action passes through An.** The person instructs An, and An relays the instruction to the agent. The agent acts only through An: graph submissions, transfers and model calls are An operations. Nothing addresses an agent, a container or another agent out of band.
3. **Each part of the work runs in its own container.** Temporary code, temporary artifacts and secrets stay inside the container that needs them. Every transfer between containers crosses an An permission check.
4. **No one draws a pipeline.** There is no predefined graph and no template. Agents decide the next step from the last result. A container may use a fixed internal procedure. Between containers, the graph grows as work happens: it is the execution record and the provenance, not a plan.
5. **A text box whenever a person is involved.** The macro UI resembles a mainstream agent harness: the person starts and steers work by typing, and the agent answers in the same timeline. Work that needs more than text, such as pressure painting or image and model inspection, is added as presentation plugins. A headless batch pipeline may use only the CLI. Intervention is mandatory in every pipeline.

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

## Curation between the person and the agent

An curates both directions, without becoming a second agent:

- **The original is always kept.** Curation adds a layer on top of the original text and never replaces it. Both are recorded, and the agent can read both.
- **Person to agent:** An attaches context, not new intent: the graph revision the person was looking at, the selected node or artifact and the turn. It does not paraphrase what the person meant.
- **Agent to person:** An turns agent output into timeline entries: messages, stage summaries, decisions and deliveries. Prompts, tool calls and the agent loop stay hidden. This direction matters most, because it is what makes the macro UI readable.
- **Curation by a model is a graph step.** If a model rewrites or summarises, it runs as a recorded node with its own inputs and outputs, so every change can be traced. Nothing on the relay path is rewritten silently.

## Starting work and agent replies

- **Start from a sentence.** The person opens the macro UI for a workspace and types what they want. An creates a task, begins a turn and records the sentence as the task's first instruction. The agent attached to that task picks it up.
- **The agent talks through An.** The agent posts messages to the person with `an_graph_say`: progress, a question, or a result. Each message is recorded with the graph revision it refers to and, when it answers an instruction, that instruction's cursor. Messages appear in the timeline; they never carry decisions, which stay typed surfaces.
- **The agent runner belongs to the product.** Whatever wakes the agent when a task starts, an instruction arrives or a surface is answered must be a harness-neutral part of An, not an example script.

## Presentation plugins

A plugin presents a kind of surface that a panel package cannot: pressure painting in a native application, image or 3D rendering, or other specialised viewers. The Krita native surface is the first plugin: the graph dispatches a document, and the plugin returns a save receipt. Further plugins follow the same contract: An dispatches a frozen input, the plugin presents it outside the agent's reach, and its result enters the graph as a validated response.

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
- **Instruction box, bottom.** Always present when a person is involved. Without a task it starts one. In a task, free text is relayed with its original kept and its context attached. Text never answers a waiting surface; the agent may supersede that surface in response.
- **Agent messages** appear in the timeline next to the instructions they answer.
- **Controls.** Stop, pause, approve and deny are separate buttons executed by An. They are never parsed from text.

A pipeline without the macro UI still needs somewhere to present surfaces and permission requests. Either it contains no surface nodes, or its harness renders the declared decision types. Controls and deliveries remain available through the CLI.

## Framework and experiment boundaries

The An host owns tasks, grants, graph transitions, immutable versions and receipt validation. Task Studio starts with `cd studio && python3 -m orchestrator --task TASK`. Its entry point imports only the graph server and configuration. A task's admitted panel declarations supply the decisions shown in the timeline.

Experiment packages under `examples/` own prompts, document transformations, application plugins, demo entry points and fixtures. Markdown review and graph Studio spools run in containers. The earlier Bridge/CAD instance UI, Blender pipeline and pressure probes live under `examples/legacy_studio/`. Start that experiment from its own directory with `python3 -m orchestrator --instance bridge-studio --fake`. Its run directory resolves to the existing local Studio archives.

The native channel asks an installed adapter for its supported extensions, snapshot filename, published filename and byte validator. An authorizes and binds each receipt to the task and attempt before publishing it. The contract lives in `src/adapters.rs`; the Krita archive checks live in `examples/krita-format`. The default binary includes this trusted adapter for existing native graphs. Build the host without experimental adapters using `cargo build --no-default-features`. Manifests requesting an unavailable adapter fail admission. Adding an adapter requires explicit registration in the binary.

## Status

| Part | State |
| --- | --- |
| Incremental graph with run, surface, delivery | Implemented |
| Staged container drivers (`expand`, `continuation`) | Implemented |
| Delivery resume as a new task | Implemented |
| Host-side instance reclamation | Implemented for settled runs and drivers |
| Macro UI with panels, permissions, deliveries | Implemented with a task timeline, inline decisions, deliveries and task controls |
| Spool-declared UI and decision types (manifest after v1) | Implemented in `an.spool/v2`, with pinned presentations and host-validated responses |
| Control channel (pause, resume, stop) | Implemented: host-executed, turn-gated, recorded as user events; not exposed to agents |
| Instruction relay with revision binding | Implemented: unchanged text, sender turn, seen and current revision, stale flag |
| Model calls through An for container subagents | Implemented for host-local Ollama via model nodes |
| Subagent authority as a strict subset of its parent | Partial: drivers see bound inputs and granted spools; model aliases and call quotas are task-scoped |
| Model-backed subagents inside drivers | Bridge model mode uses a host-mediated inference result to choose the next subgraph |
| Presentation without the macro UI (CLI / harness rendering of decision types) | Interactive and JSON CLI implemented; dedicated harness rendering remains open |
| Native surfaces | Krita native node, saved KRA receipts and Bridge native mode implemented; reference-machine Pencil acceptance gates instance-mode retirement |
| Turn-level policy to avoid approval fatigue | Implemented for declared graph routes. Markdown review and minutes-to-announcement exercise the shared CLI and Studio authorization card. |
| Reclamation of unreferenced intermediate artifacts | Not started |
| Agent messages to the person (`an_graph_say`) | Implemented: idempotent, revision- and instruction-linked, shown in the timeline |
| Starting a task from a sentence in the macro UI | Implemented: workspace Studio creates the task and the runner wakes the agent |
| Harness-neutral agent runner | Implemented in the Studio controller: wakes on instructions, answers and resume; at most once; pause and stop respected; Codex adapter |
| Curation layer (original kept, context attached, outbound summaries) | Partial: instructions carry revision and turn; no outbound summaries |
| Presentation plugin contract beyond Krita | Not started |
| Remote model providers with host-held credentials | Not started: host-local Ollama only |

## Native surfaces

Pressure painting cannot run in a web panel. Measured on the reference machine: synthetic input reaches the canvas with zero pressure, and macOS delivers the Sidecar Pencil to ordinary applications as a plain mouse. A driver-level injector (Codonic Pad) delivers genuine tablet events to Krita. Pinned versions are in [examples/legacy_studio/VERSIONS.md](../examples/legacy_studio/VERSIONS.md); the probe evidence is under `examples/legacy_studio/spikes/`.

The Bridge native graph sends one versioned layered document to the local Krita adapter. Ordinary saves update the private working copy. **完成并交付** saves the bound canvas, verifies it is clean and submits a separate KRA snapshot. The host checks the attempt, authority, content digest and archive before committing a document output. See [running and verifying the native graph](graph-studio.md#krita-native-painting).

The legacy instance-mode Studio remains available until the new graph passes its physical Pencil and exported-layer gate on the reference machine.
