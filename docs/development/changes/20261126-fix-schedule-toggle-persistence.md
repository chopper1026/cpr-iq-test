# 20261126-fix-schedule-toggle-persistence

- 创建时间：2026-11-26T10:01:38+08:00
- 状态：implemented
- 关联任务/功能：CPR-01、CPR-05

## 背景与目标

修复线上用户反馈的「定时巡检无法开启，开启后切换宿主页面再切回又自动关闭」问题：

1. 「启用定时巡检」开关此前仅修改本地复选框，不会自动持久化，与上方即时保存的「指纹异常自动停用」开关交互不一致；切换 Codex Proxy 宿主页面后 iframe 重载，读取到的仍是未保存的 `enabled: false`。
2. 首次开启定时巡检时，「指定账号」默认未勾选且位于折叠视口下方，「保存巡检设置」按钮被静默禁用且无首屏提示。
3. 5 秒后台轮询 `refresh(false)` 与 `PUT /schedule` 并发时，滞后的 `GET /schedule` 响应可能覆盖刚保存的计划。
4. 后端 `PUT /schedule` 在仅切换 `autoDisableOnSuspect`（未新启用或变更 `target`）时仍强行复核 `host.access`，且缺少开关保存与访问校验拒绝的运行期日志。

## 分类变化

### 问题修复
- **定时巡检开关即时持久化与首屏引导**（`web/index.html`、`web/style.css`、`web/app.js`）：
  - 在「定时巡检」卡片标题新增状态标签（`已关闭` / `未生效 · 待保存` / `已开启` / `有未保存修改`），并在开关正下方新增首屏状态与快捷操作横幅 `#schedule-state-banner`。
  - 已具备有效巡检目标（Client Key、模型、推理档位与至少 1 个账号）时，切换「启用定时巡检」开/关立即调用 `PUT /schedule` 持久化并同步顶部 KPI 卡片；后端拒绝时自动回弹开关并展示错误横幅。
  - 首次打开开关但缺少指定账号或其他必填项时，立即在开关下方提示未生效原因，并提供「去选账号 ↓」（平滑滚动高亮）及「全选 N 个账号并开启」（≤ `maxBatch` 时）一键保存启用；底部保存按钮改为显式校验与定位，不再静默禁用。
  - 为 `#submit-run` 与 `#save` 增加 `formnovalidate`，避免宿主桥合成提交时被浏览器原生校验拦截。
  - 为 `schedule` 同步引入单调 `scheduleEpoch` 守卫，防止滞后轮询覆盖刚提交的设置；仅在表单无未保存改动时同步其他标签页的远端变更。
- **后端 `PUT /schedule` 访问校验边界与审计日志**（`src/service.rs`）：
  - 抽离 `schedule_requires_access_check`，仅在启用定时巡检或变更 `target` 时检查 `detectorReady` 与 `host.access_with_detail`，避免已开启巡检后单独切换「异常自动停用」被无关账号权限变化阻断；为访问校验失败与定时设置变更写入运行期 `LogRing` 日志。

### 测试
- 在 `src/service.rs` 新增单元测试 `schedule_access_check_runs_on_enable_or_target_change_but_not_guard_only_toggle`，覆盖开启、变更目标、仅切换 `autoDisableOnSuspect` 与停用等状态转移。

### 文档
- 新增本次变更记录，更新 `docs/development/FEATURES.md`、`docs/development/HANDOFF.md` 与 `docs/development/CATALOG.md`。

## 决策与资料

- 入口源码：`web/index.html`、`web/style.css`、`web/app.js`、`src/service.rs`。
- 关键取舍：
  - **不在打开页面时自动预选全部账号**：避免未经确认就将大批量账号纳入可计费的定时调用；改为在用户主动打开开关且仅缺账号时，提供首屏显式「全选 N 个账号并开启」快捷按钮（仅当启用账号数 ≤ `maxBatch` 时显示）与「去选账号 ↓」定位按钮。
  - **保留已保存目标**：关闭「启用定时巡检」时立即保存 `enabled: false` 并保留原 `target`/时间点，再次点击开关即可一键恢复。

## 架构与功能影响

- `docs/development/ARCHITECTURE.md`：**无变化**（管理页路由集合、`PUT /schedule` 契约、私有状态 `schemaVersion: 1` 及维护调度模型均未变更）。
- `docs/development/PERSISTENCE.md`：**无变化**（私有状态结构、配额与迁移策略不变）。
- `docs/development/FEATURES.md`：更新 `CPR-01` 与 `CPR-05` 能力描述及本机测试/浏览器验证证据。
- `docs/development/HANDOFF.md`：同步已发布基线 `v0.1.0`（CPR `v3.55.2`）与下一候选版本 `v0.1.1` 待推送发布状态。

## 验证与未验证项

### 已执行验证（2026-11-26，macOS ARM64，Rust `1.99.1`，Python `3.12`）
- 静态检查：`node --check web/app.js`、`cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings` 全部通过。
- 单元测试：`CPR_REQUIRE_BANK=1 cargo test --locked --all-targets`（26/26 通过）、`python3 -m unittest discover -s tests -p 'test_*.py'`（22/22 通过）。
- 无头 Chrome 前端交互验证（模拟 `window.codexProxyPlugin` 桥接，0 Console / Page 错误）：
  1. 0 账号首次打开开关 → 首屏显示「未生效 · 待保存」及一键「全选 2 个账号并开启」 → 切每日 `06:00` 并开启 → 整页刷新模拟宿主切页后保持开启。
  2. 开关关闭与再次开启均即时持久化并在切页刷新后保持，原时间点与账号选择不丢失。
  3. 已开启状态下修改时间触发「有未保存修改」横幅并一键保存；清空账号后点击底部保存按钮弹出明确错误而非静默失效。
  4. 后端返回 `403 accessDenied` 时开关自动回弹并展示错误横幅。
  5. 慢速刷新与开启操作并发时，滞后响应不会覆盖新保存的 `enabled: true`。
- 联机只读预检：`scripts/release_gate.py preflight --online --repository chopper1026/cpr-iq-test` 确认上游 CPR `v3.55.2`、`lm-detector` 无新变更，识别本次源码变化触发重建并给出下一候选正式版 `0.1.1`。

### 未验证项
- 尚未对本次修改执行三平台正式构建、`smoke_host.py` 真实宿主包切换 smoke，以及线上 Codex Proxy 实例升级 `v0.1.1` 后的实测（待推送并运行远端 `follow-host.yml` 流水线验证）。

## 开发残留清理

- 人工核对全部变更文件（`src/service.rs`、`web/index.html`、`web/style.css`、`web/app.js`），无临时日志、断点、硬编码调试返回或临时路由；浏览器验证脚本保存在会话外部临时目录，不纳入仓库。
- `src/service.rs` 中新增的 `self.log(...)` 为正式运行期审计与诊断日志（供 `GET /logs` 导出排查 `PUT /schedule` 权限拒绝与配置变更），予以保留。
- 清理检查后执行 `tidy.py check --staged` 核对暂存快照。

## 风险与后续

- 本次修改尚未推送至远端或发布新版本；推送 `main` 并触发 `follow-host.yml` 后，将基于当前提交为 CPR `v3.55.2` 构建并发布正式版 `v0.1.1`（需用户显式授权推送与发版）。
