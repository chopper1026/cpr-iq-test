# 2026-11-26 发布脚本逐件上传与瞬时 5xx 有界重试

## 背景

在触发 `v0.1.0-dev.2` 预发布流程期间，GitHub Actions / Releases / Uploads 出现服务降级（`HTTP 500` 及并发上传草稿附件时的 `HTTP 502: Error updating policy`）。旧版 `scripts/release_gate.py publish` 通过单次 `gh release create <tag> <5 files>` 并发上传 5 份附件且无重试，导致单个瞬时错误即中止发布并遗留不完整草稿。

## 变更

- `scripts/release_gate.py`：
  - 将草稿创建与附件上传拆分为「创建空草稿 → 按顺序逐件 `gh release upload --clobber` → 远端校验全部 5 份附件大小与 SHA-256 → `gh release edit` 转正」。
  - 新增 `run_gh_retry` 与瞬时错误判定，对 `HTTP 5xx/429`、连接重置及上传策略错误执行有界退避重试；草稿创建与转正重试前通过 `check_done` 复核远端状态，避免重复创建草稿或重复转正；对 `git_write` 同步增加瞬时 5xx 有界重试。
- `scripts/follow_host.py`：将只读 `api()` 瞬时 5xx/429 重试上限由 3 次提升至 5 次。
- `tests/test_release.py`：更新发布顺序断言，新增瞬时 `HTTP 500/502` 退避重试、幂等 `check_done` 防重与非瞬时 4xx 立即失败用例。
- `docs/release.md`：同步更新第 2 节草稿顺序上传与瞬时重试说明。

## 边界与验证

- 不改变任何门禁安全边界：依然先校验全部 5 份远端附件 SHA-256 才转正，仍拒绝残留草稿与覆盖已有不同提交的发布。
- 验证：离线单测 `tests/test_release.py` 全量通过，`tidy.py check --staged` 通过；随后推送并在 GitHub Actions 实跑三平台构建与 `v0.1.0-dev.2` 预发布验证。
