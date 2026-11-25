# 20261125-harden-release-workflow

- 创建时间：2026-11-25T21:57:00+08:00
- 状态：implemented
- 关联任务/功能：CPR-07、CPR-08；加固自动化跟版与发版流程，并补齐 `-dev.N` 预发布预览支持。

## 背景与目标

在保持「无授权正常 OAuth 实测证据前禁止正式发布与写入 `cpr-compatibility`」前提下，消除现有 GitHub Actions 发版流程中的定时空转、无缓存冷编、缺少远端附件摘要核验及失败无法断点续跑等问题，并支持通过手动 `prerelease=true` 产出经三平台真机 smoke 校验的 `-dev.N` GitHub Prerelease 预览包。

## 分类变化

### 新增功能

- 在 [release_gate.py](../../../scripts/release_gate.py) 新增无外部副作用的 `preflight` 只读预检、`verify-resume` 断点 run 来源校验，以及 `--allow-prerelease` 预发布发布路径（输出 `publish-receipt.json`，在 Linux 构建阶段记录 `maxGlibc`）。
- 在 [follow_host.py](../../../scripts/follow_host.py) 与 [follow-host.yml](../../../.github/workflows/follow-host.yml) 新增 `dry_run`、`prerelease` 和 `resume_run_id` 手动输入与 `resume-verify` 只读复核任务；预发布使用 `X.Y.Z-dev.N`（`--prerelease --latest=false`），不占用正式版本号也不写入 `cpr-compatibility`。

### 问题修复

- [follow_host.py](../../../scripts/follow_host.py) 为 `api()` 与 `download()` 的重试循环补上 `http.client.HTTPException` 捕获，避免 `IncompleteRead` 绕过重试；补充 `download_release_asset()` 并在跨域重定向时剥离 `Authorization` 头。
- [release_gate.py](../../../scripts/release_gate.py) 在 `gh release create --draft` 与转正之间核对全部 5 份远端附件大小和 SHA-256（缺失 API 摘要时下载回算），拦截残留草稿重名，并对已发布的同提交同摘要 Release 支持幂等补写兼容表。
- [follow-host.yml](../../../.github/workflows/follow-host.yml) 为 `rebuild` 任务显式使用 `always() && needs.discover.result == 'success' && needs.decide.result == 'success'`，修复 `old` 被跳过时 GitHub Actions 传递性 `success()` 为假导致 `rebuild` 被误跳过的问题（实跑 run `45629428949` 检出）。

### 性能优化

- [follow-host.yml](../../../.github/workflows/follow-host.yml) 在 `CPR_ENABLE_PUBLISH` 未开启时跳过 `schedule` 定时触发，避免预览期每 30 分钟冷构建三平台；契约失败或无历史旧包时跳过 `old` 三平台矩阵；启用正式发布但缺档位证据时在 `decide` 提前阻断。
- [checks.yml](../../../.github/workflows/checks.yml)、[follow-host.yml](../../../.github/workflows/follow-host.yml)、[host-main.yml](../../../.github/workflows/host-main.yml) 增加 `actions/cache@v6` 缓存 Cargo 依赖/目标目录与参考数据（参考数据仍由 `detector_assets.py` 做 SHA-256 校验）。

### 测试

- [test_release.py](../../../tests/test_release.py) 扩充至 18 项离线单测，覆盖预发布、草稿远端摘要不符阻断转正、无 API 摘要下载回算、残留草稿拦截、同源已发布幂等续写兼容表、`decide` 前置短路、`verify-resume` 来源校验、`preflight` 及版本漂移检查。

### 文档

- 同步更新 [docs/release.md](../../release.md)、[README.md](../../../README.md)、[ARCHITECTURE.md](../ARCHITECTURE.md)、[FEATURES.md](../FEATURES.md) 与分类索引。

## 架构与功能影响

不修改 Rust 插件核心、前端管理页、参考数据锁或正式判定阈值；正式版发布门禁和 `cpr-compatibility` 写入条件保持不变。

## 验证与未验证项

- 离线发布闸门回归：`python3 -m unittest discover -s tests -p 'test_*.py' -v`（18 项通过）。
- 工作流静态检查：`.work/actionlint/actionlint .github/workflows/*.yml`（0 错误、0 警告）。
- 只读预检与参考数据校验：`scripts/release_gate.py preflight`（含 `--online`）与 `scripts/detector_assets.py --verify-only` 通过。
- 治理与暂存快照检查：`tidy.py sync` 与 `tidy.py check --staged` 通过（GitNexus 未启用）。
- 未验证项（待推送后在 GitHub Actions 实测）：三平台真实 runner 构建、Cargo 缓存命中表现、Linux 二进制实测 `GLIBC_*` 上限与远端 GitHub Prerelease 上传核验。

## 开发残留清理

已清理本机中断的临时容器与 `.work/linux-x86_64-*` 目录；无临时调试日志、断点或调试桩入库。
