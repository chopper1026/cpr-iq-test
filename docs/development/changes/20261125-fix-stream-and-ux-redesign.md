# 2026-11-25 流解析截断修复、模型目录自动发现与管理页重构

- 记录 ID：`20261125-fix-stream-and-ux-redesign`
- 范围：`host`、`worker`、`fingerprint`、`web`、`release`、`docs`

## 根因排查（线上 `v0.1.0-dev.1` 校准失败）

线上在 `codex-proxy-rs v3.54.5` 对 `gpt-6-sol / low` 触发手动校准，宿主侧请求日志显示上游正常返回（输入 244 tokens、输出 1,037 tokens、耗时 20.8s），但插件记录为「证据不足：回复未完整结束、被截断或不可用（`Reason::Incomplete`）」。

经逐行核对宿主 `crates/providers/openai/src/transport/canonical.rs` 与插件 `src/host.rs` 定位到确切根因：

1. **Responses 稀疏 `content_index` 被误拒（主因）**：宿主 `CodexCanonicalDecoder` 按 `output_index * 1024 + part_index` 计算 `CanonicalEvent::TextDelta.index`。推理模型输出的第 0 项为 `reasoning`（`index = 0`），第 1 项才是可见文本 `message`（`index = 1024`）。插件 `VisibleText::push` 原先误以 `index < MAX_PARTS as u32`（`MAX_PARTS = 32`）限制索引值而非分片数，导致第一条可见文本分片 `index = 1024` 立即落入 `Reason::Incomplete` 并关闭流。
2. **高推理档位流读取批次与超时过紧（隐患）**：`READ_BATCHES = 4096` 与 `MODEL_TIMEOUT = 85s` / `ROUND_BUDGET_MS = 300_000` 对 `high` / `xhigh` / `max` 等长推理流偏紧，已同步提升至 `65_536`、`180s` 与 `600_000ms`。
3. **非推理模型默认档位兼容**：`effort` 为空或 `default` 时不再强行发送 `reasoning` 字段。

## 实际变更

### 问题修复
- 修复 `src/host.rs` `VisibleText::push`，改按 `self.parts.contains_key(&index) || self.parts.len() < MAX_PARTS` 限制不同分片数量，正确接收 `index = 1024 / 2048` 等稀疏索引，并补充回归单测。
- 修复 `src/host.rs` 账号目录映射，改为优先返回并展示 `AuthRuntimeAccount.email`（同时保留 `name`、`planType` 与 `accountId`），并在队列/历史中持久化邮箱快照；读取旧版记录时自动回填实时邮箱。

### 新增功能与交互重构
- **自动获取全部模型与推理档位**：移除 `data/profiles.json` 单模型硬编码与校准锁，改为通过宿主内部管理分派 `host.http.dispatch`（`GET /api/admin/accounts/models/catalog?accountId=...`）跨不同套餐代表账号自动聚合全部模型及 `supported_reasoning_levels`，并结合 `host.models.list` 与 `/api/admin/accounts/models` 兜底补齐，结果内存缓存 10 分钟并支持手动刷新。
- **去除 24 小时探针预算、共享预算与校准模式**：统一手动检测与定时巡检逻辑；参考库内 76 个模型（含 `gpt-6-astra/6.1-sol` 别名）自动支持指纹匹配与两轮复测疑似判定，参考库外模型自动输出 Top-3 最接近参考排名与概率分布；定时巡检支持 1/6/12/24/48/168 小时周期；提供 `POST /runs/cancel` 取消待执行排队任务。
- **独立「任务记录」Tab 页**：重构 `web/index.html`、`web/style.css`、`web/app.js`，分为「检测与巡检」与「任务记录」双 Tab 页；账号选择器支持邮箱优先展示、关键字搜索与全选/清空；任务记录页支持快捷状态计数标签、邮箱/ID/模型搜索、来源/状态/模型/档位筛选与完整页码分页（服务端 `GET /runs` 过滤与分页）。
- **无损兼容旧版状态**：`schemaVersion: 1` 反序列化兼容 `v0.1.0-dev.1` 保存的 `probeBudget`、`reservations`、`profileId`、`mode` 与 `verdict: calibration`，并在 `state_codec::encode_bounded` 中保障 500 条历史压缩后不超 58 KiB RPC 上限。

### 脚本、测试与文档
- 移除 `data/profiles.json`；同步更新 `scripts/package.py`、`scripts/detector_assets.py`、`scripts/follow_host.py`（补齐 `host.models.list` 与 `host.http.*` 回调契约）、`scripts/smoke_host.py`、`scripts/release_gate.py`、`.github/workflows/follow-host.yml`、`tests/test_release.py`、`.tidy.json` 与全部项目文档。

## 验证证据

- `cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings`、`CPR_REQUIRE_BANK=1 cargo test --locked --all-targets`（19 项通过，含 `TextDelta(1024/1025)`、邮箱优先、目录合并、筛选分页、500 条历史压缩、旧版 `v0.1.0-dev.1` 迁移）
- `python3 scripts/detector_assets.py --verify-only`
- `python3 -m unittest discover -s tests -p 'test_*.py'`（17 项通过）
- `node --check web/app.js`、`.work/actionlint/actionlint`、`follow_host.contract`（16 项回调无问题）
- 真实隔离宿主 `codex-proxy-rs v3.54.5`（macOS ARM64 + Docker PostgreSQL/Redis）：`scripts/package.py` 打包 `0.1.0-dev.2` 与 `0.0.0` 基线包，`scripts/smoke_host.py --docker-services` 全流程通过（含旧版状态迁移、邮箱搜索、筛选分页、版本切换与重启保留）。
- 未验证项：线上真实 OpenAI OAuth 账号端到端调用与 Linux x86_64 / ARM64 CI 三平台构建。
