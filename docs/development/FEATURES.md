# 功能全景

本表区分代码实现、局部实测与线上验证；各行独立标注最近验证日期、提交/工作区对象、环境与层级，表头不暗示全量复验。

- 首轮实现与本地集成基线见 [2026-11-25 接入记录](changes/20261125-tidy-onboarding.md)（`ec0957f`）。
- 跟版发版加固与 `rebuild` 跳过修复见 [2026-11-25 发版加固记录](changes/20261125-harden-release-workflow.md)（`700e059`、`bc6021d`）。
- 流解析截断修复、模型/档位自动获取、真实邮箱展示、去预算/去校准及独立任务记录 Tab 重构见 [2026-11-25 流解析修复与交互重构记录](changes/20261125-fix-stream-and-ux-redesign.md)（`0cb0c29`）。
- 发布脚本顺序上传与瞬时 5xx 重试见 [2026-11-26 发布重试加固记录](changes/20261126-release-upload-retry.md)（`7f68bea`，已实跑发布 `v0.1.0-dev.2`）。
- 上游账号身份稳定校验修复与诊断日志导出见 [2026-11-26 账号身份校验修复与诊断日志导出](changes/20261126-fix-account-identity-and-log-export.md)（`17974ca`，已发布 `v0.1.0-dev.3`）。
- Codex Proxy 样式复用、账号实时状态、每日固定时间巡检与正式版双上游自动更新见 [2026-11-26 界面样式对齐、实时状态、每日巡检与正式版双上游自动跟版](changes/20261126-ui-style-live-status-daily-schedule-stable-auto-update.md)（`8b0495e`，已随 `a373b57` 发布正式版 `v0.1.0`）。
- 定时巡检开关即时持久化、首屏未生效引导与轮询竞态修复见 [2026-11-26 定时巡检开关持久化与首屏引导修复](changes/20261126-fix-schedule-toggle-persistence.md)。

| ID | 能力 | 实现状态 | 权威入口 | 已验证证据（日期 / 对象 / 环境 / 层级） | 仍待验证 |
| --- | --- | --- | --- | --- | --- |
| CPR-01 | 复用 Codex Proxy `--z-*` 主题与自定义下拉框/开关/分段器/表格组件的三 Tab 管理页：账号实时状态（最近一次检测、凭据状态、自动停用审计、近期趋势、一键检测/历史/日志）+ 检测与巡检（含「异常自动停用」即时开关、「定时巡检」即时开关/首屏未生效引导与防轮询回写）+ 任务记录（多维筛选、单账号过滤、页码分页、取消排队、诊断日志导出） | 已实现 | [管理页](../../web/index.html)、[样式](../../web/style.css)、[前端逻辑](../../web/app.js)、[服务路由](../../src/service.rs)、[宿主目录发现](../../src/host.rs) | 2026-11-26 / 本次工作区 / macOS ARM64 / 静态（`node --check`、`cargo fmt/clippy`）+ Rust 单测（26 项）+ 无头 Chrome 桥接端到端 5 场景测试（首次开开关首屏引导、一键全选开启、切页重载持久化、开关即时启停、未保存横幅、`403` 回弹与慢轮询防覆盖，0 Console 错误）；基线 `v0.1.0` 已在远端 CI 三平台及本机 `v3.54.5` iframe 验证。 | 本次修改尚未重新打包做真实宿主 iframe smoke 与线上升级 `v0.1.1` 后实测。 |
| CPR-02 | 固定指定账号，不可用时不回落到其他账号；基于上游账号身份而非易变 `credential_revision` 判断采样期账号变更 | 已实现 | [宿主适配](../../src/host.rs)、[宿主合同](../../scripts/check_bound_host.sh) | 2026-11-26 / 本次工作区 / macOS ARM64，Rust `1.99.1`，宿主源码 `45a4d580...` / 单测 + 宿主合同 + 线上 CPR 诊断包核对：Rust 单测覆盖同身份 `credential_revision` 递增通过、替换 `upstream_user_id` 拒绝、发送给 A 不访问 B 及停用拒绝。 | 线上真实 OAuth 账号实跑确认 `gpt-6.2-sol / high` 完整产出 Top-3 候选与结论。 |
| CPR-03 | 校验成对参考数据，离线生成三个独立探针，兼容 Responses 稀疏 `content_index`（如 `1024`）及默认推理档位，采集逐探针流诊断元数据，仅对完整可见文本排序 | 已实现 | [宿主适配](../../src/host.rs)、[指纹层](../../src/fingerprint.rs)、[数据锁](../../data/detector.lock.json) | 2026-11-26 / 本次工作区（并修复缓存 `CARGO_TARGET_DIR` 下测试数据路径解析）/ macOS ARM64 / 数据校验 + Rust 单测：覆盖稀疏索引、默认档位、逐探针 `SampleTrace` 与评分器 `ScoreTrace`。 | 线上多模型、多推理档位实跑结果观察。 |
| CPR-04 | 全模型指纹判定、参考排名与可选异常自动停用：库内模型支持首轮匹配与二轮复测疑似判定，库外模型输出 Top-3 参考排名与概率；显式开启 `autoDisableOnSuspect` 后，仅对两轮确认的 `Suspect` 账号自动在宿主停用并合并写入含检测结果与时间的 `[指纹异常]` 备注（保留原备注及分组/权重/并发/代理/凭据设置） | 已实现 | [指纹层](../../src/fingerprint.rs)、[判定与备注合并](../../src/domain.rs)、[宿主账号更新](../../src/host.rs)、[维护结算](../../src/service.rs) | 2026-11-26 / 本次工作区 / macOS ARM64 / Rust 单测（覆盖库内/库外判定、`[指纹异常]` 时区格式化与 500 字符合并去重、`POST /api/admin/accounts/update` 载荷无损保留、独立开关不重置活动定时计划）+ 隔离宿主 smoke + Chromium 开关即时保存验证。 | 线上真实 OAuth 账号触发两轮 `Suspect` 后在宿主账号页端到端核对停用状态与备注展示。 |
| CPR-05 | 手动检测与定时巡检共用队列（上限 200）；定时巡检支持按间隔（1/6/12/24/48/168 小时）与每日固定时间（最多 12 个 `HH:MM` + `utcOffsetMinutes` 时区），错过不补跑、停用跳过旧定时、去重与待执行取消，仅在启用或变更目标时复核账号访问权限并记录运行期日志 | 已实现 | [队列与调度](../../src/domain.rs)、[维护回调](../../src/service.rs) | 2026-11-26 / 本次工作区 / macOS ARM64 / Rust 单测（26 项，含每日固定时间/时区模拟时钟与 `schedule_access_check_runs_on_enable_or_target_change_but_not_guard_only_toggle`）+ 无头 Chrome 开关即时启停与重载持久化测试。 | 本次修改在真实宿主隔离 smoke 及线上真实账号开启每日固定时间巡检的跨日长期运行验证。 |
| CPR-06 | 设置及历史摘要（≤ 500 条、30 天）跨插件切换、旧版 `v0.1.0-dev.*` 状态迁移与进程重启保留 | 已实现 | [编解码](../../src/state_codec.rs)、[持久化边界](PERSISTENCE.md)、[隔离验收](../../scripts/smoke_host.py) | 2026-11-26 / 本次工作区 / macOS ARM64，宿主 `v3.54.5` / 单测 + 隔离 smoke：Rust 500 条历史压缩与旧版状态反序列化迁移单测通过；同一 `v3.54.5` 实例注入现代记录与 `v0.1.0-dev.1` 旧记录，完成 `0.0.0 → 0.1.0` 切换及宿主重启，每日固定时间设置、邮箱筛选与迁移记录均完整保留。 | 跨宿主版本升级/回滚、删除实例行为及未来跨 schema 迁移。 |
| CPR-07 | 正式版双上游自动跟版：自动跟踪 `codex-proxy-rs` 正式版与 `lm-detector`（`data/` + `sdk/rust/`）变更并同步锁定依赖，按 `packageSourceHash` 决定复用旧包或三平台重建，支持单调兼容表更新、预检、远端摘要核验与断点续跑；`main` 只预警 | 已实现，远端复跑待验收 | [双上游同步](../../scripts/sync_upstreams.py)、[正式版工作流](../../.github/workflows/follow-host.yml)、[main 工作流](../../.github/workflows/host-main.yml)、[发版闸门测试](../../tests/test_release.py) | 2026-11-26 / 本次工作区 / macOS ARM64，Python `3.12` / 静态 + 22 项离线单测 + 线上只读 `preflight`/`sync_upstreams` 预检 + `actionlint`：覆盖 `lm-detector` 仅改网页忽略与数据/SDK 变更识别、包源码摘要触发重建、单调兼容表更新与降级拦截、草稿重试/摘要回算与续跑校验；实测只读识别 CPR `v3.55.2` 与 `lm-detector` 最新提交。 | 推送并启用 `CPR_ENABLE_PUBLISH=true` 后在远端 GitHub Actions 完整实跑三平台构建与首个正式版 `v0.1.0` 发布。 |
| CPR-08 | 生成正式版安装包和摘要，三平台同源验证后发布 | 部分验证（仅本地 macOS ARM64） | [打包脚本](../../scripts/package.py)、[发布条件](../release.md)、[交接快照](HANDOFF.md) | 2026-11-26 / 本次工作区 / macOS ARM64，宿主 `v3.54.5` / 本地打包 + 隔离 smoke + Chromium iframe 验证：`0.1.0-aarch64-apple-darwin.tar.gz` 打包校验、真实宿主切换 smoke 与浏览器验证通过。 | Linux `x86_64-unknown-linux-gnu`、Linux `aarch64-unknown-linux-gnu` 构建与 smoke 及线上安装验收。 |

## 核验范围

盘点入口包含三 Tab 管理页及路由、自动模型目录发现、邮箱优先账号展示与实时状态、维护任务、可选指纹异常自动停用与备注写入、私有状态持久化与向后兼容迁移、安装包与离线数据、正式版双上游跟版工作流与回滚。参考排名不是身份认证；默认不自动停用账号，仅在显式开启「异常自动停用」后对两轮确认的 `Suspect` 账号停用并更新备注，始终不改变路由或升级生产。
