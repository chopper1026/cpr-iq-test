# 20261125-tidy-onboarding

- 创建时间：2026-11-25T20:47:27+08:00
- 状态：implemented
- 关联任务/功能：首次治理接入；以现有工作区建立 CPR-01 至 CPR-08 的证据基线。

## 背景与目标

在已有插件实现、尚无 Git 历史的目录建立提交规范和开发入口。记录本机可复核的基线，明确区分本地验证、远端发版和授权账户实测；不把治理接入当作提交或发布授权。

## 分类变化

### 文档

- 使用中文 Conventional Commits 与分类正文，按宿主、维护、指纹、管理页、发版登记 scope。
- 建立[开发总入口](../INDEX.md)、[架构与持久化边界](../ARCHITECTURE.md)、[功能全景](../FEATURES.md)和分类索引。既有[项目 README](../../../README.md)、[发版与回滚手册](../../release.md)、[第三方组件与许可](../../../THIRD_PARTY.md)继续维持原位和原有权威。

### 测试

- 在 macOS ARM64 使用已缓存的锁定 Rust 及依赖复跑插件测试、宿主指定账号合同、发布闸门测试和隔离宿主 smoke；不修改测试或发版工作流。

### 维护与回退

- 初始化本地 `main`，写入 tidy 配置、代理指令和提交模板。关闭 GitNexus，不运行图谱或配置 hook。
- 将本机 tidy 记录纳入忽略边界，保留既有构建、截图和调试目录的忽略规则；校验现行 Markdown、锁定 JSON 和脚本引用。

## 决策与资料

用户确认停用 GitNexus，并确认接入预览。保留既有文档路径，不备份迁移，不把本地实验材料变为共享记录。可共享的是整理后的架构、决策和验证摘要；安装包和截图继续留本地。仓库没有首次提交，该基线不引用尚不存在的项目 Git SHA。

## 架构与功能影响

运行时和交付流程无变化：不改 Rust、页面、清单、数据锁、测试、脚本或发版工作流。新增架构说明和验收表是对现有行为和已执行检查的记录，不改变校准、预算或发布条件。

## 验证与未验证项

本地核验日期 2026-11-25（Asia/Shanghai），macOS ARM64，Rust `1.99.1`，Python `3.12`。使用现有开发包，不重新打包。

| 层级 | 入口与结果 |
| --- | --- |
| 工作区与静态检查 | `cargo fmt --all -- --check`、`cargo clippy --offline --locked --all-targets -- -D warnings`、`node --check web/app.js`、三份 Actions YAML 的 `actionlint` 均通过。 |
| 参考数据 | `python3 scripts/detector_assets.py --verify-only` 确认修订 `e0472890bea827d15ca883636a77d4c10bbe843b` 的两份参考数据摘要匹配。`--profiles --require-validated` 按预期被无授权实测证据的闸门拒绝。 |
| 插件源码测试 | `CPR_REQUIRE_BANK=1 cargo test --offline --locked --all-targets`：16 项通过，含 A 不可用时无 B 调用、完整流、风控与失败分类、调度和参考语料异标签链路。有意用参考语料验证链路，不估计真实账号准确率。 |
| 宿主源码合同 | `scripts/check_bound_host.sh` 对缓存的宿主 `45a4d580c7e1ac2b2c0308d72ae5018148f2516a` 运行 2 条 Core 合同，均通过。 |
| 发布闸门回归 | `python3 -m unittest discover -s tests -p 'test_*.py'`：9 项通过，覆盖复用旧包、重建包及验证失败阻断。工作流静态与决策测试不代表实际发版。 |
| 安装与状态集成 | [隔离 smoke](../../../scripts/smoke_host.py)在真实宿主 `v3.54.5` 上传并启用，核对管理路由、资源、目录与设置；同一实例的 `0.0.0` 测试包切至 `0.1.0-dev.1`、随后重启，均保留设置和合成历史。候选为 `cpr-iq-test.account-fingerprint-0.1.0-dev.1-aarch64-apple-darwin.tar.gz`，校验通过，SHA-256 `d6791b714f968122155770f920c94ef6487e5d9a5a8be0cb32ef852cc9d9adfe`。没有真实 OAuth 或模型调用。 |
| 治理检查 | `tidy sync` 和 `tidy check` 核对工作区分类索引和现行文档引用；`--staged` 未运行（无待提交快照）。GitNexus 未启用。 |

## 开发残留清理

本次没有新增运行时代码、调试入口、断点或持久化测试日志。工作区快速扫到的 `build.rs` 输出是 Cargo 的重编译指令，予以保留。保留正式测试、测试夹具和隔离 smoke；原有本地安装包、截图、缓存及诊断资料继续忽略，不删除。首次暂存实际变更后再执行 `check --staged` 的调试残留门禁。

## 风险与后续

只有 `gpt-6-sol / low` 校准排名对开发预览开放。授权正常 OAuth 账号的正例、实际差值阈值、Linux x86_64 和 ARM64 实机/CI、远端 GitHub Actions 发包及生产切换仍未验证。不得把本地异标签或启动成功作为开放正式检测的理由；参照[发布闸门与校准步骤](../../release.md)补齐真实证据。无暂存、首次提交、推送或发布。
