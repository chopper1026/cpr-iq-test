<!-- tidy:start -->
## 项目开发治理

先读取 `.tidy.json` 与 `docs/development/INDEX.md`。提交信息遵守 `docs/development/COMMIT_CONVENTION.md`。
使用 `$tidy` 执行提交前文档/架构/功能检查；GitNexus 是否参与由项目配置决定。
tidy 更新后使用 `$tidy-rebuild` 评估并对齐现有治理；制定或维护自动化发版流程使用 `$tidy-release`；独立多维代码评审使用 `$tidy-review`。
本次记录放 `docs/development/changes/`；过程材料遵守已确认的可见性，不取消既有忽略边界。
普通计划、设计草稿、图片、过程决策和验收摘要放 `docs/development/` 对应分类；工具固定路径优先，稳定共享文档另按项目政策维护。
提交前按规范归类全部实际变化并维护统一入口；仅暂存本任务内容。
现行操作说明与源码/配置对齐；变动涉及多语言、手册、契约或持久化边界时同步核对其权威入口。功能实现与验证证据分开维护，局部结果不追认为全量验收。
已配置 document_checks 时，check 同时核对文档引用；语义、测试执行与验收仍须各自证据。
每次提交前检查并清理本任务的临时日志、断点、测试入口及调试桩；保留正式日志/回归测试，清理后验证并运行 check --staged。
GitNexus 已停用：tidy 仍执行文档、架构、功能和暂存快照检查，不运行 GitNexus；若已有历史 `docs/plans/*gitnexus-plan*.md` 保持原位不改写；需要时按项目决定重新启用。
不因治理流程获得 push、历史改写、发布、部署或硬件操作授权。现有规则冲突先询问用户。
<!-- tidy:end -->
