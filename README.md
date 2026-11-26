<p align="center"><img src="assets/icon.png" alt="An" width="96"></p>

# An

English · [简体中文](README.zh-CN.md)

**A thin adapter between agents and industrial-grade pipelines.**

An connects an agent to a workspace of spools through MCP. Each spool is a container image that packages a headless program with the instructions for using it. The agent chooses which spool to run and where its output should go next. An schedules execution with Apple Container and checks permissions when data crosses a boundary.

A spool can pass a frozen artifact directly to another spool or publish it to the workspace. The harness handles user approval. Developers maintain their own spool images.

Build the Rust CLI and stdio MCP gateway with `cargo build --release --locked`. Spool execution uses Apple Container.

The framework lives in `src/` and `studio/`; host services that need the GPU live in `services/`. Application spools and demo entry points live under `examples/`.

[Architecture](docs/architecture.md) · [Spool image format](docs/spool-images.md) · [Workspace bindings](docs/workspaces.md) · [Permissions](docs/permissions.md) · [Incremental tasks and Studio](docs/graph-studio.md) · [Configuration](docs/configuration.md)
