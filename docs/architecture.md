# Architecture

An lets an agent run industrial work for one person. The person faces one input and one output: the macro UI. Everything between them is planned by agents, executed in Apple Container and recorded by An. Painting, CAD and animation are instances of this shape, not separate products.

## Final form

1. **An agent and its subagents drive the work.** The main agent runs in the harness and talks to An through task-scoped MCP. Subagents run as drivers inside containers. A subagent never holds more authority than the agent that started it: it sees only its bound inputs, its granted spools and its task namespace.
2. **The macro UI is the only control surface.** It shows what needs the person and what was produced. Agents generate its content as panel packages. The framework guarantees its shell.
3. **Each part of the work runs in its own container.** Temporary code, temporary artifacts and secrets stay inside the container that needs them. Every transfer between containers crosses an An permission check.
4. **No one draws a pipeline.** There is no predefined graph and no template. Agents decide the next step from the last result. A container may use a fixed internal procedure. Between containers, the graph grows as work happens: it is the execution record and the provenance, not a plan.

## How the parts fit

```text
person ── macro UI ── An host (Rust: graph, versions, permissions, ledger)
                         │
          harness agent ─┤ an_graph_submit / an_graph_state
                         │
             ┌───────────┼──────────────┐
          run nodes   expand nodes   surface / delivery nodes
          (spools)    (subagent        (panels for the person,
                       drivers)         published results)
```

- **Decisions are recorded before they run.** An agent's choice becomes a graph submission with an idempotency key and a base revision. Recovery reads the record and never asks the model again.
- **Drivers are staged, not resident.** An `expand` node runs a driver to completion in a dedicated short-lived instance and returns `subgraph.json`. A surface can name a `continuation` expand node that consumes the person's response. Waiting for the person holds no container. State a driver wants to keep is an output bound to the next driver. A resident driver may later serve as a cache, but correctness depends only on the staged record.
- **The graph is append-only.** Nodes keep immutable IDs. `retry` re-runs failed or cancelled attempts. `supersede` replaces queued work or waiting surfaces. Earlier deliveries can be resumed as a new task's `original` input.
- **Instances are reclaimed by the host.** Settled runs and drivers are unloaded by the scheduler, not by agent discretion.

See [Incremental tasks and Studio](graph-studio.md) for the submission contract and panel packages, and [Permissions](permissions.md) for turns, flows and approvals.

## Macro UI rules

The shell is fixed and always present, whatever an agent generates:

- surfaces waiting for the person, with their panels;
- the current output and published deliveries;
- failures and pending permission requests;
- intervention: approve or deny a request, supersede or retry work, open an earlier version.

Agents fill the rest by submitting surfaces: a panel package and a response schema. The page never exposes prompts, tool calls or the agent loop. The rules for generating panel content are still open.

## Status

| Part | State |
| --- | --- |
| Incremental graph with run, surface, delivery | Implemented |
| Staged container drivers (`expand`, `continuation`) | Implemented |
| Delivery resume as a new task | Implemented |
| Host-side instance reclamation | Implemented for settled runs and drivers |
| Macro UI shell with panels, permissions, deliveries | Implemented; no stop or pause control yet |
| Subagent authority as a strict subset of its parent | Partial: drivers see bound inputs and granted spools; budgets and model credentials are not defined |
| Model-backed subagents inside drivers | Not started: example drivers are deterministic programs |
| Native surfaces | Not started (see below) |
| Turn-level policy to avoid approval fatigue | Not started |
| Reclamation of unreferenced intermediate artifacts | Not started |
| Intent entry from the person | Open question: behavior inference only, or a direct command channel |

## Native surfaces

Pressure painting cannot run in a web panel. Measured on the reference machine: synthetic input reaches the canvas with zero pressure, and macOS delivers the Sidecar Pencil to ordinary applications as a plain mouse. A driver-level injector (Codonic Pad) delivers genuine tablet events to Krita. Pinned versions are in [studio/VERSIONS.md](../studio/VERSIONS.md); the probe evidence is under `studio/spikes/`.

A native surface will therefore dispatch a document to an adapter inside the native application and wait for a save receipt as its response. The legacy instance-mode Studio (`python3 -m orchestrator` without `--task`) already implements this receipt: adopt, re-render and unlock are queued, the Krita adapter saves the live document over its archive, and only its acknowledgement advances state. That mode is frozen and kept only as the reference for the native surface.
