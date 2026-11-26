# 验证、跟版与回滚

## 1. 本地隔离验证

- 获取对应平台的官方宿主归档，核对官方 SHA-256，解压后指定包含 `codex-proxy-rs`、`web/dist` 与 `plugins/official/plugin-release-manifest.json` 的目录。
- 如需验证版本升级与旧状态兼容迁移，可在 `scripts/package.py` 追加 `--baseline-out .work/baseline` 生成仅供验收的 `0.0.0` 基线包。**不要将基线包安装到生产环境**。
- 启动测试所需 PostgreSQL 与 Redis 可传入 `--docker-services`（脚本自动创建并回收临时本地容器）；不传则需 PATH 中具备 `initdb`、`postgres`、`psql`、`redis-server`，脚本在临时目录与动态回环端口启动，不接触宿主机已有配置或数据。

```bash
python3 scripts/smoke_host.py \
  --host-dir .work/host-unpacked/codex-proxy-rs --host-tag v3.54.5 \
  --host-sha 45a4d580c7e1ac2b2c0308d72ae5018148f2516a \
  --package dist/cpr-iq-test.account-fingerprint-0.1.0-aarch64-apple-darwin.tar.gz \
  --baseline .work/baseline/cpr-iq-test.account-fingerprint-0.0.0-aarch64-apple-darwin.tar.gz \
  --docker-services --log .work/smoke.log
```

Smoke 在真实宿主二进制上验证：上传、信任、启用、三 Tab 管理页资源、`GET /api/catalog`、`GET /api/accounts/status`、`GET /api/runs` 筛选与分页、`GET /api/logs`、`POST /api/runs/cancel`、`PUT /api/schedule`（含每日固定时间与时区偏移）读写与非法输入拒绝；传入 `--baseline` 时同时注入现代记录与 `v0.1.0-dev.1` 旧版记录，验证版本切换与宿主重启后的状态保留、旧版迁移和邮箱筛选。全程不触发真实上游 OAuth 或模型调用。

插件 Rust 单测额外验证绑定账号不回落、Responses 稀疏 `content_index` 解析、默认推理档位省略、原生模型目录合并、邮箱与凭据状态展示、账号实时状态聚合及每日固定时间调度；`scripts/check_bound_host.sh` 复核宿主 Core 绑定账号不轮换契约；`tests/known_other.json` 复用锁定参考语料验证两轮疑似判定链路。

## 2. 启用正式版双上游自动跟版

1. 将仓库置于 GitHub 默认分支，并确保拥有 Linux x86_64、Linux ARM64 和 macOS ARM64 runners。本地可先执行只读预检：
   - `python3 scripts/release_gate.py preflight`（追加 `--online --repository <owner>/<repo>` 可只读核对 `codex-proxy-rs` 最新正式版、`lm-detector` 参考库/SDK 是否有变更、兼容表和草稿状态）。
   - `python3 scripts/sync_upstreams.py --work .work/sync`（只读检测双上游差异；追加 `--apply` 在本地应用锁定更新并执行全套测试验证）。
2. 设置仓库变量 `CPR_ENABLE_PUBLISH=true` 后，**Follow CPR and lm-detector releases**（`.github/workflows/follow-host.yml`）每 30 分钟自动巡检以下两类上游及本仓源码变更，并按正式版序列（`v0.1.0` → `v0.1.1` → …）发布带 `Latest` 标记的 GitHub 正式 Release：
   - **Codex Proxy（`zyycn/codex-proxy-rs` 正式版）**：先校验 `plugin.json` 与 `Cargo.toml` 版本一致、锁定宿主 tag/commit 与全部 16 项回调契约；若仅 patch 升级且 SDK、`lm-detector` 与包源码摘要（`packageSourceHash`）均未变，直接三平台 smoke 旧正式包并在通过后复用；若跨 minor 或 SDK/Rust 工具链更新且契约无破损，`sync-upstreams` 验证通过后同步 `plugin.json` / `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml` 至默认分支并触发三平台重编与隔离 smoke。
   - **lm-detector（`Ikaleio/lm-detector` `main`）**：精确监控 `data/unified_bank.json`、`data/shared_detector.bin` 与 `sdk/rust/`（忽略仅改官网 `web/` 或文档的提交）；检测到变更时自动下载校验新参考库、更新 `data/detector.lock.json` 与 `Cargo.toml`/`Cargo.lock` 的 `lmfpd` 锁定提交，在通过 Rust 全量单测（含 `tests/known_other.json` 两轮复测夹具）、clippy、宿主绑定契约与 Python 单测后提交至默认分支，随后三平台重编、隔离 smoke 并发布新正式版。
   - **本仓插件源码更新**：通过 `packageSourceHash` 对比 `src/`、`web/`、`Cargo.*`、`plugin.json`、`data/detector.lock.json`；仅改文档不触发发包，存在真实代码或参考库变更时自动构建发布新正式版，并在 `cpr-compatibility` 分支以单调升级方式更新 `compatibility/vX.Y.Z.json`（保留 `previousVersions` 审计链，严禁降级或改写已移动的宿主 commit）。
3. 未设置 `CPR_ENABLE_PUBLISH=true` 时，定时触发自动跳过；维护者仍可手动触发工作流，按需指定 `host_tag`、勾选 `dry_run=true` 演练、勾选 `prerelease=true` 发布 `vX.Y.Z-dev.N` 预览包，或调整 `sync_upstreams` 开关。
4. `publish` 阶段先创建空草稿 Release，再按顺序逐个以 `--clobber` 上传三个平台包、`checksums.txt` 和 `verification.json`（对 GitHub 瞬时 5xx/429 及上传策略错误做有界退避重试），逐一核对远端 5 份附件的大小与 SHA-256（缺 API 摘要时下载回算）后才转为正式版并标记 `Latest`，随后更新 `cpr-compatibility` 分支并留存 `publish-receipt` artifact；遇到残留草稿直接拒绝，不覆盖已发布资产。
5. 若上传、转正或写兼容表发生瞬时失败，可在同一默认分支提交手动触发工作流并填入 `resume_run_id`（预发布续跑须同时勾选 `prerelease=true`）：`resume-verify` 校验原 run 来源、分支、commit（含 `sync-upstreams` 推进的提交）及前置成功任务并复核候选包摘要后交由 `publish` 幂等续跑。
6. **Early warning against CPR main** 仅做合同比对、编译与测试，不发布资产；若上游宿主接口变化，优先在 `src/host.rs` 与 `src/service.rs` 适配。

## 3. 生产切换与回滚

先核对目标宿主 tag 对应的验证记录与平台包 SHA-256，在隔离测试实例核对目录拉取、检测执行与任务记录后再由管理员切换生产实例。插件不提供内置备份或导出命令，切换前由宿主管理员按宿主手册备份上一组制品与宿主 PostgreSQL（详见[持久化与备份边界](development/PERSISTENCE.md)）。需回滚时按已验证的**宿主＋插件版本对**执行实例切换，**不要通过删除实例降级**，以免移除插件私有状态且无法从安装包恢复。
