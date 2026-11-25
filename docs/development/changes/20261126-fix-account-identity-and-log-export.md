# 2026-11-26 修复采样期间凭据版本自增误判并新增诊断日志导出

## 问题根因

线上实测 `v0.1.0-dev.2`（`wreck2026@gmail.com` / `gpt-6.2-sol` / `high`）时，新版的真实邮箱展示、12 个模型及推理档位自动获取、独立任务记录 Tab 页以及 Responses 稀疏 `content_index` 流解析均已正常工作（对照宿主下载的 `req_01a21387f2de77aaa0cdf724f46168fb` 诊断包：单次探针 `242` 输入 / `1078` 输出 / `111` 推理 token、`22.7s`、`response.completed` 成功返回，且 3 次探针全部执行完成）。

但在全部 3 次探针结束后，任务仍被记为 `证据不足：采样期间账号凭据版本发生变化`。排查 `codex-proxy-rs v3.54.5` 源码（`providers/openai/src/provider/execution.rs:963-981`、`credential/selector.rs:2122-2182` 与 `gateway-store/.../core_adapter.rs:272`）发现：

- OpenAI Codex 响应（含 WebSocket 握手）携带 `Set-Cookie`（如 Cloudflare/会话 cookie）时，宿主会调用 `capture_response_cookies()` 写回 `provider_credentials_json` 并自增 `credential_revision = credential_revision + 1`；后台同账号 OAuth `access_token` 自动刷新同样会自增 `credential_revision`。
- 上述操作**不改变上游账号身份**（`upstream_user_id`、`upstream_account_id`、`email` 均不变）。
- 插件旧实现在采样前后直接比对 `after.credential_revision != info.credential_revision` 并将 `credential_revision` 写入 `round_stamp`，导致正常请求完成后被误判为 `Reason::AccountChanged` 并丢弃全部 3 份有效探针输出。

## 变更

1. **稳定上游账号身份校验**（`src/host.rs`、`src/fingerprint.rs`、`src/service.rs`）：
   - 新增 `AccountIdentity`（优先绑定 `(provider_id, authentication_kind, upstream_user_id, upstream_account_id)`，缺失上游 ID 时退回归一化 `email`），替代 `credential_revision` 参与采样前后一致性检查与 `round_stamp` 计算。
   - 宿主因 Cookie 捕获或同账号 Token 刷新自增 `credential_revision` 时仅记录一条运行期诊断日志并继续评分；只有上游账号身份被替换或账号停用时才终止采样。
2. **逐探针诊断追踪与脱敏日志导出**（`src/domain.rs`、`src/host.rs`、`src/fingerprint.rs`、`src/state_codec.rs`、`src/service.rs`）：
   - 在 `HistoryItem` 新增可选持久化字段 `diagnostics: Option<RunDiagnostic>`（向后兼容旧版记录；`encode_bounded` 逼近 58 KiB 时先裁剪最旧诊断明细再裁剪历史），记录阶段、总耗时、凭据版本前后值、身份是否变更、期望标签、评分器状态（`decision`/`usedOutputs`/`probabilityStatus`）、错误详情以及 3 次探针的耗时/批次/事件/分片/可见字符/推理字符/结束原因/wire 终态，不存任何凭据明文或原始对话正文。
   - 在 `Service` 新增 300 条有界内存环形日志 `LogRing`，注册管理路由 `GET /logs`（支持按 `runId` 单条、当前筛选或全量导出完整脱敏 JSON 诊断包）。
3. **管理页日志查看与一键复制导出**（`web/index.html`、`web/style.css`、`web/app.js`）：
   - 在「任务记录」右上角增加 **导出诊断日志** 按钮、每行增加 **日志** 按钮，底部弹出诊断日志抽屉，提供单条任务的逐探针明细表格、JSON/纯文本诊断摘要切换与 **一键复制日志**（兼容 `codex-proxy-rs` 插件 iframe `sandbox="allow-scripts"` 沙箱环境下的 `execCommand('copy')` 回落，并在非沙箱环境提供直接下载）。
4. **验证脚本与文档同步**（`scripts/smoke_host.py`、`README.md`、`docs/development/*.md`）。

## 验证计划

- Rust 格式化、Clippy (`-D warnings`) 与全量单测（含 `CPR_REQUIRE_BANK=1`）。
- `node --check web/app.js`、Python 发布单测。
- 本机 macOS ARM64 隔离真实 `codex-proxy-rs v3.54.5` 宿主 smoke（覆盖 `GET /api/logs`、`export-logs` 与旧状态切换迁移）。
- `tidy.py sync`、`tidy.py audit-docs`、`tidy.py check`。
