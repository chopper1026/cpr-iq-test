# 阶段交接与验证快照

适用提交基线：`bc6021d144445e63ab76264de371a24595dc8e21`（`main`）。证据日期：2026-11-25（Asia/Shanghai）。

本文记录当前开发预览阶段的多平台验证矩阵、分层证据与授权边界；软件项目不涉及硬件槽位字段。未执行层级均标为「未验证」，历史提交的局部通过不追认为全平台或当前构建通过。

## 当前状态与入口

- **阶段目标**：完成开发预览包的三平台 GitHub Actions 验证（`prerelease=true`），后续在隔离测试实例取得授权健康 OpenAI OAuth 账号校准证据后，再决定是否写入 `data/profiles.json` 并开启正式发版。
- **权威入口**：
  - 安装与日常使用：[README.md](../../README.md)
  - 校准、隔离 smoke、自动跟版与回滚：[docs/release.md](../release.md)
  - 系统边界与模块契约：[ARCHITECTURE.md](ARCHITECTURE.md)
  - 持久化、配额与备份边界：[PERSISTENCE.md](PERSISTENCE.md)
  - 功能实现与分层证据：[FEATURES.md](FEATURES.md)
  - 开发治理导航：[INDEX.md](INDEX.md)
- **当前主要阻断与风险**：
  1. `data/profiles.json` 中唯一档位 `gpt6-sol-low` 仍为 `validation: null`，正式判定、定时巡检和生产发布均保持关闭。
  2. `bc6021d` 修复了 run `45629428949` 中 `old` 跳过导致 `rebuild` 被误跳过的问题，但尚未完成推送后的远端三平台 `prerelease=true` 复跑。

## 构建与运行身份

- **基线提交**：`bc6021d144445e63ab76264de371a24595dc8e21`（`main`）；插件版本：`0.1.0`（`plugin.json` 与 `Cargo.toml` 一致）。
- **锁定外部契约**：
  - 宿主目标：`zyycn/codex-proxy-rs` `v3.54.5`（commit `45a4d580c7e1ac2b2c0308d72ae5018148f2516a`，`engines.codex-proxy-rs: ~3.54.5`）。
  - 参考检测库：`Ikaleio/lm-detector` commit `e0472890bea827d15ca883636a77d4c10bbe843b`（`unified_bank.json` 与 `shared_detector.bin` 锁于 `data/detector.lock.json`）。
  - 工具链：Rust `1.99.1`（`rust-toolchain.toml`）、Python `3.12+`。
- **三平台产物与验证矩阵**：

| 目标平台三元组 | CI runner | 最近已知构建身份与产物摘要 | Linux `maxGlibc` | 隔离宿主 smoke 状态 |
| --- | --- | --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | 2026-11-25 首次接入前本机开发包 `cpr-iq-test.account-fingerprint-0.1.0-dev.1-aarch64-apple-darwin.tar.gz`，SHA-256 `d6791b714f968122155770f920c94ef6487e5d9a5a8be0cb32ef852cc9d9adfe`（非 `bc6021d` CI 重编产物） | 不适用 | 本机 `v3.54.5` 通过（含 `0.0.0` → `0.1.0-dev.1` 状态切换与重启）；远端 `macos-15` runner **未验证** |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | 未构建（无已核对产物摘要） | 未验证 | 未验证 |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | 未构建（无已核对产物摘要） | 未验证 | 未验证 |

## 验证证据

| 层级 | 当次实际操作 / 证据位置 | 日期 / 对象 / 环境 | 结果与覆盖限制 |
| --- | --- | --- | --- |
| 源码与配置静态检查 | `cargo fmt --all -- --check`、`cargo clippy --offline --locked --all-targets -- -D warnings`、`node --check web/app.js`、`actionlint .github/workflows/*.yml`、`tidy.py audit-docs`、`tidy.py check` | 2026-11-25 / `ec0957f`（Rust/JS）与 `700e059`/`bc6021d`（YAML/发版脚本） / macOS ARM64 | 通过；静态与文档引用检查不代表运行期或跨平台验收。 |
| 单元与宿主合同测试 | `CPR_REQUIRE_BANK=1 cargo test --offline --locked --all-targets`（16 项）、`scripts/check_bound_host.sh`（2 条 Core 合同）、`python3 -m unittest discover -s tests -p 'test_*.py'`（18 项）、`scripts/detector_assets.py --verify-only`、`scripts/release_gate.py preflight` | 2026-11-25 / `ec0957f`（Rust/宿主合同）与 `700e059`（18 项 Python 发版回归与只读 preflight） / macOS ARM64，Rust `1.99.1`，Python `3.12` | 通过；无真实 OAuth 调用，参考语料异标签用例不代表准确率评估。 |
| 产品构建与打包 | `scripts/package.py`（本机 macOS ARM64）；三平台 CI `follow-host.yml` `rebuild` 矩阵 | 2026-11-25 / 首次接入工作区（本机 macOS ARM64） | 本机 macOS ARM64 `0.1.0-dev.1` 通过；三平台 CI 构建与 Linux `maxGlibc` 记录**未验证**。 |
| 隔离宿主 smoke 与状态切换 | `scripts/smoke_host.py`（`v3.54.5`，含 `--baseline` `0.0.0` → `0.1.0-dev.1` 与重启） | 2026-11-25 / 首次接入工作区 / macOS ARM64 | 通过（设置与合成历史保留，无真实模型调用）；Linux 两平台与跨宿主版本切换**未验证**。 |
| 远端 Release 摘要与回算核验 | `.github/workflows/follow-host.yml`（`prerelease=true` 或正式发布）、`scripts/release_gate.py publish`、`publish-receipt.json` | — | **未验证**（仅有 18 项离线桩单测覆盖，尚无远端实跑成功回执）。 |
| 授权 OAuth 校准正例 | 隔离测试实例人工校准并核验 `data/profiles.json` `validation` 证据 | — | **未验证**（`gpt6-sol-low` 仍为 `validation: null`）。 |
| 生产实例切换与回滚 | 管理员核对兼容表后于生产实例安装/切换 | — | **未验证**（未开放生产发布）。 |

## 后续行动与授权边界

1. **只读本地预检（无副作用）**：`python3 scripts/release_gate.py preflight`（按需加 `--online --repository <owner>/<repo>`）。
2. **远端预发布验证（需管理员触发 GitHub Actions）**：在默认分支 `bc6021d`（或含后续确认提交的 HEAD）手动触发 **Verify new CPR stable releases**（`host_tag=v3.54.5`、`prerelease=true`，可先选 `dry_run=true` 演练），检查三平台 `candidate-*`、`maxGlibc`、远端 5 份附件摘要核验与 `publish-receipt.json`。
3. **授权校准与正式发版前提**：仅在隔离实例用授权正常 OAuth 账号完成校准并向 `data/profiles.json` 提交真实证据后，再由维护者显式评估开启 `CPR_ENABLE_PUBLISH=true`。
4. **授权边界**：本交接快照不授予 `git push`、创建远端 Release、修改 `cpr-compatibility`、写入 OAuth 凭据或生产实例切换权限；所有外部副作用均须独立明确授权。
