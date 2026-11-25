# 持久化与备份边界

最近核验日期：2026-11-25（Asia/Shanghai）。

以下每项均对照源码读写路径、清单配额与测试脚本核验；未在真实生产环境实测的行为明确标为「源码判断，未实测」。插件不提供独立导出、备份或恢复命令，请勿假定存在“插件一键全量备份”。

## 数据覆盖

| 数据及所有者 | 存储介质、实际路径或 key | 配额与保留上限 | 插件导出/备份覆盖 | 恢复入口与兼容边界 | 源码与测试证据（日期） |
| --- | --- | --- | --- | --- | --- |
| 插件私有状态（定时设置 `settings`、下次时间槽 `nextDueAt`、`lastScheduleSkip`、`scheduleRevision`、有界队列 `queue`、历史摘要 `history`（含邮箱/名称快照与候选概率）、手动幂等键 `accepted`）<br>所有者：插件实例（托管于宿主） | 宿主 PostgreSQL 私有状态表（`plugin_state_generations` / `plugin_state_records`），经 `host.state.get` / `host.state.put` 读写；`namespace: iq-test`、`key: state`、`schemaVersion: 1`，单条 `{"document": "<base64(zlib(json))>"}` 文档。不存探针正文、原始回复、OAuth Token 或 Client Key 明文。 | 清单配额：`maximumRecords: 1`、`maximumBytes: 65536`（64 KiB）、`maximumValueBytes: 61440`（60 KiB）、`document.maxLength: 59392`（58 KiB）；编解码上限：`MAX_ENCODED = 58 KiB`、`MAX_JSON = 512 KiB`（`encode_bounded` 在接近上限时裁剪最旧历史避免写入失败）。业务上限：队列 ≤ 200 项、历史 ≤ 500 条且 ≤ 30 天、手动幂等键 ≤ 500 条（保留 24 小时或在队期间）。 | **不支持**（插件无导出、备份或恢复管理路由/CLI）。 | 依赖宿主管理员对宿主 PostgreSQL 做实例级备份与还原（本仓库范围之外）。无私有状态时以 `State::default()` 启动，历史无法从安装包重建。向后兼容 `v0.1.0-dev.1` 状态自动迁移。 | 源码：[plugin.json](../../plugin.json)、[src/host.rs](../../src/host.rs)、[src/state_codec.rs](../../src/state_codec.rs)、[src/domain.rs](../../src/domain.rs)；测试：Rust 500 条历史压缩与旧版迁移单测 + [smoke_host.py](../../scripts/smoke_host.py) macOS ARM64 `v3.54.5` 实测（2026-11-25）。 |
| 宿主凭据、账号、Client Key 明文、路由、计费与限流配置<br>所有者：宿主 `codex-proxy-rs` | 宿主数据库、Redis 与宿主配置文件（插件不直接接触存储路径）。 | 由宿主管理。 | **不支持（不属于插件范围）**。插件仅按 ID 查询启用账号/Key、内部只读读取模型目录、校验访问权并发起绑定账号的 `host.model.execute_stream`。 | 遵循宿主运维备份与权限治理，不纳入插件包、插件历史或本仓库备份。 | 源码：[src/host.rs](../../src/host.rs)、[src/service.rs](../../src/service.rs)（2026-11-25）。 |
| 随包静态资源、锁定配置与远端兼容表<br>所有者：本源码仓库与 GitHub 仓库 | 源码库内：`plugin.json`、`data/detector.lock.json`、`web/*`、`LICENSE*`、`THIRD_PARTY.md`；打包随包分发：`data/unified_bank.json`（15,275,823 B）、`data/shared_detector.bin`（5,514,204 B）；远端兼容表：`cpr-compatibility` 分支 `compatibility/vX.Y.Z.json`。 | 参考数据按 `data/detector.lock.json` 精确锁定字节数与 SHA-256；兼容表发布后不可覆盖，每次宿主 tag 写入一条。 | 锁文件与源码受 Git 版本控制；参考数据可按锁重下并校验；历史制品与兼容表随 GitHub Release / `cpr-compatibility` 分支保留。 | 按锁定 Git 提交、宿主源码 commit 与 `data/detector.lock.json` 可重编安装包；回滚依赖保留已验证的宿主＋插件制品对与兼容表，不应从生产状态反向提取。 | 源码：[data/detector.lock.json](../../data/detector.lock.json)、[scripts/package.py](../../scripts/package.py)、[scripts/release_gate.py](../../scripts/release_gate.py)（2026-11-25）。 |
| 本地工作区缓存、开发包与治理过程目录<br>所有者：本机开发者 / CI runner | `.work/`、`target/`、`dist/`、本地缓存的 `data/unified_bank.json` 与 `data/shared_detector.bin`、`docs/local/tidy/`、`.git/tidy/`、`.playwright-cli/`、`output/playwright/`。 | 受本机磁盘与 CI cache 策略约束；均处于 [.gitignore](../../.gitignore) 忽略边界或 Git 元数据目录，不入库。 | 不纳入仓库共享备份。 | 构建缓存与下载数据可重建；克隆新副本后可由 `tidy.py sync` 重建本机索引入口，私人过程日志与 `.git/tidy/` 历史回执/本机备份不会自动恢复。 | 源码：[.gitignore](../../.gitignore)、[.tidy.json](../../.tidy.json)（2026-11-25）。 |

## 操作影响

| 数据 | 常规同实例版本切换（`switch-version`） | 宿主重启 / 进程中断 | 卸载 / 删除插件实例后重装 | 修改或停用巡检计划（`PUT schedule`） | Schema / 存储布局迁移 | 依据与未验证范围 |
| --- | --- | --- | --- | --- | --- | --- |
| 插件私有状态（`iq-test / state`） | **同宿主版本、同 `schemaVersion: 1` 下保留并兼容迁移**设置与历史摘要（2026-11-25 macOS ARM64 `v3.54.5` 上 `0.0.0` → `0.1.0-dev.2` 含旧版 `v0.1.0-dev.1` `profileId`/`calibration` 记录实测通过）。跨宿主版本切换**未实测**。 | **保留**设置、历史与未认领队列；已认领未结算任务在下次 `reconcile` 调 `state.recover()` 记为 `interrupted / restarted`，不重发请求（Rust 单测 + macOS smoke 重启实测）。 | **丢失**该实例私有状态；重装新实例回退至 `State::default()`，历史无法从安装包重建（源码判断，未实测生产删除）。 | 仅更新 `settings`；停用或变更间隔/目标时递增 `scheduleRevision` 并将未认领定时任务标为 `skipped / scheduleChanged`，**不清空** `history`（Rust 单测覆盖）。无清空历史接口；支持通过 `POST /runs/cancel` 取消尚未执行的排队任务。 | `schemaVersion: 1` 内对 `v0.1.0-dev.1` 字段（`probeBudget`、`reservations`、`profileId`、`mode`、`verdict: calibration`）提供无损向后兼容反序列化与迁移；非 `1` 版本仍返回 `Fault`（跨 schema v2 迁移未实测）。 | 实测覆盖仅限 macOS ARM64 `v3.54.5` 同版本切换与重启（合成历史）；跨宿主版本升级/回滚、生产删除实例与 schema v2 迁移均未实测。 |
| 宿主凭据、账号与路由 | 插件不改动（源码判断）。 | 插件不改动（源码判断）。 | 插件不改动；由宿主管理。 | 插件不改动。 | 由宿主独立管理。 | 插件仅声明 `management v2` / `maintenance v1`，无账号/路由写接口（[src/host.rs](../../src/host.rs)）。 |
| 随包静态资源（参考库 / 前端） | 随新版本 `.tar.gz` 整体替换；启动时重校两份数据 SHA-256，校验失败则管理页可读历史但不发探针。 | 随实例包只读加载，无运行时写入。 | 删除实例时一并移除，可从原校验安装包重装。 | 无影响。 | 参考库修订变更会令进行中复测 `stamp` 失效（防止跨库混评）。 | 源码：[src/main.rs](../../src/main.rs)、[src/fingerprint.rs](../../src/fingerprint.rs)、[src/service.rs](../../src/service.rs)；Rust 单测覆盖（2026-11-25）。 |

## 安装与回滚入口

- 日常安装与使用边界见 [README.md](../../README.md)。
- 隔离验证、生产切换与版本对回滚步骤见 [验证、跟版与回滚](../release.md)。
- 操作前须知：插件无内置备份/导出功能；回滚时按已验证的**宿主＋插件版本对**执行实例切换，**不要通过删除实例来降级或重装**，否则会移除私有状态且无法从安装包恢复。
