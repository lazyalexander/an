# Architecture

An manages the flow of artifacts through a workspace. The harness agent plans work and reviews its results. An schedules isolated spools on Apple Container, checks each protected transfer, and records the versions consumed and produced. A person can inspect the resulting delivery, make a decision in Studio or the CLI, and resume from the published version in a later task.

## Artifact flow designs (中文)

1. [Decision provenance and downstream effects](artifact-flow.zh-CN.md)
2. [Artifact versions and cross-document block references](artifact-revisions.zh-CN.md)
3. [Human, agent and An responsibilities](decision-authority.zh-CN.md)
4. [Workspace, session, task and turn](workspace-lifecycle.zh-CN.md)
5. [Spool, instance, container and run](spool-execution.zh-CN.md)
6. [Identity, authorization and delegation](material-permissions.zh-CN.md)
7. [How agents access An](agent-access.zh-CN.md)
8. [Natural interaction and the role of models](natural-interaction.zh-CN.md)

The [container engine design (中文)](container-engine.zh-CN.md) describes the execution boundary, recovery rules and headless acceptance checks.

## Final form

1. **One main agent is responsible for a workspace at a time.** It can advance several tasks through task-scoped MCP. The harness manages sessions and subagents. An mediates the materials handed to each task and returned to the main agent. Container drivers receive the inputs and spools granted for their work.
2. **An mediates graph changes and artifact transfers.** The person sends instructions through An. The main agent submits work through the task gateway, including model calls and requests to downstream tools. The harness coordinates its agents' private work. An records who received protected material, exercised delegated authority or handed a result to a recipient.
3. **Each part of the work runs in its own container.** Temporary code, temporary artifacts and secrets stay inside the container that needs them. Every transfer between containers crosses an An permission check.
4. **No one draws a pipeline.** There is no predefined graph and no template. Agents decide the next step from the last result. A container may use a fixed internal procedure. Between containers, the graph grows as work happens: it is the execution record and the provenance, not a plan.
5. **A text box whenever a person is involved.** The macro UI resembles a mainstream agent harness: the person starts and steers work by typing, and the agent answers in the same timeline. Work that needs more than text, such as pressure painting or image and model inspection, can be explored in presentation plugins. A headless batch pipeline may use only the CLI. The person sets the goal and can delegate decisions within a recorded scope.

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
          (spools)             (staged drivers,      (panels for the person,
                                model calls via An)   published results)
```

- **Decisions are recorded before they run.** Graph submissions record an agent's workflow choices with an idempotency key and a base revision. The proposed business decision record also identifies the decision-maker, grant and evidence. Recovery reuses the recorded choice and reconciles its outstanding effects.
- **Drivers are staged, not resident.** An `expand` node runs a driver to completion in a dedicated short-lived instance and returns `subgraph.json`. A surface can name a `continuation` expand node that consumes the person's response. Waiting for the person holds no container. State a driver wants to keep is an output bound to the next driver. A resident driver may later serve as a cache, but correctness depends only on the staged record.
- **The graph is append-only.** Nodes keep immutable IDs. `retry` re-runs failed or cancelled attempts. `supersede` replaces queued work or waiting surfaces. Earlier deliveries can be resumed as a new task's `original` input.
- **Instances are reclaimed by the host.** Settled runs and drivers are unloaded by the scheduler, not by agent discretion.

See [Incremental tasks and Studio](graph-studio.md) for the submission contract and panel packages, [Permissions](permissions.md) for turns, flows and approvals, and [Configuration](configuration.md) for settings and text.

## Instructions and controls

An separates two channels from the person:

- **Controls** are executed by An directly and never relayed: stop or pause a task, cancel work, deny or approve a permission request. They must work even when an agent is stuck or unwilling.
- **Instructions** express intent ("use B", "rotate slower") and are relayed to the agent, which decides what to do. An relays the text unchanged. It adds a record: sender, time, turn and the graph revision the person was looking at. An agent that receives an instruction against an older revision can detect that it is stale. An never interprets or rewrites an instruction; doing so would make it a second agent.

## Curation between the person and the agent

The main agent interprets free text. An presents recorded facts and executes the person's controls even while the agent is offline. The workspace requires no resident LLM in An. [Natural interaction and the role of models (中文)](natural-interaction.zh-CN.md)

- **What the person said and saw.** An preserves the original instruction and its turn. The proposed context records the viewed graph revision and event cursor, the selected artifact version and block, and any explicitly selected recipient. The agent receives the context it is authorized to use.
- **What happened.** The timeline shows instructions, agent messages, stage progress, decisions, deliveries, failures and controls. The proposed artifact view traces the selected block to its sources and the graph at the time. Opening captured model inputs and execution logs requires evidence access.
- **What it means.** The person can ask the main agent to explain the evidence and save a versioned explanation with its author and citations. If the task uses a model node to summarize or rewrite material, An records the model call and its input and output versions.

## What sets An apart from a chat harness

Two things a mainstream harness does not do carry most of An's weight:

- **Heavy translation.** One sentence from the person becomes inputs for several spools, and their outputs become something the person can read. Every hop can lose meaning, so each translation is a recorded graph step that keeps its original, and a wrong hop can be traced.
- **The picture is context.** A stroke on the sketch canvas or a drag on a panel is information the agent should have, not only typed text. Presentation plugins report *observations*: a versioned snapshot of what the person sees and how it changed since the previous one. An observation enters the graph like any artifact, under the same permissions, and can wake the agent. Not every change should: plugins report settled changes, not every stroke.

## Vocabulary

- **Spool** stays the name of a packaged, admitted capability. It is already the manifest, admission and CLI term, and the image fits: each spool holds one kind of skill.
- **Graph** is the term in APIs and documentation for what agents assemble at run time, because the work branches, merges and grows while it runs; "pipeline" suggests a fixed line. Interfaces for people call it **流程** (flow) and do not show the word graph.

## Scope and realism

Markdown review exercises container admission, authorized handoffs and publication end to end. The planner and the presentation layer still face the following constraints.

1. **Choosing and composing spools.** With a dozen well-described spools a capable model composes them; with hundreds it needs retrieval and evaluation, not a longer prompt. Small local models plan poorly.
2. **Format compatibility** between spools is the most common failure point and needs converter spools and strict media types.
3. **Translation costs** a model call, latency and a failure point per hop, so hops stay few and inspectable.
4. **Visual context is expensive**, and deciding which changes matter is itself a problem.
5. **Output quality is bounded by the spools**, not by orchestration.

So An targets bounded domains first: tens of curated spools, a person at the decisions that matter, tolerance for tens of seconds per step. Painting assistance, CAD drawings and document work fit. "Any sentence into any pipeline" is not a near-term goal; one domain is made reliable before the spool catalog grows.

## Starting work and agent replies

- **Start from a sentence.** The person opens the macro UI for a workspace and types what they want. An creates a task, begins a turn and records the sentence as the task's first instruction. The agent attached to that task picks it up.
- **The agent talks through An.** The agent posts messages to the person with `an_graph_say`: progress, a question, or a result. Each message is recorded with the graph revision it refers to and, when it answers an instruction, that instruction's cursor. Messages appear in the timeline. A typed surface records a person's response. The proposed delegation contract records a decision taken by an authorized agent.
- **The controller wakes the configured harness.** An passes task events such as a new instruction or an answered surface to the harness through the agent runner. The harness chooses which agent or thread to start or resume.

## Presentation plugins

Spools can ship a custom presentation for a declared decision. An supplies the frozen inputs and validates the response against the same schema used by the CLI. Specialized drawing and viewing interactions are developed on the visual experiment branch.

## Agent boundary

- The main agent operates An through task-scoped MCP. A harness that offers shell tools must isolate the host registry, control socket and controller credentials from those tools. The proposed review interface gives a subagent frozen files for ordinary review and scoped access for independent protected actions. [How agents access An (中文)](agent-access.zh-CN.md)
- Container drivers have no network. A subagent that needs a model calls it through An: credentials stay on the host, and each call is budgeted and recorded per task. No model credential enters a container.

## Presentation

### Spools declare their UI

A spool declares the interaction it needs in its manifest, next to its inputs and outputs. The declaration is admitted, digest-pinned and versioned with the image, so every panel a person sees was reviewed at admission. An agent chooses a spool and fills its declared surface with parameters: which artifacts to show, defaults, and a one-line recommendation. An agent never invents a panel.

A declaration uses a small set of decision types: choose among artifacts, locate a point or region on an artifact, adjust a value or enumeration, confirm or reject, and compare versions. The response schema follows from the declaration. A spool may also ship a custom panel package for the macro UI, but it must declare one of these types as a fallback so the CLI and the harness can present the same decision.

Ask the person when their judgment or authority is needed, for example on a matter of taste, ambiguous intent, a costly branch or an irreversible action. A surface names the choice and can offer "let the agent decide" when the person has authority to delegate it. A combined confirmation lists each covered action and An records each separately. Waiting surfaces are queued by priority. Responses are recorded so later runs can ask less.

This needs a manifest schema after `an.spool/v1`, whose reader rejects unknown fields.

### Macro UI layout

The macro UI follows a harness layout:

- **Timeline, center.** Only An-level records: the person's instructions, the agent's stage summaries, decisions, failures and deliveries. No prompts, tool calls, agent loop or scrolling logs.
- **Side panel.** The surfaces waiting for the person and the outputs to check. It widens or switches to a focus view when an image or model needs room.
- **Instruction box, bottom.** Always present when a person is involved. Without a task it starts one. In a task, free text is relayed with its original kept and its context attached. Text never answers a waiting surface; the agent may supersede that surface in response.
- **Agent messages** appear in the timeline next to the instructions they answer.
- **Controls.** Stop, pause, approve and deny are separate buttons executed by An. They are never parsed from text.

A pipeline without the macro UI still needs somewhere to present surfaces and permission requests. Either it contains no surface nodes, or its harness renders the declared decision types. Controls and deliveries remain available through the CLI.

## Framework and experiment boundaries

The An host owns tasks, grants, graph transitions, immutable versions and receipt validation. Task Studio starts with `cd studio && python3 -m orchestrator --task TASK`. Its entry point imports only the graph server and configuration. A task's admitted panel declarations supply the decisions shown in the timeline.

Markdown review and minutes own their prompts, transformations, spools and fixtures under `examples/`. The Codex adapter under `examples/agents/` connects a harness to the task MCP interface. Studio and the scheduler can run independently of all three examples.

The [visual experiment branch](https://github.com/lazyalexander/an/tree/experiment/visual-studio) preserves the Bridge and CAD workflows, canvas, image services, composition benchmarks and their tests. The common boundary is a frozen artifact revision and its published bindings. Opening, exporting or resuming from a published visual delivery reads those records directly. Finish pending visual work with the experiment build. Use a separate registry for new experiments. A capability belongs on `dev` when a spool-independent contract can express it and a headless test can verify its behavior. A domain workflow stays with its fixtures and acceptance tests.

## Status

| Part | State |
| --- | --- |
| Incremental graph with run, surface, delivery | Implemented |
| Staged container drivers (`expand`, `continuation`) | Implemented |
| Delivery resume as a new task | Implemented |
| Main agent workspace across sessions | Designed. Explicit handoffs create durable review items. Reviewers can receive read-only evidence snapshots scoped to a graph revision and event cursor. |
| Follow-up work across agents and fresh instances | Designed. A follow-up links its original delegation, report version and new run evidence. |
| Host-side instance reclamation | Implemented for settled runs and drivers |
| Macro UI with panels, permissions, deliveries | Implemented with a task timeline, inline decisions, deliveries and task controls |
| Spool-declared UI and decision types (manifest after v1) | Implemented in `an.spool/v2`, with pinned presentations and host-validated responses |
| Delegated business decisions and versioned review conclusions | Designed. The decision record links the authorized decision-maker, grant, review evidence and subsequent effects. |
| Control channel (pause, resume, stop) | Implemented: host-executed, turn-gated, recorded as user events; not exposed to agents |
| Instruction relay with revision binding | Implemented: unchanged text, sender turn, seen and current revision, stale flag |
| Model calls through An for container subagents | Implemented for host-local Ollama via model nodes |
| Scoped authority for a subagent | Agents have task- and turn-bound credentials. Drivers receive bound inputs and granted spools. Per-assignment delegation, access to selected blocks and returning reports to named recipients are proposed in [item 6](material-permissions.zh-CN.md). |
| Model-backed subagents inside drivers | The Markdown review driver reads a host-mediated inference result to select automatic revision or a human decision |
| Presentation without the macro UI (CLI / harness rendering of decision types) | Interactive and JSON CLI implemented; dedicated harness rendering remains open |
| Turn-level policy to avoid approval fatigue | Implemented for declared graph routes. Markdown review and minutes-to-announcement exercise the shared CLI and Studio authorization card. |
| Reclamation of unreferenced intermediate artifacts | Implemented for unreferenced file revisions after task settlement. The workspace design adds retention for handoffs to the main agent and frozen review snapshots. |
| Agent messages to the person (`an_graph_say`) | Implemented: idempotent, revision- and instruction-linked, shown in the timeline |
| Starting a task from a sentence in the macro UI | Implemented: workspace Studio creates the task and the runner wakes the agent |
| Harness-neutral agent runner | Implemented in the Studio controller: wakes on instructions, answers and resume; at most once; pause and stop respected; Codex adapter |
| Versioned selections and explanations | Instructions already preserve the original text and graph revision. Binding a selected block and viewed cursor, opening its historical graph and saving a cited explanation are proposed in [item 8](natural-interaction.zh-CN.md). |
| Outbound curation: stage summaries and readable failure reasons | Implemented without a model: agent and driver submissions fold into titled stages with progress; failures carry a reason category shown as locale text |
| Observations: plugin snapshots of what the person sees, as versioned context | Not started |
| Spool catalog | Task-scoped MCP discovery exposes the admitted interfaces available to the agent. |
