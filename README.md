<p align="center"><img src="assets/icon.png" alt="An" width="96"></p>

# An

English · [简体中文](README.zh-CN.md)

**A workspace for agent-produced artifacts.**

An connects an agent to a workspace of spools through MCP. Each spool is a container image that packages a headless program with the instructions for using it. The agent chooses which spool to run and where its output should go next. An schedules execution with Apple Container and checks permissions when data crosses a boundary.

A spool can pass a frozen artifact directly to another spool or publish it to the workspace. The harness handles user approval. Developers maintain their own spool images.

Build the Rust CLI and stdio MCP gateway with `cargo build --release --locked`. Spool execution uses Apple Container.

The host and Studio live in `src/` and `studio/`. Markdown review is the reference workflow on `dev`. It exercises a container driver, a model decision, human review and publication of a revised document with an applicable diff. Minutes provides a second, two-spool check of information flow.

Pressure drawing, CAD, image providers and the visual composition benchmark live on the [visual experiment branch](https://github.com/lazyalexander/an/tree/experiment/visual-studio). Published files can still be opened, exported or used as the original input of a new task on `dev`. Run unfinished visual workflows on the experiment branch.

[Architecture](docs/architecture.md) · [Spool image format](docs/spool-images.md) · [Workspace bindings](docs/workspaces.md) · [Permissions](docs/permissions.md) · [Incremental tasks and Studio](docs/graph-studio.md) · [Configuration](docs/configuration.md)
