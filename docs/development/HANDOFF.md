# 阶段交接与验证快照

证据日期：2026-11-26（Asia/Shanghai）。

本文记录当前多平台验证矩阵、分层证据与授权边界；软件项目不涉及硬件槽位字段。未执行层级均标为「未验证」，历史提交的局部通过不追认为全平台或当前构建通过。

## 当前状态与入口

- **阶段目标**：完成前端全面复用 Codex Proxy 样式（live `--z-*` 主题令牌 + 自定义 `ZSelect` 下拉框等组件）、新增「账号实时状态」页（展示队列进度与最近一次检测情况）、定时巡检新增每日固定时间与时区支持、新增可选「指纹异常自动停用并写明检测结果与时间备注」机制，并启用正式版（`v0.1.0` 起）与双上游（`codex-proxy-rs` + `lm-detector`）自动同步跟版；待管理员确认推送、开启 `CPR_ENABLE_PUBLISH=true` 后触发首个正式版 `v0.1.0` 发布。
- **权威入口**：
  - 安装与日常使用：[README.md](../../README.md)
  - 隔离 smoke、正式版双上游自动跟版与回滚：[docs/release.md](../release.md)
  - 双上游自动同步脚本：[sync_upstreams.py](../../scripts/sync_upstreams.py)
  - 系统边界与模块契约：[ARCHITECTURE.md](ARCHITECTURE.md)
  - 持久化、配额与备份边界：[PERSISTENCE.md](PERSISTENCE.md)
  - 功能实现与分层证据：[FEATURES.md](FEATURES.md)
  - 开发治理导航：[INDEX.md](INDEX.md)
- **当前主要待办与风险**：
  1. 尚未推送本次提交、设置仓库变量 `CPR_ENABLE_PUBLISH=true` 或在 GitHub Actions 触发首个正式版 `v0.1.0` 发布（需用户明确授权；上游最新 CPR 正式版已为 `v3.55.2`，推送并开启自动发布后 `sync-upstreams` 将自动同步至 `v3.55.2` 并发布 `v0.1.0`）。
  2. 尚未在线上真实 OAuth 账号环境实测账号实时状态批量展示、一键检测与每日固定时间跨日触发。

## 构建与运行身份

- **插件版本**：`0.1.0`（`plugin.json` 与 `Cargo.toml` 一致；历史预发布 `v0.1.0-dev.1..3`，本次起启用正式版本，首个正式版候选 `v0.1.0`）。
- **锁定外部契约**：
  - 当前仓库基线宿主：`zyycn/codex-proxy-rs` `v3.54.5`（commit `45a4d580c7e1ac2b2c0308d72ae5018148f2516a`，`engines.codex-proxy-rs: ~3.54.5`）；线上只读预检确认最新稳定宿主为 `v3.55.2`（`add1f9f40f79781823475b4ed0f459d3b8c9cb9f`，契约无破损，开启自动发布后由 `sync-upstreams` 自动同步）。
  - 参考检测库：`Ikaleio/lm-detector` commit `e0472890bea827d15ca883636a77d4c10bbe843b`（`unified_bank.json` 与 `shared_detector.bin` 锁于 `data/detector.lock.json`；与上游 `main` 同步一致）。
  - 工具链：Rust `1.99.1`（`rust-toolchain.toml`）、Python `3.12+`。
- **三平台产物与验证矩阵**：

| 目标平台三元组 | CI runner | 最近已知构建身份 | Linux `maxGlibc` | 隔离宿主 smoke 状态 |
| --- | --- | --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | 2026-11-26 本机正式版候选包 `0.1.0-aarch64-apple-darwin.tar.gz`（位于 `.work/smoke-dist/`，未入库） | 不适用 | 本机 `v3.54.5` 通过（含 `0.0.0 → 0.1.0` 状态切换、账号实时状态、每日固定时间持久化、旧版迁移、重启及 Chromium 明暗主题/下拉框验证）；远端 `macos-15` runner **未验证** |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | 未构建 | 未验证 | 未验证 |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | 未构建 | 未验证 | 未验证 |

## 验证证据

| 层级 | 当次实际操作 / 证据位置 | 日期 / 对象 / 环境 | 结果与覆盖限制 |
| --- | --- | --- | --- |
| 源码与配置静态检查 | `cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings`、`node --check web/app.js`、`actionlint`、`tidy.py audit-docs`、`tidy.py check` | 2026-11-26 / 本次工作区 / macOS ARM64 | 通过；静态与文档检查不代表线上运行验收。 |
| 单元与宿主合同测试 | `CPR_REQUIRE_BANK=1 cargo test --locked --all-targets`（25 项）、`python3 -m unittest discover -s tests -p 'test_*.py'`（22 项）、`scripts/detector_assets.py --verify-only`、`sync_upstreams.py` 只读检测（含 CPR `v3.55.2` 16 项回调契约无破损） | 2026-11-26 / 本次工作区 / macOS ARM64，Rust `1.99.1`，Python `3.12` | 通过；无真实线上 OAuth 调用。`scripts/check_bound_host.sh`（2 条 Core 合同）基线已于 `ec0957f` 验证。 |
| 产品构建、隔离宿主 smoke 与 Chromium iframe 验收 | `scripts/package.py --version 0.1.0` + `scripts/smoke_host.py --docker-services`（`v3.54.5`，`0.0.0 → 0.1.0` 含 `GET /api/accounts/status`、`autoDisableOnSuspect` 独立/组合持久化、每日固定时间与旧版迁移）+ Chromium 实测浅色/暗黑皮肤同步、「指纹异常自动停用」即时开关、`ZSelect` 下拉框与桥接保存（0 Console 错误） | 2026-11-26 / 本次工作区 / macOS ARM64 | 通过；Linux 两平台与线上真实 OAuth 调用（含实账号 `Suspect` 自动停用实跑）**未验证**。 |
| 远端 Release 摘要与双上游自动同步实跑 | `.github/workflows/follow-host.yml`、`scripts/sync_upstreams.py`、`scripts/release_gate.py publish`、`publish-receipt.json` | — | **未验证**（仅有 22 项离线桩单测与只读联机预检覆盖，待推送并启用 `CPR_ENABLE_PUBLISH=true` 后实跑）。 |
| 线上真实 OAuth 实例验收 | 管理员安装/切换 `v0.1.0` 后实测实时状态、一键检测与每日定时巡检 | — | **未验证**（待首个正式版发布后线上复测）。 |

## 后续行动与授权边界

1. **只读本地预检**：`python3 scripts/release_gate.py preflight --online --repository chopper1026/cpr-iq-test` 与 `python3 scripts/sync_upstreams.py --work .work/sync`。
2. **启用正式版自动发布（需管理员明确授权）**：推送本次提交至 `origin/main`，在 GitHub 仓库变量中设置 `CPR_ENABLE_PUBLISH=true`，然后手动触发一次 **Follow CPR and lm-detector releases**（或等待下一个 30 分钟定时槽）完成 `v3.55.2` 同步、三平台构建/smoke 与正式版 `v0.1.0` 发布。
3. **线上升级注意**：在宿主管理页使用 **切换版本（`switch-version`）** 或 GitHub 检查更新升级现有实例，无需删除实例；旧版 `v0.1.0-dev.*` 设置与历史记录将自动兼容迁移。
4. **授权边界**：本交接快照不授予 `git push`、修改仓库变量、创建远端 Release、修改 `cpr-compatibility`、写入 OAuth 凭据或生产实例切换权限；所有外部副作用均须独立明确授权。
