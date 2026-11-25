# 功能全景

本表区分代码实现、局部实测与生产开放；各行独立标注最近验证日期、提交/工作区对象、环境与层级，表头不暗示全量复验。

- 首轮实现与本地集成基线见 [2026-11-25 接入记录](changes/20261125-tidy-onboarding.md)（首次提交前工作区及 `ec0957f`，macOS ARM64，宿主 `v3.54.5`，无真实 OAuth 调用）。
- 后续跟版发版加固与 `rebuild` 跳过修复见 [2026-11-25 发版加固记录](changes/20261125-harden-release-workflow.md)（`700e059` 与 `bc6021d`）。

| ID | 能力 | 实现状态 | 权威入口 | 已验证证据（日期 / 对象 / 环境 / 层级） | 仍待验证 |
| --- | --- | --- | --- | --- | --- |
| CPR-01 | 管理员查看目录、设置和历史，提交手动单测或批量校准 | 已实现（开发预览） | [管理页](../../web/index.html)、[路由](../../src/service.rs)、[操作说明](../../README.md) | 2026-11-25 / 首次接入工作区与 `ec0957f` / macOS ARM64，宿主 `v3.54.5` / 静态（`node --check web/app.js`）+ 隔离宿主 smoke（`smoke_host.py`）：通过上传、信任、启用、管理路由、静态资源、空账号目录、设置读写及不支持档位 `422` 拒绝。 | 实际 OAuth 账号选择到模型返回的端到端调用。 |
| CPR-02 | 固定指定账号，不可用时不回落到其他账号 | 已实现 | [宿主适配](../../src/host.rs)、[宿主合同](../../scripts/check_bound_host.sh) | 2026-11-25 / 首次接入工作区与 `ec0957f` / macOS ARM64，Rust `1.99.1`，宿主源码 `45a4d580...` / 单测 + 宿主合同：Rust 桩测试覆盖发送给 A、失败不访问 B、停用账号不发请求；`check_bound_host.sh` 2 条 Core 合同通过。 | 真实生产/测试 OAuth 账号运行验收。 |
| CPR-03 | 校验成对参考数据，离线生成三个独立探针，仅对完整可见文本排序 | 已实现 | [指纹层](../../src/fingerprint.rs)、[数据锁](../../data/detector.lock.json) | 2026-11-25 / 首次接入工作区与 `ec0957f` / macOS ARM64 / 数据校验 + 单测：`detector_assets.py --verify-only` 确认 `e0472890...` 两份资源 SHA-256 匹配；`CPR_REQUIRE_BANK=1 cargo test`（16 项）覆盖离线加载、完整/截断/中间件改写流与 `tests/known_other.json` 参考语料两轮判定链路。 | 非参考语料的真实上游模型回复分布（参考语料回归不代表独立准确率评估）。 |
| CPR-04 | 已校准档位的正式结论及必要时复测 | 逻辑已实现，正式开放被阻断 | [档位定义](../../data/profiles.json)、[判定策略](../../src/domain.rs)、[校准流程](../release.md) | 2026-11-25 / 首次接入工作区与 `ec0957f` / macOS ARM64 / 单测 + 闸门校验：`gpt6-sol-low`（`gpt-6-sol / low`）为 `validation: null`，`detector_assets.py --profiles --require-validated` 按预期拒绝；Rust 单测覆盖版本绑定、差值门槛、取消/限流/截断不判疑似。 | 授权健康 OAuth 账号的正例校准、实际 `minimumMarginBps` 阈值确定及双方向（正例与异标签）实测通过。 |
| CPR-05 | 手动和计划共享队列、额度、停用和到期去重 | 逻辑已实现，定时入口受档位限制 | [队列与预算](../../src/domain.rs)、[维护回调](../../src/service.rs) | 2026-11-25 / 首次接入工作区与 `ec0957f` / macOS ARM64 / 单测：Rust 模拟时钟覆盖 1–10 批量、30 队列上限、去重、不补跑、滚动 24h 预算耗尽、停用跳过待跑复测及重启中断恢复。 | 真实 OAuth 账号开启定时巡检的长期运行验证。 |
| CPR-06 | 设置及历史摘要跨插件切换与进程重启保留 | 已实现 | [编解码](../../src/state_codec.rs)、[持久化边界](PERSISTENCE.md)、[隔离验收](../../scripts/smoke_host.py) | 2026-11-25 / 首次接入工作区 / macOS ARM64，宿主 `v3.54.5` / 单测 + 隔离 smoke：编解码往返单测通过；同一 `v3.54.5` 实例从 `0.0.0` 测试包切至 `0.1.0-dev.1` 并重启，设置和合成历史均保留。 | 跨宿主版本升级/回滚、删除实例行为及未来跨 schema 迁移。 |
| CPR-07 | 检查宿主正式版，复用旧包或重建；支持预检、预发布预览、远端摘要核验与断点续跑；`main` 只预警 | 代码已实现，远端工作流待验收 | [正式版工作流](../../.github/workflows/follow-host.yml)、[main 工作流](../../.github/workflows/host-main.yml)、[发版闸门测试](../../tests/test_release.py) | 2026-11-25 / `700e059` 与 `bc6021d` / macOS ARM64，Python `3.12` / 静态 + 离线单测 + 只读预检：18 项 Python 单测覆盖旧包复用、重建、无证据/契约失败前置短路、`vX.Y.Z-dev.N` 预发布（不写兼容表）、草稿远端摘要不符阻断转正、下载回算、残留草稿拦截、已发布同源幂等续写兼容表、断点 run 来源校验与只读 `preflight`；3 份工作流 `actionlint` 通过；`bc6021d` 修复了 run `45629428949` 检出的 `old` 跳过时 `rebuild` 被误跳过问题。 | 推送 `bc6021d` 后在远端 GitHub Actions 完整实跑、Cargo/数据缓存命中、Linux `maxGlibc` 实测、远端 Prerelease/正式发布与断点续跑实测。 |
| CPR-08 | 生成安装包和摘要，三平台同源验证后发布 | 部分验证（仅本地 macOS ARM64） | [打包脚本](../../scripts/package.py)、[发布条件](../release.md)、[交接快照](HANDOFF.md) | 2026-11-25 / 首次接入工作区 / macOS ARM64，宿主 `v3.54.5` / 本地打包 + 隔离 smoke：开发包 `0.1.0-dev.1-aarch64-apple-darwin.tar.gz`（SHA-256 `d6791b71...`）校验与安装通过。 | Linux `x86_64-unknown-linux-gnu`、Linux `aarch64-unknown-linux-gnu` 构建与 smoke、三平台同源发布及生产实例切换。 |

## 核验范围

盘点入口包含管理静态页及路由、维护任务、私有状态持久化边界、安装包与离线数据、跟版工作流、人工校准与回滚。参考排名不是身份认证，也无法证明同一模型内部能力是否变化。没有自动停用账号、改变路由或升级生产的功能。
