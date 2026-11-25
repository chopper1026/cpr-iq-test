# 阶段交接与验证快照

证据日期：2026-11-25（Asia/Shanghai）。

本文记录当前多平台验证矩阵、分层证据与授权边界；软件项目不涉及硬件槽位字段。未执行层级均标为「未验证」，历史提交的局部通过不追认为全平台或当前构建通过。

## 当前状态与入口

- **阶段目标**：修复线上 `v0.1.0-dev.1` 流解析 `Reason::Incomplete` 缺陷，移除 24 小时探针预算与校准模式限制，上线宿主全部模型与推理档位自动拉取、账号真实邮箱展示以及独立「任务记录」筛选分页 Tab 页，待推送并经 GitHub Actions 三平台构建验证后供线上升级。
- **权威入口**：
  - 安装与日常使用：[README.md](../../README.md)
  - 隔离 smoke、自动跟版与回滚：[docs/release.md](../release.md)
  - 系统边界与模块契约：[ARCHITECTURE.md](ARCHITECTURE.md)
  - 持久化、配额与备份边界：[PERSISTENCE.md](PERSISTENCE.md)
  - 功能实现与分层证据：[FEATURES.md](FEATURES.md)
  - 开发治理导航：[INDEX.md](INDEX.md)
- **当前主要待办与风险**：
  1. 尚未执行线上真实 OAuth 账号的端到端模型目录拉取与指纹实跑验证（已通过宿主源码核对与单测/隔离 smoke 覆盖协议细节）。
  2. 尚未推送至 GitHub 触发 Linux x86_64 / Linux ARM64 / macOS ARM64 三平台 CI 重建与 smoke。

## 构建与运行身份

- **插件版本**：`0.1.0`（`plugin.json` 与 `Cargo.toml` 一致；本地验收包版本 `0.1.0-dev.2`）。
- **锁定外部契约**：
  - 宿主目标：`zyycn/codex-proxy-rs` `v3.54.5`（commit `45a4d580c7e1ac2b2c0308d72ae5018148f2516a`，`engines.codex-proxy-rs: ~3.54.5`）。
  - 参考检测库：`Ikaleio/lm-detector` commit `e0472890bea827d15ca883636a77d4c10bbe843b`（`unified_bank.json` 与 `shared_detector.bin` 锁于 `data/detector.lock.json`）。
  - 工具链：Rust `1.99.1`（`rust-toolchain.toml`）、Python `3.12+`。
- **三平台产物与验证矩阵**：

| 目标平台三元组 | CI runner | 最近已知构建身份 | Linux `maxGlibc` | 隔离宿主 smoke 状态 |
| --- | --- | --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | 2026-11-25 本机开发包 `0.1.0-dev.2-aarch64-apple-darwin.tar.gz`（位于 `.work/smoke-dist/`，未入库） | 不适用 | 本机 `v3.54.5` 通过（含 `0.0.0 → 0.1.0-dev.2` 状态切换、旧版迁移与重启）；远端 `macos-15` runner **未验证** |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | 未构建 | 未验证 | 未验证 |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | 未构建 | 未验证 | 未验证 |

## 验证证据

| 层级 | 当次实际操作 / 证据位置 | 日期 / 对象 / 环境 | 结果与覆盖限制 |
| --- | --- | --- | --- |
| 源码与配置静态检查 | `cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings`、`node --check web/app.js`、`actionlint`、`tidy.py audit-docs`、`tidy.py check` | 2026-11-25 / 本次重构工作区 / macOS ARM64 | 通过；静态与文档检查不代表线上运行验收。 |
| 单元与宿主合同测试 | `CPR_REQUIRE_BANK=1 cargo test --locked --all-targets`（19 项）、`python3 -m unittest discover -s tests -p 'test_*.py'`（17 项）、`scripts/detector_assets.py --verify-only`、`follow_host.contract`（16 项回调） | 2026-11-25 / 本次重构工作区 / macOS ARM64，Rust `1.99.1`，Python `3.12` | 通过；无真实线上 OAuth 调用。`scripts/check_bound_host.sh`（2 条 Core 合同）基线已于 `ec0957f` 验证。 |
| 产品构建与隔离宿主 smoke | `scripts/package.py` + `scripts/smoke_host.py --docker-services`（`v3.54.5`，`0.0.0 → 0.1.0-dev.2` 含 `v0.1.0-dev.1` 旧记录迁移、邮箱搜索、筛选分页与重启） | 2026-11-25 / 本次重构工作区 / macOS ARM64 | 通过；Linux 两平台与线上真实 OAuth 调用**未验证**。 |
| 远端 Release 摘要与回算核验 | `.github/workflows/follow-host.yml`、`scripts/release_gate.py publish`、`publish-receipt.json` | — | **未验证**（仅有 17 项离线桩单测覆盖，待推送后由管理员触发工作流实跑）。 |
| 线上真实 OAuth 实例验收 | 管理员上传新包并切换实例后实测目录拉取、手动与定时指纹检测 | — | **未验证**（待本次发包后线上复测）。 |

## 后续行动与授权边界

1. **只读本地预检**：`python3 scripts/release_gate.py preflight`（按需加 `--online --repository <owner>/<repo>`）。
2. **远端三平台发包（需管理员授权推送与触发 GitHub Actions）**：推送本次提交后在默认分支触发 **Verify new CPR stable releases**（`host_tag=v3.54.5`，按需选 `prerelease=true` 或开启 `CPR_ENABLE_PUBLISH=true` 正式发布），核对三平台产物、`maxGlibc` 与远端摘要回执。
3. **线上升级注意**：在宿主管理页使用 **切换版本（`switch-version`）** 升级现有实例，无需删除实例；旧版 `v0.1.0-dev.1` 历史记录将自动兼容迁移。
4. **授权边界**：本交接快照不授予 `git push`、创建远端 Release、修改 `cpr-compatibility`、写入 OAuth 凭据或生产实例切换权限；所有外部副作用均须独立明确授权。
