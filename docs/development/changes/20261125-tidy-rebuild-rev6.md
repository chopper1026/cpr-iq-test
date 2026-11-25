# 20261125-tidy-rebuild-rev6

- 创建时间：2026-11-25T22:58:00+08:00
- 状态：implemented
- 关联任务/功能：执行 `$tidy-rebuild`，将仓库治理从 revision 5 增量升级至 revision 6，并完成九类文档与检查机制对齐（基线提交 `bc6021d144445e63ab76264de371a24595dc8e21`）。

## 背景与目标

按新版 `tidy`（revision 6）对已接入仓库执行增量升级与全量九类文档语义整理：保留全部已确认项目策略（`commit_policy: new`、`history_visibility: tracked`、`legacy_docs: preserve`、`catalog.asset_mode: grouped`、`gitnexus.enabled: false`），不迁移历史文档、不改动业务源码、不安装 hook，也不在无授权情况下暂存、提交、推送或发布。

## 升级来源与本地备份

- 治理修订：`5` → `6`
- 技能指纹：`528e481f57f68e1d4148a75f5a3d4e94b3ec80ee522da50d3b19ac3e909cffd2` → `990fce613344df0107b8ab424c267a3fb4a45976448c72bea44c889bcaf6be2e`
- `AGENTS.md` 托管块哈希：`110297d103eeb11bcce7b0862335d2133adcdb2671d60d6bf7d57e384ad1609a` → `08a8d8bf9b55bae1833df1c82c7a62cddfc4e0564286ca359991b7246f27da27`
- 机械升级计划与本机备份：`plan_id=df91ac30112e8207d346a52958c00fefbb1be13b23914c3bbff0b22b89943e10`，备份目录 `.git/tidy/rebuild-backups/df91ac30112e-pm6b5s87`（覆盖 `.tidy.json`、`AGENTS.md`）。
- 语义整理本机逐文件备份：`.git/tidy/rebuild-backups/semantic-20261125T225710-kco8n7ec`（校验保存 8 份待改文件原文、权限与 SHA-256，并登记 3 份新建文件原先不存在）。

## 九类核对完成表

| 事项 | 现状/证据 | 适用范围 | 修改文件 | 验证结果 | 状态/剩余项 |
| --- | --- | --- | --- | --- | --- |
| 1. 构建、环境与运行契约 | 逐项核对 `Cargo.toml`、`rust-toolchain.toml`、`plugin.json`、`data/detector.lock.json`、`data/profiles.json`、`scripts/*`、`.github/workflows/*.yml` 与 `README.md`/`docs/release.md`：Rust `1.99.1`、Python `3.12+`、SDK/数据锁、`~3.54.5`、三平台三元组与核心命令均一致；补充 `docs/release.md` 中档位 ID `gpt6-sol-low`、宿主解压目录清单及 `resume_run_id` 续跑预发布须同步勾选 `prerelease=true` 的契约细节。 | 适用（纯软件原生插件，无硬件/刷机） | [docs/release.md](../../release.md) | 源码与配置交叉核对通过；`scripts/detector_assets.py --profiles` 与 `scripts/release_gate.py preflight` 只读校验通过。 | 已修复（无已知契约矛盾） |
| 2. 对外入口、多语言/手册与许可链 | 仓库仅含中文 `README.md`、`docs/README.md`、`docs/release.md` 及英文 `THIRD_PARTY.md`（随包分发）与 `LICENSE`/`LICENSE-lm-detector`，无多语言 README 漂移或文档生成器；原 `README.md` 与 `docs/release.md` 缺少持久化与真实备份覆盖入口链接。 | 适用（中文入口 + 英文第三方许可链；无多语言站点生成器） | [README.md](../../../README.md)、[docs/README.md](../../README.md)、[docs/release.md](../../release.md) | `tidy.py audit-docs` 与 `tidy.py check` 确认所有本地链接、锚点与许可引用有效。 | 已修复（`THIRD_PARTY.md` 核对一致未改动） |
| 3. GitNexus 项目集成 | `.tidy.json` 已由用户确认 `gitnexus.enabled: false`（无 `required: true`）；无 `CLAUDE.md`/`CONTRIBUTING.md`；`core.hooksPath` 未配置且 `.git/hooks` 不存在；旧 rev 5 `AGENTS.md` 仍保留 2 条 `gitnexus-work` 活动计划说明，随 rev 6 机械升级收拢为停用口径；`.gitignore` 的 `/.gitnexus/` 与历史记录如实保留。 | 适用（项目已确认停用 GitNexus） | [AGENTS.md](../../../AGENTS.md)、[.tidy.json](../../../.tidy.json) | 全仓搜索 `gitnexus` 确认现行指令与入口无冲突规则，无 Git hook 调用；`tidy.py status` 报告 `disabled`。 | 已修复 |
| 4. 交接与验收模板 | 此前缺少独立的多平台构建身份、分层验收与授权边界交接快照；项目存在三平台矩阵与多阶段发布闸门，需显式区分已执行的本地 macOS ARM64 证据与未执行的 Linux/远端 CI/真实 OAuth 验收。 | 适用（多平台软件交付，无硬件分区/槽位） | [HANDOFF.md](../HANDOFF.md) | 对照 `bc6021d` 与历史变更记录核对摘要与未验证边界；`audit-docs` / `check` 链接与引用通过。 | 已修复；远端三平台 CI、Linux `maxGlibc`、真实 OAuth 校准与生产切换本身仍待验证 |
| 5. 功能与架构证据 | 原 `FEATURES.md` 表头仅引用首次接入基线且未在行内分列实现状态、验证日期/对象/环境/层级与仍待验证项，`CPR-07` 保留过期表述「待本次执行验证」且未体现 `bc6021d` 修复。 | 适用 | [FEATURES.md](../FEATURES.md) | 保留 2026-11-25 首次接入基线并补充 `700e059`/`bc6021d` 证据分列；`data/profiles.json` 注册合约 `gpt6-sol-low` 通过 `audit-docs`。 | 已修复；产品端实测缺口按实标注为仍待验证 |
| 6. 持久化、状态与备份边界 | 核对 `plugin.json`、`src/host.rs`、`src/state_codec.rs`、`src/domain.rs`、`src/service.rs`、`src/fingerprint.rs`、`scripts/smoke_host.py` 与 `.gitignore`：补齐精确配额上限（64 KiB / 60 KiB / 58 KiB / 256 KiB JSON）、插件无导出/备份/恢复能力声明及五类操作影响矩阵，并在 `README.md`/`docs/release.md` 互链。 | 适用 | [PERSISTENCE.md](../PERSISTENCE.md)、[ARCHITECTURE.md](../ARCHITECTURE.md)、[README.md](../../../README.md)、[docs/release.md](../../release.md) | 源码核对一致；`plugin.json` `/state` `namespace=iq-test` 注册合约与引用通过 `audit-docs`。 | 已修复；跨宿主版本升级/回滚、删除实例与 schema 迁移按源码判断标为未实测 |
| 7. 权威去重 | 将持久化与备份完整矩阵收敛至 `PERSISTENCE.md`，精简 `ARCHITECTURE.md` 对应章节并回链；明确产品入口、发版回滚、许可链、架构契约、持久化边界、功能证据、阶段交接与提交规范各自单一正文权威，不搬迁既有文件。 | 适用 | [ARCHITECTURE.md](../ARCHITECTURE.md)、[PERSISTENCE.md](../PERSISTENCE.md)、[INDEX.md](../INDEX.md) | 人工复核各入口职责无正文重复，显式安全与许可约束均完整保留。 | 已修复 |
| 8. 导航语义 | 更新 `INDEX.md` 标注各权威入口职责范围，更新 `docs/README.md` 路由，并在 `.tidy.json` `catalog.categories` 纳入 `PERSISTENCE.md`（现行开发规范与契约）与 `HANDOFF.md`（交接与验收快照）。 | 适用 | [INDEX.md](../INDEX.md)、[docs/README.md](../../README.md)、[.tidy.json](../../../.tidy.json)、[CATALOG.md](../CATALOG.md) | `tidy.py sync` 重建 `CATALOG.md` 与本机 `docs/local/tidy/INDEX.md`，`check` 验证分类表同步一致。 | 已修复 |
| 9. 文档检查 | 原 `document_checks` 缺少 `toml`、`tests/known_other.json`、`scripts/release_gate.py` 等新增引用及任何 `registries` 合约。补齐 `toml`（`Cargo.toml`、`rust-toolchain.toml`）、扩充 `json` 与 `references`（共 31 项），新增 7 项 JSON/TOML `registries` 合约。 | 适用 | [.tidy.json](../../../.tidy.json) | `tidy.py audit-docs`、`tidy.py check` 与二次 `tidy.py rebuild` 预览均通过（退出码 0，`rebuild` 返回 `current`）。 | 已修复（工具边界：静态链接/锚点与显式注册表检查，不替代代码测试与语义复核） |

## 架构与功能影响

无运行时代码、构建脚本、锁文件或 GitHub Actions 行为变更；仅升级治理元数据、Agent 托管块、现行文档结构与静态文档检查配置。

## 验证与未验证项

- 治理与暂存快照检查：`tidy.py sync`、`tidy.py audit-docs --staged`、`tidy.py check --staged`（含新增暂存代码调试残留扫描）、`tidy.py rebuild`（幂等复检 `current`）与 `tidy.py status`（确认 GitNexus `disabled`）均通过。
- 只读配置与发版预检：`python3 scripts/detector_assets.py --profiles`、`python3 scripts/release_gate.py preflight` 确认配置与锁文件可读无回归。
- 未执行项：本次未修改 Rust/Python/JS/YAML 运行代码，未重跑 `cargo test` / `smoke_host.py`；未执行 `git push`、安装 hook 或远端发布。

## 开发残留清理

本次仅变更治理配置与 Markdown 文档，未新增运行时代码、临时日志、断点或调试桩；仅暂存本任务 12 个文件并通过 `check --staged` 扫描。
