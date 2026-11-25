# 项目架构

最近更新：2026-11-26（Asia/Shanghai）。覆盖 Rust 原生插件、双 Tab 管理页与诊断日志导出、离线参考指纹锁、打包脚本与自动化跟版工作流。实际验证状态与缺口见[功能全景](FEATURES.md)，不从架构描述推断全量验收。

## 系统边界

`cpr-iq-test` 是 `codex-proxy-rs` 加载的独立原生插件，不 fork 宿主。管理员通过嵌入管理页选择测试 Client Key、模型、推理档位和指定账号发起手动检测或定时巡检；宿主拥有管理员鉴权、OAuth 凭据、Client Key、模型目录、模型执行、限流与计费。插件只通过宿主回调使用账号与 Key ID，返回本地参考指纹排名和结论，不写账号、路由或计费配置，不把统计排名等同于身份认证。

[清单](../../plugin.json)仅声明 `management v2` 和 `maintenance v1`，运行模式为 `trustedProcess`，非沙箱。管理静态资源由宿主托管，无需独立 Web 服务。插件通过 stdin/stdout 建立 SDK 会话，模型请求与目录发现均走宿主回调，指纹评分仅读取包内锁定数据。

## 模块与依赖

| 模块 | 入口 | 职责与依赖 |
| --- | --- | --- |
| 进程入口 | [main.rs](../../src/main.rs)、[清单](../../plugin.json) | 加载包内锁定参考数据与内嵌清单，声明 600 秒维护阶段预算，注册管理与维护回调。参考数据损坏时管理页仍可读历史但不发送探针。 |
| 管理页 | [HTML](../../web/index.html)、[CSS](../../web/style.css)、[app.js](../../web/app.js) | 提供「检测与巡检」和「任务记录」两个独立 Tab 页及诊断日志抽屉；通过宿主桥接调用 `GET /catalog`、`GET /runs`（带筛选与分页参数）、`POST /runs`、`POST /runs/cancel`、`GET /logs`（单条/筛选/全量脱敏诊断导出）、`GET /schedule`、`PUT /schedule`。 |
| 管理与维护服务 | [service.rs](../../src/service.rs) | 管理模型目录 10 分钟内存缓存（支持 `refresh=1` 强刷）、运行期 300 条内存环形诊断日志、`GET /logs` 导出装配、服务端多维筛选与页码分页、待执行任务取消、手动与定时入队及串行维护调度；维护回调每次最多跑一个账号的一轮探针。 |
| 宿主适配 | [host.rs](../../src/host.rs) | 经 `host.auth.list` 返回邮箱优先账号目录；经内部管理 `host.http.dispatch`（`/api/admin/accounts/models/catalog`）与 `host.models.list` 自动聚合全部模型及推理档位；提取稳定上游账号身份（`upstream_user_id` / `upstream_account_id` / `email`），避免宿主捕获响应 `Set-Cookie` 或 OAuth 刷新触发 `credential_revision` 自增时误判账号变更；执行绑定账号的 `host.model.execute_stream`，正确处理 Responses 稀疏 `content_index`、采集逐探针诊断元数据并封装私有状态 CAS。 |
| 调度与状态 | [domain.rs](../../src/domain.rs)、[state_codec.rs](../../src/state_codec.rs) | 管理有界队列（≤ 200）、手动幂等键、灵活定时周期（1/6/12/24/48/168 小时，错过不补跑）、摘要历史（≤ 500 条、≤ 30 天，含逐探针脱敏诊断元数据）、`v0.1.0-dev.1` 状态向后兼容迁移与有界压缩编解码（接近 58 KiB 时优先裁剪最旧诊断明细再裁剪历史）。 |
| 离线指纹 | [fingerprint.rs](../../src/fingerprint.rs)、[数据锁](../../data/detector.lock.json) | 启动时校验 `unified_bank.json` 与 `shared_detector.bin` 长度与 SHA-256；通过 `lmfpd` 生成 3 份独立探针并离线评分（输出评分器诊断元数据），支持参考库全量 76 个模型及合并显示名别名匹配，返回 Top-3 候选及概率（bp）。 |

## 执行流程

1. **目录与模型发现**：管理页请求 `GET /catalog` 时，插件读取启用的 OpenAI OAuth 账号（优先展示真实 `email`，附带 `name`、`planType` 与 `accountId`）和启用的 Client Key，再选取最多 3 个不同订阅套餐的代表账号通过内部 `host.http.dispatch` 拉取原生 Codex 模型目录，合并全部模型及各自 `supported_reasoning_levels`，并结合 `host.models.list` 补齐额外模型，结果内存缓存 10 分钟并支持手动强刷。
2. **手动检测与定时巡检**：手动单批支持 1–100 个账号；定时巡检默认关闭，开启后按设定周期每次仅调度当前时间槽，不补跑历史积压。两者共用有界队列（上限 200），按 `(account_id, model, effort)` 去重，且无 24 小时探针预算或校准模式限制。待执行任务可通过 `POST /runs/cancel` 取消。
3. **串行执行与中断安全**：维护回调先通过私有状态 CAS 清理过期项、恢复异常中断的遗留认领、触发到期定时入队并认领下一项任务，再执行该账号本轮 3 次串行模型调用。调用前与调用后核验账号启用状态与**稳定上游账号身份**（`upstream_user_id` / `upstream_account_id` / `email`），同时在诊断日志中记录 `credential_revision` 变化；宿主在响应期间因 `Set-Cookie` 捕获或同账号 OAuth 刷新递增 `credential_revision` 不再误判为 `AccountChanged`，仅在上游账号身份被替换时拒绝合并样本。`host.model.execute_stream` 显式绑定 `provider=openai` 与 `account_id`，不回落其他账号。若进程重启导致已认领任务结果未知，标记为 `interrupted / restarted` 不自动重发。
4. **流解析与指纹判定**：流式解析仅收集正常结束（`FinishReason::Stop`）的可见文本，按不同分片数量（≤ 32）而非原始索引值做有界限制，兼容推理模型 `output_index >= 1`（`content_index = 1024, 2048...`），拒绝被截断、拒绝回答或被中间件改写的流。
   - 若被测模型在本地 76 模型参考库内：首轮目标排名第 1 且头两名分差 > 0 判 `match`（指纹匹配）；目标位于第 2/3 或平分判 `insufficient / nearby`；目标掉出前三则自动用 3 个新探针执行第 2 轮复测，两轮均掉出前三且第一名为同一非目标标签判 `suspect`（疑似指纹异常），否则判 `insufficient / inconsistent`。
   - 若被测模型未收录于当前参考库：一轮 3 次探针后返回 `ranked`（参考排名），展示 Top-3 最接近参考模型及概率分布。

## 持久化与恢复边界

- 插件在宿主中仅持有单一私有状态键 `iq-test / state`（`schemaVersion: 1`，由 [state_codec.rs](../../src/state_codec.rs) 做 zlib+Base64 有界压缩、由 [host.rs](../../src/host.rs) 以 CAS 提交），保存定时计划、排队任务、手动幂等键与摘要历史（含账号邮箱/名称快照）；不存探针正文、原始回复、OAuth 凭据或 Client Key 明文。
- 向后兼容 `v0.1.0-dev.1` 状态文档（自动忽略已移除的 `probeBudget`/`reservations`，将旧 `profileId`/`calibration` 迁移为 `model`/`effort`/`ranked`）。完整配额、备份覆盖与操作影响见[持久化与备份边界](PERSISTENCE.md)。

## 构建、测试与交付

- [Cargo.toml](../../Cargo.toml)、[rust-toolchain.toml](../../rust-toolchain.toml) 定义 Rust 工具链与依赖；[detector_assets.py](../../scripts/detector_assets.py) 校验参考数据；[package.py](../../scripts/package.py) 调用锁定宿主提交的 `cpr-plugin` 产出平台安装包与 SHA-256。
- Rust 单测覆盖绑定账号、稀疏 `content_index`、默认推理档位、模型目录合并、邮箱展示、筛选分页、旧版状态迁移与离线两轮判定；[Python 回归](../../tests/test_release.py)覆盖跟版决策；[check_bound_host.sh](../../scripts/check_bound_host.sh)验证宿主 Core 不轮换契约；[smoke_host.py](../../scripts/smoke_host.py)在隔离真实宿主上验证安装、双 Tab 资源、目录、筛选分页、旧版迁移切换与重启。
- [正式版跟版](../../.github/workflows/follow-host.yml)通过 [follow_host.py](../../scripts/follow_host.py) 和 [release_gate.py](../../scripts/release_gate.py) 执行契约核验、旧包复用/三平台重建、隔离 smoke、`prerelease` 预览、远端附件摘要核验及 `resume_run_id` 断点续跑。[main 预警](../../.github/workflows/host-main.yml)仅预警不发包；[常规检查](../../.github/workflows/checks.yml)独立运行。
