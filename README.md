<p align="center"><img src="assets/icon.png" alt="An" width="96"></p>

# An

English · [简体中文](README.zh-CN.md)

**An tracks how artifacts move through a workspace.** From a sentence in a published announcement, a reviewer can follow a block reference to the exact decision it cites. Starting from that decision, the reviewer can find which documents used it and inspect the evidence they have permission to read.

The agent plans the work and reviews the results through the task MCP gateway. People or explicitly delegated agents make decisions. An checks permissions, runs headless programs in digest-pinned spool images on Apple Container, and records the versions each run received and produced. Spool authors define how their documents are processed and how their blocks cite sources.

Build the Rust CLI and stdio MCP gateway with `cargo build --release --locked`. Spool execution uses Apple Container.

The [headless minutes example](examples/minutes/provenance.py) follows one decision through an announcement, an action list and a later revision. The [Markdown review example](docs/graph-studio.md) shows how Studio presents a choice and lets a person retrieve the revised file and its patch.

Pressure drawing, CAD, image providers and the visual composition benchmark live on the [visual experiment branch](https://github.com/lazyalexander/an/tree/experiment/visual-studio). Published files can still be opened, exported or used as the original input of a new task on `dev`. Run unfinished visual workflows on the experiment branch.

[Architecture](docs/architecture.md) · [Spool image format](docs/spool-images.md) · [Workspace bindings](docs/workspaces.md) · [Permissions](docs/permissions.md) · [Incremental tasks and Studio](docs/graph-studio.md) · [Configuration](docs/configuration.md)
