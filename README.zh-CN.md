<p align="center"><img src="assets/icon.png" alt="An" width="96"></p>

# An

[English](README.md) · 简体中文

**An 管理工作区中的产物流。** 从已发布公告中的一句话，审阅者可以沿块引用找到当时采纳的那版决定。也可以从决定出发，查出哪些文档采用了它，并在授权范围内核对运行证据。

Agent 通过任务级 MCP 编排工作、检验结果，人或获得明确委托的 agent 作决定。An 检查权限，通过 Apple Container 运行固定镜像版本的无头 spool，并记录每次执行实际收到和产出的版本。文档如何处理、其中的块引用了哪些来源，由相应的 spool 声明。

运行 `cargo build --release --locked` 构建 Rust CLI 和 stdio MCP 网关。运行 spool 需要 Apple Container。

[会议纪要无头示例](examples/minutes/provenance.py)展示一版决定如何被公告和行动清单采用，以及单独修订公告后如何追查旧交付。[Markdown 审阅示例](docs/graph-studio.md)展示同一后端如何通过 Studio 请人作选择，并交付修订文档和补丁。

压感绘画、CAD、图像服务和视觉组合评测保存在[视觉实验分支](https://github.com/lazyalexander/an/tree/experiment/visual-studio)。`dev` 可以打开和导出已发布的视觉文件，也能用原交付创建新任务。尚在编辑的视觉任务请在实验分支继续。

[架构](docs/architecture.md) · [Spool 镜像规范](docs/spool-images.md) · [工作区绑定](docs/workspaces.md) · [权限机制](docs/permissions.md) · [增量任务与宏 UI](docs/graph-studio.md) · [配置](docs/configuration.md)
