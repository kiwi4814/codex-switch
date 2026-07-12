# Release 流程

日常质量门由 `.github/workflows/ci.yml` 驱动：`dev` 分支 push，以及目标为 `dev` / `master` 的 pull request，都会在 Linux、macOS、Windows 上执行测试、Clippy 和构建；Linux 质量 job 另外执行 fmt、`cargo audit` 与安装脚本语法检查。发布构建由 `.github/workflows/release.yml` 驱动，只监听 tag 事件 `v*` 与 `dev`。

本文面向项目维护者。普通用户请使用 README 中的安装与更新命令，不需要操作 Git tag。

## 发布资格

开始推送前必须同时满足：

- 当前分支为本地 `dev`，工作树干净，且本次变更已提交
- `Cargo.toml` 是目标基础版本，`docs/CHANGELOG.md` 顶部存在对应的 Unreleased 条目
- 独立代码审查无 CRITICAL/HIGH，认证、更新或用户数据变更还需通过安全审查
- 本地质量门和真实 CLI 冒烟通过
- 已明确获得 `git push` 授权，并记录待推送 commit

`dev` 发布分两道门：先推分支并等待三平台 CI 全绿，再移动 `dev` tag 触发 Release。分支 CI 未通过时禁止移动 tag。

## 版本号策略

基础版本采用兼容 SemVer 的 `YYYYMMDD.V.0`：

- `YYYYMMDD` 是发布日期，例如 2026-07-12 为 `20260712`
- `V` 是当天发布序号，从 `1` 开始；同日第二版为 `20260712.2.0`
- 最后一段固定为 `0`，因为 Cargo/SemVer 要求 `major.minor.patch` 三段；不要使用无效的两段式 `20260712.1`
- 日期必须按 `YYYYMMDD` 排列，不能使用会破坏时间排序的 `YYYYDDMM`
- 从旧的 `0.0.x` 迁移是正常升级；迁移后不要再发布更小的 `0.x` 版本，否则 self-update 会将其视为降级

| 推送的 tag | CI 输出版本号 | GitHub Release 名 | self-update 通道 | 触发 homebrew |
|---|---|---|---|---|
| `dev`（rolling，每次覆盖） | `YYYYMMDD.V.0-dev.<UTC时间戳>` | `dev` | `--dev` | 否 |
| `vYYYYMMDD.V.0-<suffix>`（永久 prerelease） | `YYYYMMDD.V.0-<suffix>` | 同 tag | 拿不到（客户端硬编码 tag=`dev`） | 否 |
| `vYYYYMMDD.V.0`（stable） | `YYYYMMDD.V.0` | 同 tag | 默认通道 | 是 |

> Cargo.toml `version` 字段不带 `-dev` 后缀，由 CI 在 inject 步骤统一加。
>
> 客户端 `src/update.rs` 的 `--dev` 通道 `fetch_release(Some("dev"))` 是写死的 tag=`dev`，所以"独立 prerelease tag"对 self-update 不可见。

## ⚠ `dev` 是分支也是 tag（refname 歧义）

本仓库 `dev` 同时是开发分支与 rolling tag。**所有 push/delete/lookup 必须使用完整 refspec**，否则报：

```
error: src refspec dev matches more than one
```

或意外推到错误的目标。

## 发布 dev 测试版（标准流程）

前置：`dev` 分支已合入待发布的所有 commit；本地工作树干净。

```bash
# 1) 跑本地质量门（本地预检，不作为发布产物依据）
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo audit
bash -n scripts/install.sh

# 2) 推送 dev 分支到远端（必须完整 refspec）
git push origin refs/heads/dev:refs/heads/dev

# 3) 等待 dev 分支 CI 全绿，并确认远端分支指向本次 commit
gh run list --branch dev --workflow CI --limit 1
git rev-parse refs/remotes/origin/dev

# 4) 删除远端旧 dev tag（指向旧 commit，需要先删才能"移动"）
git push origin :refs/tags/dev

# 5) 在本地把 dev tag 重打到 HEAD
git tag -d dev && git tag dev

# 6) 推送新 dev tag —— 触发 CI 构建 6 平台二进制 + 覆盖 GitHub Release `dev`
git push origin refs/tags/dev:refs/tags/dev
```

> 第 6 步**不能写 `git push origin dev`**：歧义错误（分支 + tag 同名）。必须用 `refs/tags/dev:refs/tags/dev`。
>
> 第 2 步同理：必须 `refs/heads/dev:refs/heads/dev`。

发布产物以 GitHub Actions `Release` workflow 构建为准，不用本地 `target/release` 作为发布依据。Release job 会先逐个验证归档对应的 `.sha256`，校验失败不会创建 GitHub Release。CI 完成后产物：
- Linux / macOS：`cs-{linux,darwin}-{amd64,arm64}.tar.gz` + `.sha256`
- Windows：`cs-windows-{amd64,arm64}.zip` + `.sha256`
- `install.sh` / `install.ps1`
- 用户侧：`codex-switch self-update --dev` 立即可拉取

发布后复测至少确认：
- GitHub Actions `Release` run 成功，6 平台 build 和 release job 通过
- 从 GitHub Release 下载对应平台 `.tar.gz` 或 `.zip` 与 `.sha256`，校验 SHA256
- 解包后的 release 产物 `codex-switch --version` 输出 CI 注入版本
- 原触发路径可用，例如 `codex-switch self-update --check --dev`

## 发布 stable

```bash
# 1) 在 dev 分支充分验证后，合并到 master
git checkout master && git merge --ff-only dev && git push origin master

# 2) 在 master 上打版本 tag（示例：2026-07-12 当天首版）
git tag v20260712.1.0
git push origin refs/tags/v20260712.1.0:refs/tags/v20260712.1.0

# 3) CI 会自动：构建 6 平台 + 创建对应 GitHub Release + 触发 homebrew job
```

发布前别忘了：
- 先运行 `date` 获取本机真实日期，再把 `Cargo.toml` bump 到当天的 `YYYYMMDD.V.0`
- `docs/CHANGELOG.md` 顶部新增对应的 `## vYYYYMMDD.V.0 — YYYY-MM-DD` 段

## 排错

**`error: src refspec dev matches more than one`**
完整 refspec：`refs/heads/dev:refs/heads/dev`（分支） / `refs/tags/dev:refs/tags/dev`（tag）。

**dev tag 推上去但 CI 没跑**
检查 GitHub Actions 页面有没有 `Release` workflow 触发；workflow 文件 `on.push.tags` 是否仍包含 `"dev"`。

**`self-update --dev` 取不到新版**
GitHub Release 名必须是字面 `dev`（小写）。如果误推成 `v0.0.15-dev` 这类带 `v` 前缀的独立 tag，会创建独立 prerelease，客户端通道看不到。

**Cargo.toml 版本号该带 `-dev` 吗？**
不带。CI 的 dev 路径会自动追加 `-dev.<timestamp>`，本地 `Cargo.toml` 始终保持 `YYYYMMDD.V.0` 干净版本号。
