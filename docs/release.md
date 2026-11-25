# 验证、跟版与回滚

## 1. 本地隔离验证

- 获取对应平台的官方宿主归档，核对官方 SHA-256，解压后指定包含 `codex-proxy-rs`、`web/dist` 与 `plugins/official/plugin-release-manifest.json` 的目录。
- 如需验证版本升级与旧状态兼容迁移，可在 `scripts/package.py` 追加 `--baseline-out .work/baseline` 生成仅供验收的 `0.0.0` 基线包。**不要将基线包安装到生产环境**。
- 启动测试所需 PostgreSQL 与 Redis 可传入 `--docker-services`（脚本自动创建并回收临时本地容器）；不传则需 PATH 中具备 `initdb`、`postgres`、`psql`、`redis-server`，脚本在临时目录与动态回环端口启动，不接触宿主机已有配置或数据。

```bash
python3 scripts/smoke_host.py \
  --host-dir .work/host-unpacked/codex-proxy-rs --host-tag v3.54.5 \
  --host-sha 45a4d580c7e1ac2b2c0308d72ae5018148f2516a \
  --package dist/cpr-iq-test.account-fingerprint-0.1.0-dev.2-aarch64-apple-darwin.tar.gz \
  --baseline .work/baseline/cpr-iq-test.account-fingerprint-0.0.0-aarch64-apple-darwin.tar.gz \
  --docker-services --log .work/smoke.log
```

Smoke 在真实宿主二进制上验证：上传、信任、启用、管理页资源、`GET /api/catalog`、`GET /api/runs` 筛选与分页、`POST /api/runs/cancel`、`PUT /api/schedule` 读写与非法输入拒绝；传入 `--baseline` 时同时注入现代记录与 `v0.1.0-dev.1` 旧版记录，验证版本切换与宿主重启后的状态保留、旧版迁移和邮箱筛选。全程不触发真实上游 OAuth 或模型调用。

插件 Rust 单测额外验证绑定账号不回落、Responses 稀疏 `content_index` 解析、默认推理档位省略、原生模型目录合并与邮箱优先展示；`scripts/check_bound_host.sh` 复核宿主 Core 绑定账号不轮换契约；`tests/known_other.json` 复用锁定参考语料验证两轮疑似判定链路。

## 2. 启用自动跟版

1. 将仓库置于 GitHub 默认分支，并确保拥有 Linux x86_64、Linux ARM64 和 macOS ARM64 runners。本地可先执行只读预检：
   `python3 scripts/release_gate.py preflight`（追加 `--online --repository <owner>/<repo>` 可只读核对上游最新正式版、兼容表和草稿状态）。
2. 未设置仓库变量 `CPR_ENABLE_PUBLISH=true` 时，定时触发自动跳过。维护者可手动触发 **Verify new CPR stable releases**（指定 `host_tag`，如 `v3.54.5`；可勾选 `dry_run=true` 仅演练构建与验证，或勾选 `prerelease=true` 发布 `vX.Y.Z-dev.N` 预发布预览包，不占用正式版本、不写 `cpr-compatibility`）。首次无旧包时自动跳过 `old` 三平台矩阵直接进入 `rebuild`。
3. 脚本在重建前先校验 `plugin.json` 与 `Cargo.toml` 版本一致、锁定宿主 tag/commit 与全部宿主回调契约（含 `host.models.list`、`host.http.dispatch`、`host.model.execute_stream` 等）。契约不符立即阻断；旧包覆盖目标版本且三平台 smoke 通过则复用；仅 `engines` 超界且 SDK 未变则清单重建；SDK 变更则三平台重编与 smoke。
4. 设置仓库变量 `CPR_ENABLE_PUBLISH=true` 后，默认分支的非 `dry_run` 运行在三平台 smoke 全部通过后执行 `publish`：先创建空草稿 Release，再按顺序逐个以 `--clobber` 上传三个平台包、`checksums.txt` 和 `verification.json`（对 GitHub 瞬时 5xx/429 及上传策略错误做有界退避重试），逐一核对远端 5 份附件的大小与 SHA-256（缺 API 摘要时下载回算）后才转为正式版并核验 tag 提交，随后写入 `cpr-compatibility` 分支 `compatibility/vX.Y.Z.json` 并留存 `publish-receipt` artifact；遇到残留草稿直接拒绝，不覆盖已发布资产。
5. 若上传、转正或写兼容表发生瞬时失败，可在同一默认分支提交手动触发工作流并填入 `resume_run_id`（预发布续跑须同时勾选 `prerelease=true`）：`resume-verify` 校验原 run 来源、分支、commit SHA 及前置成功任务并复核候选包摘要后交由 `publish` 幂等续跑。
6. **Early warning against CPR main** 仅做合同比对、编译与测试，不发布资产；若上游宿主接口变化，优先在 `src/host.rs` 与 `src/service.rs` 适配。

## 3. 生产切换与回滚

先核对目标宿主 tag 对应的验证记录与平台包 SHA-256，在隔离测试实例核对目录拉取、检测执行与任务记录后再由管理员切换生产实例。插件不提供内置备份或导出命令，切换前由宿主管理员按宿主手册备份上一组制品与宿主 PostgreSQL（详见[持久化与备份边界](development/PERSISTENCE.md)）。需回滚时按已验证的**宿主＋插件版本对**执行实例切换，**不要通过删除实例降级**，以免移除插件私有状态且无法从安装包恢复。
