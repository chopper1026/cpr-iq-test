# 阶段交接与验证快照

证据日期：2026-11-26（Asia/Shanghai）。

本文记录当前多平台验证矩阵、分层证据与授权边界；软件项目不涉及硬件槽位字段。未执行层级均标为「未验证」，历史提交的局部通过不追认为全平台或当前构建通过。

## 当前状态与入口

- **阶段目标**：已完成正式版 `v0.1.0`（宿主 `v3.55.2`，提交 `a373b57`）三平台发布；当前工作区修复了定时巡检开关切页丢失、首次开启缺账号无首屏提示、轮询覆盖刚保存设置及 `PUT /schedule` 仅切换自动停用误触权限复核的问题，准备提交并等待用户授权推送及触发正式版 `v0.1.1` 发布。
- **权威入口**：
  - 安装与日常使用：[README.md](../../README.md)
  - 隔离 smoke、正式版双上游自动跟版与回滚：[docs/release.md](../release.md)
  - 双上游自动同步脚本：[sync_upstreams.py](../../scripts/sync_upstreams.py)
  - 系统边界与模块契约：[ARCHITECTURE.md](ARCHITECTURE.md)
  - 持久化、配额与备份边界：[PERSISTENCE.md](PERSISTENCE.md)
  - 功能实现与分层证据：[FEATURES.md](FEATURES.md)
  - 开发治理导航：[INDEX.md](INDEX.md)
- **当前主要待办与风险**：
  1. 尚未将本次修复提交推送至 `origin/main` 或触发远端 `follow-host.yml` 构建发布 `v0.1.1`（需用户明确授权；联机只读预检已确认 `nextCandidateVersion = 0.1.1`，上游 CPR `v3.55.2` 与 `lm-detector` 均无新增上游差异）。
  2. 尚未对本次修改执行真实宿主安装包 smoke 与线上升级 `v0.1.1` 后的实测验证。

## 构建与运行身份

- **插件版本系列**：`plugin.json` 与 `Cargo.toml` 声明基线 `0.1.0`；已发布正式版 `v0.1.0`（`2026-11-26T01:29:08Z`，提交 `a373b57fe45c264b443913d0735757cf1c394ce3`），下一正式候选版本 `0.1.1`（由 `follow_host.py` 自动按 `0.1.x` 递增产出）。
- **锁定外部契约**：
  - 宿主基线：`zyycn/codex-proxy-rs` `v3.55.2`（commit `add1f9f40f79781823475b4ed0f459d3b8c9cb9f`，`engines.codex-proxy-rs: ~3.55.2`）。
  - 参考检测库：`Ikaleio/lm-detector` commit `e0472890bea827d15ca883636a77d4c10bbe843b`（`unified_bank.json` 与 `shared_detector.bin` 锁于 `data/detector.lock.json`）。
  - 工具链：Rust `1.99.1`（`rust-toolchain.toml`）、Python `3.12+`。
- **三平台产物与验证矩阵（针对本次定时巡检修复工作区）**：

| 目标平台三元组 | CI runner | 最近已知构建身份 | Linux `maxGlibc` | 隔离宿主 smoke 状态 |
| --- | --- | --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | 已发布基线 `v0.1.0`（`a373b57`）；本次修复尚未构建 `0.1.1` | 不适用 | 本次修复 **未验证**（待 CI 构建并执行 `v3.55.2` 隔离 smoke） |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | 已发布基线 `v0.1.0`（`a373b57`）；本次修复尚未构建 `0.1.1` | 未验证（当前修复） | 本次修复 **未验证**（待 CI 构建并执行 `v3.55.2` 隔离 smoke） |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | 已发布基线 `v0.1.0`（`a373b57`）；本次修复尚未构建 `0.1.1` | 未验证（当前修复） | 本次修复 **未验证**（待 CI 构建并执行 `v3.55.2` 隔离 smoke） |

## 验证证据

| 层级 | 当次实际操作 / 证据位置 | 日期 / 对象 / 环境 | 结果与覆盖限制 |
| --- | --- | --- | --- |
| 源码与配置静态检查 | `cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings`、`node --check web/app.js`、`tidy.py check --staged` | 2026-11-26 / 本次修复提交 / macOS ARM64 | 通过；静态与文档检查不代表线上运行验收。 |
| 单元测试 | `CPR_REQUIRE_BANK=1 cargo test --locked --all-targets`（26 项，含 `schedule_access_check_runs_on_enable_or_target_change_but_not_guard_only_toggle`）、`python3 -m unittest discover -s tests -p 'test_*.py'`（22 项） | 2026-11-26 / 本次修复提交 / macOS ARM64，Rust `1.99.1`，Python `3.12` | 通过；无真实线上 OAuth 调用。`scripts/check_bound_host.sh` 合同基线已在 `v0.1.0` CI 验证。 |
| 浏览器前端交互验证 | 无头 Chrome 驱动 `web/index.html` + `web/app.js`（模拟 `window.codexProxyPlugin` 桥接，覆盖 0 账号首次开开关引导、一键全选开启、重载持久化、即时开关启停、未保存横幅、`403` 回弹与慢轮询竞态，0 Console 错误） | 2026-11-26 / 本次修复提交 / macOS ARM64 Chrome | 通过；使用模拟宿主桥接，不等同于真实宿主二进制打包验证。 |
| 联机只读发布预检 | `scripts/release_gate.py preflight --online --repository chopper1026/cpr-iq-test` | 2026-11-26 / 本次修复提交 / macOS ARM64 | 确认上游 CPR `v3.55.2`、`lm-detector` 无新变化、`ledgerSourceCurrent=false`、下一候选版本 `0.1.1`、无残留草稿。 |
| 三平台构建与真实宿主 smoke | `.github/workflows/follow-host.yml`（`0.1.1` 三平台打包与 `v3.55.2` 隔离 smoke） | — | **未验证**（待推送并运行远端流水线）。 |
| 线上真实 OAuth 实例验收 | 管理员通过切换版本升级至 `v0.1.1` 后实测定时巡检开启、切页保持与实际到期触发 | — | **未验证**（待 `v0.1.1` 发布后线上复测）。 |

## 后续行动与授权边界

1. **推送并触发 `v0.1.1` 发布（需用户明确授权）**：推送 `origin/main` 后，手动触发 **Follow CPR and lm-detector releases**（或等待下一个 30 分钟定时槽）完成三平台重建、`v3.55.2` 隔离 smoke、正式版 `v0.1.1`（`Latest`）发布与 `cpr-compatibility` 单调更新。
2. **线上升级方式**：在 Codex Proxy 宿主管理页点击更新/切换版本（`switch-version`）升级至 `v0.1.1`，无需删除实例，既有设置与历史自动保留。
3. **授权边界**：本交接快照不授予 `git push`、创建/推送标签、触发远端工作流、创建 GitHub Release 或生产实例切换权限；所有外部副作用均须独立明确授权。
