# 功能全景

本表区分代码实现、局部实测与线上验证；各行独立标注最近验证日期、提交/工作区对象、环境与层级，表头不暗示全量复验。

- 首轮实现与本地集成基线见 [2026-11-25 接入记录](changes/20261125-tidy-onboarding.md)（`ec0957f`）。
- 跟版发版加固与 `rebuild` 跳过修复见 [2026-11-25 发版加固记录](changes/20261125-harden-release-workflow.md)（`700e059`、`bc6021d`）。
- 流解析截断修复、模型/档位自动获取、真实邮箱展示、去预算/去校准及独立任务记录 Tab 重构见 [2026-11-25 流解析修复与交互重构记录](changes/20261125-fix-stream-and-ux-redesign.md)。

| ID | 能力 | 实现状态 | 权威入口 | 已验证证据（日期 / 对象 / 环境 / 层级） | 仍待验证 |
| --- | --- | --- | --- | --- | --- |
| CPR-01 | 双 Tab 管理页：检测与巡检配置 + 独立任务记录列表（搜索、多维筛选、快捷标签、页码分页、取消排队），真实邮箱展示，自动拉取宿主全部模型及推理档位 | 已实现 | [管理页](../../web/index.html)、[前端逻辑](../../web/app.js)、[服务路由](../../src/service.rs)、[宿主目录发现](../../src/host.rs) | 2026-11-25 / 本次重构工作区 / macOS ARM64，宿主 `v3.54.5` / 静态（`node --check web/app.js`、`cargo clippy -D warnings`）+ Rust 单测（邮箱优先、原生目录解析与多套餐档位合并、筛选分页）+ 隔离宿主 smoke（`smoke_host.py`）：通过上传、信任、启用、双 Tab 资源、`GET /api/catalog`、`GET /api/runs` 邮箱搜索/来源/状态筛选分页、`POST /api/runs/cancel`、`PUT /api/schedule` 及非法输入 `400` 拒绝。 | 线上真实 OAuth 账号环境下的原生 `/api/admin/accounts/models/catalog` 实测与浏览器端到端交互核验。 |
| CPR-02 | 固定指定账号，不可用时不回落到其他账号 | 已实现 | [宿主适配](../../src/host.rs)、[宿主合同](../../scripts/check_bound_host.sh) | 2026-11-25 / 本次重构工作区与 `ec0957f` / macOS ARM64，Rust `1.99.1`，宿主源码 `45a4d580...` / 单测 + 宿主合同：Rust 桩测试覆盖发送给 A、失败不访问 B、停用账号不发请求；`check_bound_host.sh` 2 条 Core 合同通过。 | 真实线上 OAuth 账号故障场景验收。 |
| CPR-03 | 校验成对参考数据，离线生成三个独立探针，兼容 Responses 稀疏 `content_index`（如 `1024`）及默认推理档位，仅对完整可见文本排序 | 已实现 | [宿主适配](../../src/host.rs)、[指纹层](../../src/fingerprint.rs)、[数据锁](../../data/detector.lock.json) | 2026-11-25 / 本次重构工作区 / macOS ARM64 / 数据校验 + 单测：`detector_assets.py --verify-only` 校验通过；`CPR_REQUIRE_BANK=1 cargo test`（19 项）覆盖离线加载、`ReasoningDelta(0) + TextDelta(1024/1025)` 稀疏索引解析、默认档位省略 `reasoning`、截断/中间件改写流拒绝、76 模型别名匹配及 `tests/known_other.json` 两轮判定链路。 | 线上真实 `gpt-6-sol` 及其他推理模型各档位的端到端实测返回与耗时观察。 |
| CPR-04 | 全模型指纹判定与参考排名：库内模型支持首轮匹配与二轮复测疑似判定，库外模型输出 Top-3 参考排名与概率，无校准模式或手动档案锁阻断 | 已实现 | [指纹层](../../src/fingerprint.rs)、[判定策略](../../src/domain.rs) | 2026-11-25 / 本次重构工作区 / macOS ARM64 / 单测：覆盖库内 `Match` / `Nearby` / `Confirm → Suspect` / `Inconsistent`、库外 `Ranked`、Top-3 概率输出及异常不判疑似。 | 线上真实 OAuth 账号多模型、多推理档位的实测命中率与复测表现。 |
| CPR-05 | 手动检测与定时巡检共用队列（上限 200）、支持 1/6/12/24/48/168 小时周期、错过不补跑、停用跳过旧定时、去重与待执行取消（已移除 24h 探针预算） | 已实现 | [队列与调度](../../src/domain.rs)、[维护回调](../../src/service.rs) | 2026-11-25 / 本次重构工作区 / macOS ARM64 / 单测：Rust 模拟时钟覆盖批量入队（≤ 100）、200 队列上限、去重、不补跑、计划变更/停用跳过待跑复测、待执行取消及重启中断恢复。 | 真实线上账号开启定时巡检的跨周期长期运行验证。 |
| CPR-06 | 设置及历史摘要（≤ 500 条、30 天）跨插件切换、旧版 `v0.1.0-dev.1` 状态迁移与进程重启保留 | 已实现 | [编解码](../../src/state_codec.rs)、[持久化边界](PERSISTENCE.md)、[隔离验收](../../scripts/smoke_host.py) | 2026-11-25 / 本次重构工作区 / macOS ARM64，宿主 `v3.54.5` / 单测 + 隔离 smoke：Rust 500 条历史压缩与 `v0.1.0-dev.1` 状态反序列化迁移单测通过；同一 `v3.54.5` 实例注入现代记录与 `v0.1.0-dev.1` 旧记录，完成 `0.0.0 → 0.1.0-dev.2` 切换及宿主重启，设置、邮箱筛选与迁移记录均完整保留。 | 跨宿主版本升级/回滚、删除实例行为及未来跨 schema 迁移。 |
| CPR-07 | 检查宿主正式版，复用旧包或重建；支持预检、预发布预览、远端摘要核验与断点续跑；`main` 只预警 | 已实现，远端复跑待验收 | [正式版工作流](../../.github/workflows/follow-host.yml)、[main 工作流](../../.github/workflows/host-main.yml)、[发版闸门测试](../../tests/test_release.py) | 2026-11-25 / 本次重构工作区 / macOS ARM64，Python `3.12` / 静态 + 离线单测 + 契约核验：17 项 Python 单测覆盖旧包复用、重建、无旧包短路、`vX.Y.Z-dev.N` 预发布、草稿远端摘要核验、下载回算、残留草稿拦截、幂等续跑与只读 `preflight`；`follow_host.contract` 核验全部 16 项回调契约无问题；`actionlint` 通过。 | 推送后在远端 GitHub Actions 完整实跑三平台构建与发布。 |
| CPR-08 | 生成安装包和摘要，三平台同源验证后发布 | 部分验证（仅本地 macOS ARM64） | [打包脚本](../../scripts/package.py)、[发布条件](../release.md)、[交接快照](HANDOFF.md) | 2026-11-25 / 本次重构工作区 / macOS ARM64，宿主 `v3.54.5` / 本地打包 + 隔离 smoke：`0.1.0-dev.2-aarch64-apple-darwin.tar.gz` 打包校验与真实宿主切换 smoke 通过。 | Linux `x86_64-unknown-linux-gnu`、Linux `aarch64-unknown-linux-gnu` 构建与 smoke 及线上安装验收。 |

## 核验范围

盘点入口包含双 Tab 管理页及路由、自动模型目录发现、邮箱优先账号展示、维护任务、私有状态持久化与向后兼容迁移、安装包与离线数据、跟版工作流与回滚。参考排名不是身份认证，不自动停用账号、改变路由或升级生产。
