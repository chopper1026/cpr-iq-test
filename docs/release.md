# 验证、跟版与回滚

## 1. 首发前验证模型档位

`data/profiles.json` 只允许列出的「请求模型 × `reasoning.effort`」进入校准（当前仅登记 `gpt6-sol-low`，即 `gpt-6-sol` / `low`，处于 `validation: null`）；只有 `validation` 绑定到该档位及当前数据锁时才开放判定。插件不会用校准任务自动修改档位配置。

1. 在隔离测试实例中安装开发包，选用**已经授权**的正常 Codex OAuth 对照账号、独立测试 Key。确认宿主没有配置模型重写或推理档位覆盖。先看界面显示的消耗，再运行多轮手动校准；每轮三次，不提供凭据给开发者或 CI。
2. 核对对照应有的参考标签是否稳定排第一。记录宿主 tag、档位、参考库版本、完整且匹配的轮数、排名摘要及合适的头两名最小差（单位 bp，100 bp = 1 个百分点）。无法稳定通过的档位留为 `validation: null`。测试输出的 **候选差** 是参考库内的统计信息，不是身份置信度。
3. `python3 scripts/detector_assets.py --profiles` 列出待验证档位的 `profileStamp`。将证据填入 `validation`：`profileStamp`、`hostTag`、`evidence`（不含账号、Token、Key 或原始回复的评审记录引用）、`positiveRounds` 和 `minimumMarginBps`。不要为了通过 CI 手工编造证据。`python3 scripts/detector_assets.py --profiles --require-validated` 只校验记录格式和版本绑定，不负责证明记录真实。
4. 用仓库的异标签离线用例验证 `Rank → 复测 → 疑似`，另验证授权正常账号的正式结论。只在两个方向均通过后签发生产版本。SDK/数据锁/探针请求策略变化后重新验证档位，不能把旧证据沿用到新数据。

## 2. 本地隔离验证

- 拿官方宿主对应平台安装归档，核对官方 SHA-256，解压后指定包含 `codex-proxy-rs`、`web/dist` 与 `plugins/official/plugin-release-manifest.json` 的目录。
- 如需验证版本切换，可在 `scripts/package.py` 的参数里追加 `--baseline-out .work/baseline` 生成只供验收的 `0.0.0` 测试包。**不要安装该基线包到生产**。
- 启动测试所需的 PostgreSQL、Redis 可选择 `--docker-services`（脚本会创建并回收新的本地容器）；不加此选项则需 PATH 中有 `initdb`、`postgres`、`psql`、`redis-server`，脚本在临时目录和动态回环端口启动。不会读取已有宿主设置、账号或库。

```bash
python3 scripts/smoke_host.py \
  --host-dir .work/host-unpacked/codex-proxy-rs --host-tag v3.54.5 \
  --host-sha 45a4d580c7e1ac2b2c0308d72ae5018148f2516a \
  --package dist/cpr-iq-test.account-fingerprint-0.1.0-dev.1-aarch64-apple-darwin.tar.gz \
  --baseline .work/baseline/cpr-iq-test.account-fingerprint-0.0.0-aarch64-apple-darwin.tar.gz \
  --docker-services --log .work/smoke.log
```

Smoke 验证真实宿主上传、信任、启用、目录和静态资源、私有状态读写及不支持档位拒绝。传入 `--baseline` 时再验证旧包 → 新包 → 重启的设置和历史持久性（历史来自独立测试库中的合成 fixture）。不发送真实模型调用。插件 Rust 测试验证请求中显式绑定 A、B 不被调用和失败分类；上游 Core 合同测试额外验证已绑定账号失败后不会轮换。`tests/known_other.json` 的两轮已知标签样本来自锁定的 `lm-detector` 参考语料，仅验证判定链路，不是独立准确率评估。

## 3. 启用自动跟版

1. 将项目放到自己的 GitHub 仓库默认分支，确保有 GitHub 托管的 Linux x86_64、Linux ARM64 和 macOS ARM64 runners。本地可先运行无外部副作用的只读预检：
   `python3 scripts/release_gate.py preflight`（追加 `--online --repository <owner>/<repo>` 可只读查询上游最新正式版、兼容表与草稿冲突）。
2. 保持仓库变量 `CPR_ENABLE_PUBLISH` 未设置时，定时触发会自动跳过，避免开发预览阶段反复冷构建三平台；维护者可手动触发 **Verify new CPR stable releases**（可指定 `host_tag`，如 `v3.54.5`；开启发布后也可勾选 `dry_run=true` 只演练验证与构建而不写入远端）。如需把三平台真机 smoke 通过的开发预览包挂到 GitHub Pre-release 便于测试服务器下载，可在手动触发时勾选 `prerelease=true`：工作流会按 `X.Y.Z-dev.N`（如 `v0.1.0-dev.1`）三平台构建并在 Linux 记录二进制最大 `GLIBC_*` 依赖，核对远端附件摘要后以 `--prerelease --latest=false` 发布，不占用正式版本号且不写 `cpr-compatibility` 兼容表。首次没有旧包时会直接跳过 `old` 三平台矩阵，进入三平台重建。
3. 查看 `host-plan`、三平台输出、`release-decision` 与失败日志。脚本先校验 `plugin.json` 与 `Cargo.toml` 版本一致、锁定正式 tag 和 commit，再核对宿主二进制中的发布声明与源代码清单。若契约存在问题直接阻断并跳过昂贵的合同编译与旧包矩阵；旧包若覆盖目标版本并全部通过则复用；仅 `engines` 不覆盖、SDK 未变则重建清单；有兼容 SDK 变化则重编并再验。协议、必需能力、验收或摘要失败均禁止发布。兼容区间的下限是实际验证的宿主，不跨次版本声明支持。
4. 完成授权模型验证并提交证据后，维护者再设置仓库变量 `CPR_ENABLE_PUBLISH=true`（若缺少有效档位证据，`decide` 会在启动三平台重建前提前阻断）。仅默认分支的 `publish` 任务在非 `dry_run` 时具备 `contents: write`。它使用未使用过的插件补丁版本，先以草稿上传三个包、`checksums.txt` 和 `verification.json`，核对远端全部 5 份附件的大小与 SHA-256（无 API 摘要时下载回算）通过后才转为正式版并核对 tag 提交，最后写兼容表并产出 `publish-receipt` artifact；不覆盖任何已发布文件，遇到残留草稿直接拒绝并提示人工核查。`cpr-compatibility` 分支中的 `compatibility/vX.Y.Z.json` 是该宿主 tag 的验证记录，包含宿主 commit、插件版本、平台和 SHA-256。
5. 若上传、转正式或写兼容表阶段发生瞬时失败，可在同一默认分支提交上手动触发 **Verify new CPR stable releases** 并填入 `resume_run_id` 续跑（续跑预发布时仍须勾选 `prerelease=true`，以传入 `--allow-prerelease` 并保持与正式版及兼容表隔离）：`resume-verify` 会先校验原 run 的工作流路径、默认分支、`head_sha` 和上游成功任务，复核全部候选包摘要后再交由 `publish` 执行（支持搭配 `dry_run=true` 只做只读复核）。若同提交、同摘要的正式 Release 已存在而仅缺兼容表，`publish` 会幂等复用已发布 Release 补写兼容表，无需重新编译三平台。
6. **Early warning against CPR main** 只做合同比较、编译和测试。遇到接口变更，修 `src/host.rs` 或 `src/service.rs` 的宿主接入，确认私有状态兼容；不需要修改探针或指纹策略来追宿主接口版本。

## 4. 生产切换

先查对应宿主 tag 的**已验证**记录，再核对平台摘要并从宿主管理页安装。在测试实例核对管理页和历史后，由管理员切换生产实例。插件不提供独立的导出、备份或恢复命令，切换前由宿主管理员按宿主手册自行保留上一组宿主/插件制品与宿主 PostgreSQL 备份；完整状态覆盖、配额与破坏性操作影响见[持久化与备份边界](development/PERSISTENCE.md)。若新宿主无法运行旧插件，按经过验证的**宿主＋插件版本对**回滚，不绕过兼容校验，也不要删除实例（删除会移除私有状态，且无法从安装包重建）。
