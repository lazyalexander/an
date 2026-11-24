<p align="center"><img src="assets/icon.png" alt="An" width="96"></p>

# An

[English](README.md) · 简体中文

**从 Agent 到工业级流水线的薄适配层。**

Agent 通过 MCP 连接 An，使用工作区中的 spool。每个 spool 都是一个容器镜像，封装了无头程序和使用说明。Agent 决定调用哪个 spool，以及把产物交给谁。An 通过 Apple Container 调度执行，并在数据跨越边界时检查权限。

一个 spool 可以把产物快照定向交给另一个 spool，也可以将它发布到工作区。需要用户授权时，由 harness 发起确认。开发者自行维护 spool 镜像。

运行 `cargo build --release --locked` 构建 Rust CLI 和 stdio MCP 网关。运行 spool 需要 Apple Container。

[架构](docs/architecture.md) · [Spool 镜像规范](docs/spool-images.md) · [工作区绑定](docs/workspaces.md) · [权限机制](docs/permissions.md) · [增量任务与宏 UI](docs/graph-studio.md)
