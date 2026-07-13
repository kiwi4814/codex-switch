# 中文指南

> 英文文档是 `codex-switch` 的主文档与行为依据。本页提供中文快速入口，不单独维护第二套实现说明。

`codex-switch` 用于管理本机多个 OpenAI Codex CLI 登录、查看额度，并在新会话前选择合适账号。它会操作 Codex 的文件型认证，因此请勿分享 profile、`auth.json`、Token、代理凭据或未经脱敏的 debug 输出。

## 快速开始

Codex 必须使用 file credential store。在 `$CODEX_HOME/config.toml`（通常是 `~/.codex/config.toml`）中确认：

```toml
cli_auth_credentials_store = "file"
```

macOS / Linux 安装正式版：

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash
```

Windows PowerShell 安装正式版：

```powershell
irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

添加账号并打开界面：

```bash
codex-switch login
codex-switch tui
```

无浏览器服务器使用 `codex-switch login --device`。完整中文说明见主仓库的 [README_CN.md](https://github.com/xjoker/codex-switch/blob/master/README_CN.md)。

## 参与开发版测试

开发版属于滚动 prerelease 通道。安装、验证、回退和问题反馈步骤见 [Testing development releases](Development-Releases)，其中附有中文摘要。

## 常用入口

- [开始使用](Getting-Started) — 安装、登录和首次启动
- [功能指南](Feature-Guide) — 主要工作流与安全边界
- [故障排查](Troubleshooting) — 常见错误与恢复方式
- [常见问题](FAQ) — 简短项目说明
- [命令参考](https://github.com/xjoker/codex-switch/blob/master/docs/COMMANDS.md) — 以已安装版本的 `--help` 为最终依据
- [中文 README](https://github.com/xjoker/codex-switch/blob/master/README_CN.md) — 更完整的中文使用说明

## 反馈问题

提交 Issue 时请附操作系统、终端、`codex-switch --version`、完整命令、预期结果、实际结果与最小复现步骤。分享 debug 输出前必须删除 Token、邮箱、account ID、工作区名称、可识别身份的路径和代理凭据。

[提交 GitHub Issue](https://github.com/xjoker/codex-switch/issues)
