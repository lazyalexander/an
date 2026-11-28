<p align="center"><img src="assets/icon.png" alt="An" width="96"></p>

# An

[English](README.md) · 简体中文

**管理 agent 工作产物的工作区。**

Agent 通过 MCP 连接 An，使用工作区中的 spool。每个 spool 都是一个容器镜像，封装了无头程序和使用说明。Agent 决定调用哪个 spool，以及把产物交给谁。An 通过 Apple Container 调度执行，并在数据跨越边界时检查权限。

一个 spool 可以把产物快照定向交给另一个 spool，也可以将它发布到工作区。需要用户授权时，由 harness 发起确认。开发者自行维护 spool 镜像。

运行 `cargo build --release --locked` 构建 Rust CLI 和 stdio MCP 网关。运行 spool 需要 Apple Container。

宿主和宏 UI 位于 `src/` 和 `studio/`。`dev` 以 Markdown 审阅为参考流程，覆盖容器驱动、本地模型判断、人工决定和交付。交付包含修订文档和可以应用到原稿的补丁。会议纪要示例用两个 spool 验证跨容器授权。

压感绘画、CAD、图像服务和视觉组合评测保存在[视觉实验分支](https://github.com/lazyalexander/an/tree/experiment/visual-studio)。`dev` 可以打开和导出已发布的视觉文件，也能用原交付创建新任务。尚在编辑的视觉任务请在实验分支继续。

[架构](docs/architecture.md) · [Spool 镜像规范](docs/spool-images.md) · [工作区绑定](docs/workspaces.md) · [权限机制](docs/permissions.md) · [增量任务与宏 UI](docs/graph-studio.md) · [配置](docs/configuration.md)
