# 项目开发总入口

- [Git 提交规范](COMMIT_CONVENTION.md)
- [整体架构](ARCHITECTURE.md)
- [功能全景](FEATURES.md)
- [方案、计划、设计、图片与开发历程分类索引](CATALOG.md)
- [插件安装、发版、回滚与许可入口](../README.md)

本机资料与回执：`docs/local/tidy/INDEX.md`（sync 后生成，不入库）。

总入口仅作导航；当前能力与验证状态见功能表，历史变化见分类索引，不在此逐次追加任务摘要。

## 开发记忆与提交闭环

- 已确认提交策略：`new`；记录可见性：`tracked`。
- 文档位置：保留根 README、第三方说明、许可证与现行发版手册；只增加导航，不迁移原文。
- 本次变更记录位置：`docs/development/changes/`；普通过程资料在 `docs/development/` 下按 plans/designs/decisions/assets/validation 分类，工具固定路径保留。
- 整理过的开发记录、契约和验证摘要可纳入版本控制；不收入本机构建缓存、安装包、参考数据缓存、调试日志或截图。忽略边界以[仓库忽略规则](../../.gitignore)为准。
- 提交前完成分类变化、验证证据、索引与架构/功能影响记录，并与相关代码一同提交。
- 提交前清理本任务遗留的临时日志、断点和测试入口，保留正式日志与回归测试；清理后验证并运行 check --staged。
- GitNexus 已停用；本机真实提交回执位于 `docs/local/tidy/COMMITS.md`，运行后生成且不入库。
- 当前 SHA 不写入同一提交的记录；用稳定记录 ID 和 Git 历史关联，避免递归提交。
- 架构、功能与记录的语义必须根据源码和实际证据维护，目录脚本不能自动证明完整。
