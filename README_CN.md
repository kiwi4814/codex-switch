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

先手动创建两个宿主目录。Compose 使用了 `create_host_path: false`，宿主目录不存在时会直接报错，而不是被 Docker 悄悄创建成一个 root 所有的空目录：

```bash
mkdir -p "$HOME/.codex" "$HOME/.codex-switch"
chmod 700 "$HOME/.codex" "$HOME/.codex-switch"
```

`.env` 用命令生成，不要手填 UID/GID：

```bash
cat > .env <<EOF
PUID=$(id -u)
PGID=$(id -g)
HOST_HOME=$HOME
TZ=Asia/Shanghai
EOF
```

`PUID`、`PGID`、`HOST_HOME` 都是必填且没有默认值 — 缺失或为空时 Compose 直接失败，所以填了一半的 `.env` 不会把容器起到错误的路径上，也不会以 root 运行。

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

容器**不适合**做这些事：

| 命令 | 原因 |
| --- | --- |
| `launch` | 它要启动 Codex CLI，而 Codex CLI 按设计装在宿主机、不在这个镜像里。请先 `use` 切号，再在宿主机执行 `codex`。 |
| `self-update` | 它替换的是直装的二进制。Docker 模式请重建镜像：`git pull --ff-only && docker compose build --pull && docker compose up -d`。 |
| `daemon install` | 生命周期由 Compose 的 `restart: unless-stopped` 负责。 |
| 桌面通知（`notify = true`） | 容器内没有通知守护进程和 session bus。保持 `notify = false`，看 `docker compose logs`。 |

如果配置了 `[proxy] url`，该地址必须**从容器网络可达**。容器里的 `127.0.0.1` 指的是容器自己，所以只监听宿主 `127.0.0.1:7890` 的代理在容器内访问不到；请填宿主机的局域网地址（并在代理侧放开 LAN 访问）。不要为此添加 `network_mode: host`。

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

请在自己的宿主机上实测，不要假设它生效。数 `/proc` 下的条目数量证明不了任何事，daemon 需要的是**读到宿主 Codex 进程的命令行**。先在宿主机启动一个交互式 Codex 并保持会话：

```bash
codex
```

另开一个 shell，拿到它的 PID，然后从容器内读这个具体进程：

```bash
pgrep -af codex
PID=<codex 的 pid>
docker compose exec codex-switch sh -c "tr '\0' ' ' < /proc/$PID/cmdline; echo"
```

必须打印出 Codex 的命令行。出现 `No such file or directory` 说明宿主 PID namespace 没生效；出现 `Permission denied` 则要查 rootless Docker 或 `/proc` 的 `hidepid` 挂载选项，同时容器 UID 必须与拥有该 Codex 进程的用户一致。

### 权限

`user: "${PUID}:${PGID}"` 让容器进程以宿主普通用户身份运行，容器内生成的 `auth.json`、`config.toml`、`profiles/*` 仍归该用户所有。这两个变量是强制的 — 缺失时 Compose 拒绝启动，而不是退回 root 在家目录留下 root 所有的文件。两个 bind mount 同样使用 `create_host_path: false`，Docker 不会替你创建 `~/.codex` 和 `~/.codex-switch`，请用上面的命令自行创建。

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
