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

The eight designs above govern backend development. Step one has a headless acceptance case covering decisions, frozen evidence and downstream impact. Step two extends that implementation with independent document revisions, ordered blocks and stable block identities. Step two now has headless coverage for block handoff, evidence-only revisions, retention, and workspace-owner approval of a retained decision version. Step three covers report-bound rejection, registration of decisions reported in meeting records, and delegated publication of a reviewed version to its workspace. A single human response can now confirm document accuracy and approve an exact workspace delivery, with separate receipts for each action. Task-local decision notifications retain separate attempts across failure, acknowledgement loss and explicit retry. Explicit corrections now link both decision receipts and expose the old and new notification outcomes per recipient grant. The notification acceptance case covers late acknowledgements, unopened notices and corrections queued after the decision. The acceptance tables in each design define completion.

The earlier Studio-centered proposal is retained in [repository history](https://github.com/lazyalexander/an/blob/d7572d2/docs/architecture.md). The [Studio protocol](graph-studio.md) documents the existing integration. Domain workflows and their rendering choices belong in the example spools.
