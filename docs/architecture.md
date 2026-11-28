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

## Implementation order

The eight designs above govern backend development. Step one has a headless acceptance case covering decisions, frozen evidence and downstream impact. Step two extends that implementation with independent document revisions, ordered blocks and stable block identities. Step two now has headless coverage for block handoff, evidence-only revisions, retention, and workspace-owner approval of a retained decision version. Step three covers report-bound rejection, registration of decisions reported in meeting records, and delegated publication of a reviewed version to its workspace. A single human response can now confirm document accuracy and approve an exact workspace delivery, with separate receipts for each action. Task-local decision notifications retain separate attempts across failure, acknowledgement loss and explicit retry. Explicit corrections now link both decision receipts and expose the old and new notification outcomes per recipient grant. The notification acceptance case covers late acknowledgements, unopened notices and corrections queued after the decision. Step four now persists a primary agent inbox for reviewed block hand-ins and verifies offline receipt across MCP restarts and producer task closure. The primary agent can record a version-bound disposition for each hand-in. Session takeover remains open. The acceptance tables in each design define completion.

The earlier Studio-centered proposal is retained in [repository history](https://github.com/lazyalexander/an/blob/d7572d2/docs/architecture.md). The [Studio protocol](graph-studio.md) documents the existing integration. Domain workflows and their rendering choices belong in the example spools.

## Backend change guide

| When changing | Start here | Contract to preserve |
| --- | --- | --- |
| Workspace binding, task lifetime or primary inbox | [Workspace modules](../src/workspace.rs) | Inbox entries refer to frozen blocks and survive producer closure. |
| A provenance command | [Operation policy](../src/provenance/operation.rs) and [dispatch](../src/provenance/mod.rs) | Declare MCP availability explicitly. Authorization still checks the caller and scope. |
| A decision, correction or notification | [Decision modules](../src/provenance/decisions/mod.rs) | Keep decision receipts and notification attempts independently identifiable. |
| Revision collection | [Retention queries](../src/provenance/retention.rs) and [collector](../src/store/gc.rs) | Keep reachable material and evidence, including evidence visible through active grants. |
| A graph node kind | [Admission](../src/graph/admission.rs) and [scheduler](../src/graph/scheduler.rs) | Validate before recording the graph revision. Commit permission consumption together with its result. |

Run the architecture test after changing module boundaries. Its [TOML contract](../architecture.toml) covers the domain dependencies, CLI and MCP entry points, inbox storage access, and collector boundary.
