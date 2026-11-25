# 项目架构

2026-11-25（Asia/Shanghai）首次盘点：覆盖 Rust 可执行文件、管理页、锁文件、打包脚本和工作流。实际通过的环境与能力见[功能全景](FEATURES.md)，不从架构描述推断生产验收。

## 系统边界

`cpr-iq-test` 是 `codex-proxy-rs` 加载的独立原生插件，不 fork 宿主。宿主管理员通过嵌入页选账号和档位；宿主拥有管理员权限检查、OAuth 凭据、Client Key、模型执行、限流及计费。插件使用账号及 Key ID，返回参考指纹排名和结论，不写账号、路由或计费配置，不把排名当成模型身份认证。

[清单](../../plugin.json)仅声明 `management v2` 和 `maintenance v1`，运行模式为 `trustedProcess`，不是沙箱。管理资源由宿主提供，不需要独立 Web 服务。插件进程通过 stdin/stdout 与宿主建立 SDK 会话；模型请求只走宿主回调，参考评分只使用包内数据。

## 模块与依赖

| 模块 | 入口 | 职责与依赖 |
| --- | --- | --- |
| 进程入口 | [main.rs](../../src/main.rs)、[清单](../../plugin.json) | 加载包内参考数据和内嵌清单，注册管理与维护回调。数据损坏时管理页仍可用，不发送探针。 |
| 管理页 | [HTML](../../web/index.html)、[app.js](../../web/app.js) | 通过宿主桥接读取 `GET /catalog`、`GET /runs`、`GET /schedule`；调用 `POST /runs` 和 `PUT /schedule`。展示预计消耗、排队及摘要；浏览器不直接调用模型。 |
| 管理与维护服务 | [service.rs](../../src/service.rs) | 校验目标、档位、权限和设置；触发状态转换；每次维护调用最多跑一个账号的一轮，对接宿主适配与评分。 |
| 宿主适配 | [host.rs](../../src/host.rs) | 查询启用账号、启用 Key 和访问权限；执行绑定账号的 Responses 流；提取可见文本；封装状态 CAS。宿主 RPC 变化优先在这里与服务边缘处理。 |
| 调度与策略 | [domain.rs](../../src/domain.rs)、[state_codec.rs](../../src/state_codec.rs) | 管理有界队列、巡检时间槽、滚动预算、摘要和保守判定。状态转换不依赖网络；编解码有大小上限。 |
| 离线指纹 | [fingerprint.rs](../../src/fingerprint.rs)、[数据锁](../../data/detector.lock.json)、[档位](../../data/profiles.json) | 校验同版本的 `unified_bank.json` 与 `shared_detector.bin`；使用 `lmfpd` 生成探针、评分并输出参考标签。档位证据绑定请求模型、推理档位和参考库。 |

## 执行流程

1. 管理页列出启用的 OpenAI OAuth 账号和启用的 Client Key，管理员应选专用测试 Key。提交与执行前均检查 Key ID、目标账号及模型访问权。`host.model.execute_stream` 显式传 `provider=openai` 和 `account_id`；不发起未固定账号的替代请求。
2. 手动单测、批量与定时巡检共用队列：每批 1–10 个账号、每实例最多 30 个任务。巡检默认关闭，间隔可选 24 小时、48 小时、7 天；到期只调度一个时间槽，不补跑错过的周期。每实例滚动 24 小时预算默认 60 次，可降到 3 次。
3. 维护先以同一个私有状态版本提交认领、时间槽变更和本轮 3 次额度预留，再开始串行调用。CAS 冲突只重试纯状态转换，不重放已经发出的模型调用。重启后仍可领取未发出的任务；已认领但结果未知的记为中断，不自动重跑该轮。预留按最晚发出时刻保守计入窗口，实际消耗可能更少。
4. 评分只接收三份正常结束的可见文本，拒绝被中间件改写、截断或异常结束的流。已校准档位在首轮目标排名第一且分差达标时判断未发现异常；首轮分差达标而目标掉出前三才用三个新探针复测。只有两轮均达标、同一非目标第一且目标都不在前三才判疑似。校准模式仅显示一轮排名，不给结论。账号版本或评分条件变化、证据不足、执行失败和预算跳过都不等于指纹不符。

## 持久化与恢复边界

| 数据 | 位置与保留 | 备份、重建或切换边界 |
| --- | --- | --- |
| 巡检参数、目标 ID、队列、时间槽、手动幂等键、预算、历史摘要 | 宿主私有状态 `iq-test / state`，`schemaVersion: 1`，单条受限压缩文档。历史保留最近 100 条且不超过 30 天；记录候选、档位、账号 ID、版本等摘要，不留探针、原始回复或凭据。 | 插件没有独立导出、还原或数据库备份命令。原实例的版本切换依赖宿主保留私有状态；没有该状态时使用默认设置，历史和在途额度不能从安装包重建。跨宿主版本恢复或状态格式迁移须单独验证。 |
| SDK、两份参考数据、档位证据 | 依赖锁和数据锁随源码管理；校验过的两份数据随包分发。 | 可按锁定版本重新构建，不应从生产状态中提取。SDK 与参考库独立升级；更换参考数据或探针策略要重新验证档位。 |
| OAuth 凭据、Key 明文、账号与路由配置 | 由宿主拥有，不属于插件私有状态。 | 不纳入插件包、插件历史或本仓库备份；宿主级备份与权限治理在本仓库范围之外。 |
| 安装包、浏览器证据、构建缓存和临时日志 | 本地忽略区；共享文档只写可脱敏的摘要和必要的版本、哈希。 | 构建产物可重建；临时诊断不是共享记录，也不是生产备份。 |

## 构建、测试与交付

- [Cargo.toml](../../Cargo.toml)、[rust-toolchain.toml](../../rust-toolchain.toml)定义 Rust 构建入口及工具链；[detector_assets.py](../../scripts/detector_assets.py)校验参考数据，[package.py](../../scripts/package.py)用指定宿主提交的 `cpr-plugin` 产出目标平台包和 SHA-256。
- Rust 测试覆盖绑定、流解析、状态和离线判定；[Python 回归](../../tests/test_release.py)覆盖跟版决策；[check_bound_host.sh](../../scripts/check_bound_host.sh)检查宿主不轮换绑定账号的契约，[smoke_host.py](../../scripts/smoke_host.py)用真实隔离宿主验证安装、运行及状态切换。操作和回退步骤以[验证、跟版与回滚](../release.md)为准。
- [正式版跟版](../../.github/workflows/follow-host.yml)先锁宿主 tag/commit，再跑合同检查、旧包验证或重建验证。发现与发布闸门在 [follow_host.py](../../scripts/follow_host.py) 和 [release_gate.py](../../scripts/release_gate.py)。[main 预警](../../.github/workflows/host-main.yml)不发包；[常规检查](../../.github/workflows/checks.yml)独立运行。生产安装与切换始终由管理员确认。
