# 2026-11-26 复用 Codex Proxy UI 样式、新增账号实时状态、每日固定时间巡检与正式版双上游自动更新

- 日期：2026-11-26（Asia/Shanghai）
- 状态：已完成本地实现与隔离宿主/浏览器验证，待推送及远端正式发版
- 关联功能：CPR-01、CPR-03、CPR-04、CPR-05、CPR-06、CPR-07、CPR-08

## 变更明细

### 新增功能
1. **复用 Codex Proxy 现有 UI 样式与组件**（`web/index.html`、`web/style.css`、`web/app.js`）：
   - 接入宿主桥 `bridge-runtime.ts` 实时注入的 `@codex-proxy/ui` `--z-*` 主题令牌（含浅色/暗黑模式及 Relay Blue / Deep Teal / Signal Violet / Graphite 等自定义皮肤与圆角/阴影/控件高度），透明画布直接融入宿主页面，并提供独立访问时的 Relay Blue 回落令牌。
   - 移除全部原生 `<select>` 与原生复选/开关样式，统一实现对齐 `@codex-proxy/ui` 的自定义 `ZSelect` 下拉框（支持键盘导航、内联搜索、辅助描述与徽章、自动上下翻转避让）、`ZSwitch` 开关、`ZSegmented` 分段器、`ZCard` 卡片、`ZButton`、`ZInput`、`ZTag`、`ZCheckbox` 与 `ZTable`。
2. **账号实时状态视图与接口**（`src/host.rs`、`src/service.rs`、`web/*`）：
   - `AccountChoice` 增加实时 `credentialState` 字段；新增管理路由 `GET /accounts/status`，汇总所有启用 OpenAI OAuth 账号的当前队列进度（执行中轮次 / 排队位次）、最近一次检测结论与 Top-3 候选、失败时的上次有效指纹回溯、近 5 次结果趋势与全局状态统计。
   - 管理页新增「账号状态」Tab（默认首屏，页面可见时每 5 秒自动同步），支持按状态/套餐筛选、按状态优先级/最近检测/邮箱排序、一键发起单账号检测、跳转按账号过滤的「任务记录」与直达最近诊断日志；「检测与巡检」的账号多选器同步展示实时状态标签并支持一键选中「异常/未测」账号。
3. **定时巡检支持每日固定时间与时区**（`src/domain.rs`、`src/service.rs`、`web/*`）：
   - 新增 `ScheduleMode::Interval | Daily`、`dailyTimes`（1–12 个 `HH:MM` 时间点）与 `utcOffsetMinutes`（-720..=840，15 分钟步长）；保持 `v0.1.0-dev.*` 私有状态零破坏向后兼容迁移。
   - 每日固定时间模式下按时区精确计算下次时间槽，跨日/停机后仅触发最近到期时间槽、绝不补跑历史积压；前端通过分段器切换「按间隔巡检 / 每日固定时间」，提供小时/分钟 `ZSelect` 下拉框、动态增删时间点与浏览器本地时区自动识别。
4. **正式版发布与双上游（Codex Proxy + lm-detector）自动更新**（`scripts/sync_upstreams.py`、`scripts/follow_host.py`、`scripts/release_gate.py`、`scripts/package.py`、`.github/workflows/follow-host.yml`）：
   - 打包默认版本切换为正式版 `0.1.0`；发布链路面向正式 `v0.1.x`（带 `Latest` 标记，可直接被 Codex Proxy 的 GitHub 插件更新检查识别）。
   - 新增 `scripts/sync_upstreams.py` 与工作流 `sync-upstreams` 作业，同时跟踪 `zyycn/codex-proxy-rs` 最新稳定版（SDK / `engines` / Rust 工具链 + 16 项回调契约）与 `Ikaleio/lm-detector` `main`（精确监控 `data/unified_bank.json`、`data/shared_detector.bin` 与 `sdk/rust/`，忽略仅改网页/文档提交），验证通过后自动同步锁定文件并触发三平台构建、隔离宿主 smoke 与正式发布。
   - 新增 `packageSourceHash` 覆盖运行时与打包源码，避免纯文档提交误发包，同时在插件代码或 `lm-detector` 更新时跳过旧包复用并强制重建；`cpr-compatibility` 分支支持同一 `hostCommit` 下更严格的更高正式版单调更新（保留 `previousVersions` 审计链，继续拦截降级与宿主 tag 移动）。
5. **可选指纹异常自动停用与账号备注写入**（`src/domain.rs`、`src/host.rs`、`src/service.rs`、`plugin.json`、`web/*`）：
   - `Settings` 新增 `autoDisableOnSuspect`（默认 `false`，兼容旧状态自动补齐），单独切换不重置进行中的定时计划或队列。
   - 仅当账号经两轮独立复测确认为 `Verdict::Suspect`（手动检测与定时巡检均适用）且 `autoDisableOnSuspect == true` 时，维护回调先通过内部管理 `host.http.dispatch` 调用 `GET /api/admin/accounts/detail` 读取现有并发上限、权重、分组与备注，再调用 `POST /api/admin/accounts/update` 将账号设为 `enabled: false` 并在备注顶部写入 `[指纹异常] <YYYY-MM-DD HH:MM UTC±HH:MM> | <模型(档位)> 两轮复测疑似异常，判定 #1 <候选(概率)>，已自动停用`（自动替换旧 `[指纹异常]` 行，保留原有手工备注并遵守宿主 500 字符上限，省略名称/代理/模型访问/连接字段以无损保留宿主配置）。
   - `RunDiagnostic`、`GET /accounts/status`、`GET /runs`、`GET /logs` 与管理页全面记录并展示 `autoDisabled` / `autoDisableNote` / `autoDisableError` 审计信息；「账号实时状态」保留已有历史记录的已停用账号，避免自动停用后账号悄然消失；「检测与巡检」顶部新增「异常自动停用」卡片与即时保存 `ZSwitch` 开关。

### 问题修复
- 修复 `src/fingerprint.rs` 测试在复用 `CARGO_TARGET_DIR`（如二次本地打包或 CI 命中 Cargo 缓存）时 `env!("CARGO_MANIFEST_DIR")` 指向已销毁临时目录的问题，改为优先按当前工作目录读取 `data/`。

### 架构与功能影响
- 更新 `README.md`、`docs/release.md`、`docs/development/ARCHITECTURE.md`、`docs/development/PERSISTENCE.md`、`docs/development/FEATURES.md`、`docs/development/HANDOFF.md` 与 `.tidy.json`。

## 验证结果

- 静态检查：`cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings`、`node --check web/app.js`、`actionlint` 全部通过。
- 单元测试：`CPR_REQUIRE_BANK=1 cargo test --locked --all-targets`（25/25 通过）、`python3 -m unittest discover -s tests -p 'test_*.py'`（22/22 通过）、`python3 scripts/detector_assets.py --verify-only` 通过。
- 联机只读预检：`scripts/release_gate.py preflight --online --repository chopper1026/cpr-iq-test` 与 `scripts/sync_upstreams.py --work .work/sync-dryrun` 通过（无远端写入），正确识别 CPR `v3.55.2`（契约无破损）与 `lm-detector` 最新提交。
- 真实宿主隔离 smoke 与浏览器验证：在 macOS ARM64 真实 `codex-proxy-rs v3.54.5` 上完成 `0.0.0 → 0.1.0` 升级切换、旧版迁移、`GET /api/accounts/status`、`autoDisableOnSuspect` 独立/组合持久化、每日固定时间持久化与宿主重启 smoke；使用 Chromium 实测真实插件 iframe 的浅色/暗黑模式皮肤跟随、「指纹异常自动停用」开关即时保存、自定义 `ZSelect` 下拉框弹层及通过桥接保存每日定时（0 Console 错误）。
- 未验证项：Linux `x86_64`/`aarch64` 远端构建与 smoke、远端 GitHub Actions 首个正式版 `v0.1.0` 实跑及线上真实 OAuth 账号实测（含实账号 `Suspect` 自动停用与备注展示）。
