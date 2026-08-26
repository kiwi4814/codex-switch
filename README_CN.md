# codex-switch

**[OpenAI Codex CLI](https://github.com/openai/codex) 多账号管理工具** — 保存本机 Codex 登录、监控配额，并在下一次会话前选出最佳账号。

[English README](README.md) · [**完整文档（Wiki）**](https://github.com/xjoker/codex-switch/wiki) · [中文指南](https://github.com/xjoker/codex-switch/wiki/Chinese-Guide) · [Releases](https://github.com/xjoker/codex-switch/releases)

> `codex-switch` 会在本机保存账号凭据。请勿分享 profile、`auth.json`、Token、代理凭据或未脱敏的 debug 输出。

## Fork 说明

本仓库是 [xjoker/codex-switch](https://github.com/xjoker/codex-switch) 的 fork，新增：

- weekly 额度窗口自动预热检测（`daemon.weekly_auto_warmup`）；
- 5 小时窗口的定时预热（`daemon.five_hour_warmup_times`），默认本地时间 `05:00` / `10:10` / `15:20`；
- Docker Compose 部署：Codex CLI 装在宿主机，`codex-switch` daemon 跑在容器里。

**上游安装方式** — 下方的 `install.sh` / `install.ps1` / Homebrew 命令安装的是 xjoker 官方版本，**不包含**本 fork 的 scheduled warmup 改动。

**本 fork** — 推荐用 [Docker Compose 部署](#docker-compose-部署单机-ubuntu)，或从 `feature/scheduled-warmup` 分支源码构建：

```bash
git clone -b feature/scheduled-warmup https://github.com/kiwi4814/codex-switch.git
cd codex-switch
cargo build --locked --release   # 产物 target/release/codex-switch
```

## 快速开始

Codex 必须使用文件型凭据存储。如有需要，在 `$CODEX_HOME/config.toml`（通常为 `~/.codex/config.toml`）中加入下面一行；受管配置若设置了 `forced_login_method = "api"` 则不兼容：

```toml
cli_auth_credentials_store = "file"
```

安装正式版 — macOS / Linux：

```bash
curl -fsSL https://github.com/xjoker/codex-switch/releases/latest/download/install.sh | bash
```

Windows PowerShell：

```powershell
irm https://github.com/xjoker/codex-switch/releases/latest/download/install.ps1 | iex
```

Homebrew 用户：`brew install xjoker/tap/codex-switch`。

> **注意**：本项目不在 crates.io 分发——请勿 `cargo install codex-switch`，该包名属于另一个无关的同名项目。

然后添加账号并打开仪表盘：

```bash
codex-switch login        # 无浏览器服务器加 --device
codex-switch tui          # 交互式仪表盘
codex-switch use          # 自动切换到最佳账号
codex-switch launch       # 用最佳账号启动 Codex
```

![TUI](docs/tui.png)

## 功能一览

- 保存、导入、重命名、切换和可恢复地删除 Codex 账号。
- CLI 与 TUI 展示主额度池和每个模型的独立额度池。
- 自适应配速感知评分自动选号，并可直接用它启动 Codex。
- 支持重置卡、配额预热、JSON 输出、代理，以及 Beta 后台守护进程（macOS LaunchAgent / Linux systemd / Windows 任务计划程序 Task Scheduler；可调 `cache_refresh_interval_secs` 与 `auto_warmup`）。
- 自动刷新即将过期的 Token；直装版本自更新：`self-update`、`self-update --stable`、`self-update --version <VERSION>`，或用 `self-update --dev` 切换滚动开发通道 — 新装开发版使用 dev release 的 [install.sh](https://github.com/xjoker/codex-switch/releases/download/dev/install.sh) / [install.ps1](https://github.com/xjoker/codex-switch/releases/download/dev/install.ps1)。
- 直装版 `self-update` 同时校验 SHA-256 与 GitHub 构建来源，执行时会调用 `gh attestation verify`；使用前需安装当前版 [GitHub CLI](https://cli.github.com/)。
- 支持 macOS、Linux、Windows。

> **从 `0.0.x` 旧版本升级？** 本轮发布刻意做了两个破坏性变更：版本号改为日历格式（`YYYYMMDD.N.0`，一眼可读版本分配日期且仍按 SemVer 正常排序升级），macOS/Linux 安装位置从 `/usr/local/bin` 改为用户级 `$HOME/.local/bin`（`self-update` 不再需要 `sudo`）。正常 `self-update` 或重跑一次安装脚本即可迁移；账号与配置全部保留。全部破坏性变更及原因见 [Updating](https://github.com/xjoker/codex-switch/wiki/Updating)。

## Docker Compose 部署（单机 Ubuntu）

Codex CLI 仍然装在宿主机，容器里只跑 `codex-switch` daemon。两者通过 bind mount 共享宿主机真实的 `~/.codex` 与 `~/.codex-switch`，所以在容器里切号，宿主机下一次运行 `codex` 就会用上新账号。

```text
Ubuntu 宿主机
├── OpenAI Codex CLI            -> 读取 ~/.codex/auth.json
└── Docker Compose
    └── codex-switch            -> daemon start --foreground
        /data/codex             <- bind mount 自 ~/.codex
        /data/codex-switch      <- bind mount 自 ~/.codex-switch
```

### 宿主机前置条件

在宿主机安装 [OpenAI Codex CLI](https://github.com/openai/codex)，并确认 `~/.codex/config.toml` 含：

```toml
cli_auth_credentials_store = "file"
```

`codex-switch` 直接读写 `~/.codex/auth.json`；用 keychain 存储时没有这个文件可管。Docker 不负责安装或运行宿主 Codex CLI。

### 配置

```bash
cp .env.example .env
```

填入四个值 — `id -u` 得到 `PUID`，`id -g` 得到 `PGID`，`echo "$HOME"` 得到 `HOST_HOME`：

```env
PUID=1000
PGID=1000
HOST_HOME=/home/ubuntu
TZ=Asia/Shanghai
```

校验并构建：

```bash
docker compose config -q
docker compose build
```

### 账号与一次性命令

每条一次性命令都在临时容器里执行，操作的是同一份 bind mount 数据：

```bash
docker compose run --rm codex-switch login --device account-1
docker compose run --rm codex-switch login --device account-2
docker compose run --rm codex-switch list -f
docker compose run --rm -it codex-switch tui
docker compose run --rm codex-switch warmup account-1
docker compose run --rm codex-switch use account-2
```

`--device` 是设备码登录流程，无浏览器的容器必须用它。因为 `~/.codex` 是 bind mount，`use account-2` 改写的就是宿主机的 `~/.codex/auth.json`，宿主机下一次 `codex` 即使用该账号。

### 启动常驻 daemon

```bash
docker compose up -d
docker compose ps
docker compose logs -f codex-switch
docker compose down
```

容器执行 `codex-switch daemon start --foreground`，生命周期由 Compose 的 `restart: unless-stopped` 负责。Docker 模式下**不要**运行 `codex-switch daemon install`；systemd user service、launchd、`enable-linger` 属于裸机安装方式。

`docker compose down` 只删除容器，不会删除宿主机的 `~/.codex` 与 `~/.codex-switch`（它们是宿主目录）。`docker compose down --volumes` 和 `docker system prune` 不属于日常清理操作。

### 配置文件与时区

配置仍然在宿主机编辑 `~/.codex-switch/config.toml`：

```toml
[daemon]
poll_interval_secs = 60
switch_threshold = 100
cache_refresh_interval_secs = 300
auto_warmup = false
weekly_auto_warmup = true
five_hour_warmup_times = [
    "05:00",
    "10:10",
    "15:20",
]
token_check_interval_secs = 300
notify = false
log_level = "info"
defer_switch_while_codex_running = true
```

`five_hour_warmup_times` 比对的是**容器**本地时间，由 `.env` 里的 `TZ` 决定。不设 `TZ` 时容器是 UTC，这几个时刻会落在错误的挂钟时间上。

### 为什么需要 `pid: host`

`defer_switch_while_codex_running = true` 的实现是扫描进程表判断是否有交互式 Codex 会话（`src/daemon/codex_process.rs` 在 Linux 上读 `/proc/*/cmdline`）。Codex CLI 跑在宿主机、daemon 跑在容器里，默认 PID namespace 下 daemon 看不到任何 Codex 进程，就会在会话进行中替换 `auth.json`。`pid: host` 就是为了让这个检测继续有效。

代价：容器能看到宿主机完整进程列表，包括其他用户的命令行。这是本方案唯一使用的宿主机集成 — 没有 `privileged: true`、没有挂载 Docker socket、没有 host networking、也不发布任何端口（`codex-switch` 不提供网络服务）。

### 权限

`user: "${PUID}:${PGID}"` 让容器进程以宿主普通用户身份运行，容器内生成的 `auth.json`、`config.toml`、`profiles/*` 仍归该用户所有。`PUID`/`PGID` 留空会以 root 运行，在家目录留下 root 所有的文件。

### 升级与备份

```bash
git fetch
git pull --ff-only
docker compose build --pull
docker compose up -d
```

`~/.codex` 与 `~/.codex-switch` 是宿主持久化目录，不随镜像重建删除。升级前建议备份：

```bash
cp -a ~/.codex-switch/config.toml ~/.codex-switch/config.toml.bak
cp -a ~/.codex-switch/profiles ~/.codex-switch/profiles.bak
```

## 文档

**[GitHub Wiki](https://github.com/xjoker/codex-switch/wiki)** 是完整文档：开始使用、功能指南、命令参考、配置、更新与通道、故障排查、FAQ 以及贡献者指南。中文读者从 [中文指南](https://github.com/xjoker/codex-switch/wiki/Chinese-Guide) 开始；行为细节以英文页面为准。

维护者文档：[发布流程](docs/RELEASE.md) · [更新日志](docs/CHANGELOG.md) · [贡献指南](CONTRIBUTING.md)。

## 许可证

[MIT](LICENSE)
