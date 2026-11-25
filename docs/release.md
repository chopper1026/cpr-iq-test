# 验证、跟版与回滚

## 1. 首发前验证模型档位

`data/profiles.json` 只允许列出的「请求模型 × `reasoning.effort`」进入校准；只有 `validation` 绑定到该档位及当前数据锁时才开放判定。插件不会用校准任务自动修改档位配置。

1. 在隔离测试实例中安装开发包，选用**已经授权**的正常 Codex OAuth 对照账号、独立测试 Key。确认宿主没有配置模型重写或推理档位覆盖。先看界面显示的消耗，再运行多轮手动校准；每轮三次，不提供凭据给开发者或 CI。
2. 核对对照应有的参考标签是否稳定排第一。记录宿主 tag、档位、参考库版本、完整且匹配的轮数、排名摘要及合适的头两名最小差（单位 bp，100 bp = 1 个百分点）。无法稳定通过的档位留为 `validation: null`。测试输出的 **候选差** 是参考库内的统计信息，不是身份置信度。
3. `python3 scripts/detector_assets.py --profiles` 列出待验证档位的 `profileStamp`。将证据填入 `validation`：`profileStamp`、`hostTag`、`evidence`（不含账号、Token、Key 或原始回复的评审记录引用）、`positiveRounds` 和 `minimumMarginBps`。不要为了通过 CI 手工编造证据。`python3 scripts/detector_assets.py --profiles --require-validated` 只校验记录格式和版本绑定，不负责证明记录真实。
4. 用仓库的异标签离线用例验证 `Rank → 复测 → 疑似`，另验证授权正常账号的正式结论。只在两个方向均通过后签发生产版本。SDK/数据锁/探针请求策略变化后重新验证档位，不能把旧证据沿用到新数据。

## 2. 本地隔离验证

- 拿官方宿主对应平台安装归档，核对官方 SHA-256，解压后指定包含 `codex-proxy-rs` 和 `web/dist` 的目录。
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

1. 将项目放到自己的 GitHub 仓库默认分支，确保有 GitHub 托管的 Linux x86_64、Linux ARM64 和 macOS ARM64 runners。保持仓库变量 `CPR_ENABLE_PUBLISH` 未设置，先运行 **Verify new CPR stable releases**；可以指定 `v3.54.5`。不需要生产凭据。第一次没有旧包，应走三平台重建；开发候选只作为 run artifacts 下载。
2. 查看 `host-plan`、三平台输出、`release-decision` 与失败日志。脚本锁定正式 tag 和 commit，核对宿主二进制中的发布声明与源代码清单。旧包若覆盖目标版本并全部通过则复用；仅 `engines` 不覆盖、SDK 未变则重建清单；有兼容 SDK 变化则重编并再验。协议、必需能力、验收或摘要失败均禁止发布。兼容区间的下限是实际验证的宿主，不跨次版本声明支持。
3. 完成授权模型验证并提交证据后，维护者再设置仓库变量 `CPR_ENABLE_PUBLISH=true`。仅默认分支发布任务具备 `contents: write`。它使用未使用过的插件补丁版本，上传三个包、`checksums.txt` 和 `verification.json`；成功后才写兼容表，不覆盖任何已发布文件。`cpr-compatibility` 分支中的 `compatibility/vX.Y.Z.json` 是该宿主 tag 的验证记录，包含宿主 commit、插件版本、平台和 SHA-256。若发布后写表失败，下一次检查会先尝试复验、复用已发布包。
4. **Early warning against CPR main** 只做合同比较、编译和测试。遇到接口变更，修 `src/host.rs` 或 `src/service.rs` 的宿主接入，确认私有状态兼容；不需要修改探针或指纹策略来追宿主接口版本。

## 4. 生产切换

先查对应宿主 tag 的**已验证**记录，再核对平台摘要并从宿主管理页安装。在测试实例核对管理页和历史后，由管理员切换生产实例。保留上一组宿主和插件制品以及现有数据备份。若新宿主无法运行旧插件，按经过验证的**宿主＋插件版本对**回滚，不绕过兼容校验，也不要删除实例（删除会移除私有状态）。
