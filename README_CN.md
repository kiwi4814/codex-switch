# codex-switch

**[OpenAI Codex CLI](https://github.com/openai/codex) 多账号管理工具** — 无限账号管理、实时配额监控、智能自动切换。

[**English Documentation →**](README.md)

> 当前正式版：`v0.0.21`。

## 两分钟快速上手

开始前需要安装 [Codex CLI](https://github.com/openai/codex)，并准备一个可以登录 Codex 的 ChatGPT 账号。`codex-switch` 使用 Codex 的文件型 `auth.json`；如果 Codex 配置选择了不兼容的凭据存储，程序会停止并给出修复说明，不会擅自修改认证状态。

安装正式版：

**macOS / Linux**

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash
```

**Windows PowerShell**

```powershell
irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

然后添加第一个账号并打开仪表盘：

```bash
codex-switch login
codex-switch tui
```

偏好普通命令行？运行 `codex-switch list` 查看账号，运行不带别名的 `codex-switch use` 自动选择当前最佳账号。

> `codex-switch` 会在本机保存账号凭据。请勿分享 profile 文件或未经脱敏的 `--debug` 输出。

---

### TUI

![TUI](docs/tui.png)

### CLI

![CLI](docs/cli.png)

## 功能特性

- **账号管理** — 保存、切换、重命名、删除 Codex 账号
- **自动探测** — 自动发现并追踪当前 `auth.json`
- **用量仪表盘** — 实时监控配额（5 小时和 7 天窗口），包含每个账号自己的刷新时间；拥有按模型配额池的账号（如 Pro 20×）会以缩进子行展示每个池，TUI 详情面板还会列出账号可用的模型列表
- **重置卡（v0.0.20）** — 展示 Codex reset card 数量和过期时间，并可在 CLI 或 TUI 中确认后消耗最早过期的可用重置卡
- **自适应智能切换** — `codex-switch use` 不带参数时通过统一的 5 组件自适应评分算法自动选择最优账号，Team 账号默认优先
- **后台守护进程（Beta）** — 可选的 `daemon` 命令在 macOS 使用 LaunchAgent、Linux 使用 systemd 用户服务、Windows 使用任务计划程序（Task Scheduler）
- **仅刷新过期账号** — `use`、`list` 和 TUI 默认只刷新缓存已过期的账号
- **进度展示** — 大批量 `use`、`list`、目录 `import` 统一显示单行跨平台进度条
- **交互式 TUI** — 完整的终端界面，实时用量数据、颜色状态、键盘快捷键
- **OAuth 登录** — 内置 PKCE 浏览器登录流程，无需手动复制 token
- **Token 自动刷新** — 使用 refresh_token 自动刷新过期 token
- **批量导入校验** — 支持单文件导入，也支持递归扫描目录、分阶段校验并自动分配不重复别名
- **配速标记** — 用量条上显示基于窗口已过时间的预期消耗位置，直观判断用量快慢
- **预热** — `warmup` 发送最小请求以启动配额窗口倒计时，已激活的账号自动跳过
- **手动自更新** — `self-update --check` 按需检查 GitHub Releases，`self-update` 更新直装版本（支持 stable 和 dev 双渠道）
- **启动 Codex** — `launch` 使用指定（或最佳）账号的认证启动 Codex CLI，透传所有参数。认证仅在启动时短暂替换，codex 读取后立即还原，不阻塞其他操作
- **超速预警** — 当用量超过预期配速时，5h/7d 列显示红色 `!` 标记
- **代理支持** — HTTP/HTTPS/SOCKS4/SOCKS5/SOCKS5H，支持鉴权
- **跨平台** — macOS、Linux、Windows（全 RGB 调色板确保 TUI 渲染一致）
- **JSON 输出** — `--json` 参数支持脚本化和自动化

## 更多安装方式

大多数用户使用上面的快速安装即可。以下内容适用于包管理器安装、开发版测试、手动下载和源码编译。

### Homebrew（macOS / Linux）

```bash
brew install xjoker/tap/codex-switch
```

### 安装开发版（最新开发构建）

开发版可能不稳定，仅建议用于在下一个正式版发布前参与测试。

**macOS / Linux：**

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash -s -- --dev
```

**Windows（PowerShell）：**

```powershell
$env:CS_DEV="1"; irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

### 卸载

**macOS / Linux：**

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash -s -- --uninstall
```

**Windows（PowerShell）：**

```powershell
$env:CS_UNINSTALL="1"; irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

### 手动下载

从 [Releases](https://github.com/xjoker/codex-switch/releases) 下载对应平台的预编译二进制：

| 平台 | 架构 | 文件 |
|------|------|------|
| macOS | Apple Silicon (M1/M2/M3) | `cs-darwin-arm64.tar.gz` |
| macOS | Intel | `cs-darwin-amd64.tar.gz` |
| Linux | x86_64 | `cs-linux-amd64.tar.gz` |
| Linux | ARM64 | `cs-linux-arm64.tar.gz` |
| Windows | x86_64 | `cs-windows-amd64.zip` |
| Windows | ARM64 | `cs-windows-arm64.zip` |

安装脚本会下载匹配的 `.sha256` 文件，并在解压归档前完成校验。

### 从源码编译

需要 [Rust](https://rustup.rs/) 1.88+：

```bash
git clone https://github.com/xjoker/codex-switch.git
cd codex-switch
cargo build --release
sudo cp target/release/codex-switch /usr/local/bin/  # macOS/Linux
```

## 常用任务

| 目标 | 命令 |
|------|------|
| 添加账号 | `codex-switch login` |
| 在无浏览器服务器上添加账号 | `codex-switch login --device` |
| 查看账号和实时配额 | `codex-switch list` |
| 打开交互式仪表盘 | `codex-switch tui` |
| 切换到指定账号 | `codex-switch use <别名>` |
| 自动选择最佳可用账号 | `codex-switch use` |
| 使用最佳账号启动 Codex | `codex-switch launch` |
| 导入已有认证文件 | `codex-switch import <路径>` |
| 检查更新 | `codex-switch self-update --check` |

### 认证与存储要求

`codex-switch` 通过切换 Codex 的文件型 `auth.json` 工作。因此 Codex 必须使用默认的 file credential store，或在 `$CODEX_HOME/config.toml`（通常为 `~/.codex/config.toml`）中显式配置 `cli_auth_credentials_store = "file"`。显式的 `keyring`、`auto` 或 `ephemeral` 模式可能绕过实时认证文件，程序会直接拒绝。空的 `CODEX_HOME` 会回退到 `~/.codex`；非空值同时决定 `auth.json` 与 `config.toml` 的 Codex 主目录。

`CODEX_SWITCH_HOME` 可将 codex-switch 自身的 profiles、cache、锁与 daemon 状态从 `~/.codex-switch` 迁到其他目录；它不会改变 Codex `auth.json` 的位置。

本工具要求 ChatGPT 登录。如果受管 Codex 配置设置了 `forced_login_method = "api"`，程序会给出可操作错误并停止，不会修改认证状态。

将 `<别名>` 替换为 `work`、`personal` 等自定义名称。运行 `codex-switch <命令> --help` 可以查看该命令的选项和示例。

## 命令列表

| 命令 | 说明 |
|------|------|
| `codex-switch use [别名] [--consume-card]` | 切换账号。不带别名则用自适应评分算法自动选择最优账号；若账号池全部耗尽，`--consume-card`（或交互式 y/N 确认）会消耗最早过期的 reset card 复活一个账号，而不是直接落回耗尽账号（带别名时该选项被忽略） |
| `codex-switch list [-f]` | 显示所有账号信息、用量和可用状态（`-f` 强制刷新，忽略缓存） |
| `codex-switch reset-card <别名> [--yes]` | 消耗该账号最早过期的可用 Codex reset card。默认会先确认；JSON 模式需要 `--yes` |
| `codex-switch launch [别名] [--consume-card] [-- 参数...]` | 用指定账号的认证启动 Codex CLI。不带别名则自适应评分自动选择，`--consume-card` 行为与 `use` 相同。`--` 后的参数透传给 codex |
| `codex-switch warmup [别名]` | 发送最小请求以触发 5h/7d 配额窗口倒计时。不带别名则预热所有账号 |
| `codex-switch login [--device] [别名]` | OAuth 登录（`--device` 用于无浏览器的服务器）。若别名已存在则重新授权 |
| `codex-switch rename <旧别名> <新别名>` | 重命名账号 |
| `codex-switch delete <别名> [--yes]` | 从账号列表移除非激活 profile，并归档以便恢复；默认会先确认 |
| `codex-switch import <路径> [别名]` | 导入单个 auth.json，或递归扫描目录下所有 JSON 文件并校验后导入 |
| `codex-switch daemon start [--foreground]` | 启动自动切换守护进程（Beta）。默认后台运行；`--foreground` 用于服务管理器 |
| `codex-switch daemon stop` | 停止运行的 Beta 守护进程 |
| `codex-switch daemon status` | 显示 Beta 守护进程状态和平台支持信息 |
| `codex-switch daemon install` | 安装 Beta 守护进程（macOS LaunchAgent / Linux systemd 用户服务 / Windows 任务计划程序；Windows 需以管理员 PowerShell 执行） |
| `codex-switch daemon uninstall` | 卸载 Beta 守护进程系统服务 |
| `codex-switch self-update [--check] [--dev\|--stable] [--version <VERSION>]` | 检查或更新直装版本；不带通道参数时保持当前 stable/dev 通道，`--version` 安装指定正式版本 |
| `codex-switch tui` | 启动交互式终端界面 |
| `codex-switch open` | 在文件管理器中打开配置目录 |

### 全局选项

| 选项 | 说明 |
|------|------|
| `--json` | 以紧凑 JSON 格式输出（适合脚本/管道） |
| `--json-pretty` | 以格式化 JSON 输出 |
| `--proxy <URL>` | 设置代理（参见[代理支持](#代理支持)） |
| `--color <auto\|always\|never>` | 颜色输出模式（默认: auto） |
| `--debug` | 开启调试日志（显示 HTTP 请求、API 响应、缓存状态） |
| `-V, --version` | 打印版本号 |

## TUI 快捷键

按 `Enter` 打开选中账号的操作菜单；如果已有账号被标记，则打开批量操作菜单。

| 按键 | 操作 |
|------|------|
| `j` / `k` 或 `↑` / `↓` | 导航 |
| `Enter` | 打开账号或批量操作菜单 |
| `/` | 搜索/过滤账号 |
| `r` | 刷新可见账号 |
| `a` | 添加新账号 |
| `t` | 开关自动刷新 |
| `W` | 开关自动预热 5h 窗口已过期的账号 |
| `i` | 显示/隐藏账号详情面板 |
| `s` | 切换排序（名称/配额/状态） |
| `Space` | 标记/取消标记账号 |
| `u`（账号菜单） | 切换到选中账号 |
| `c`（账号菜单） | 确认并消耗最早过期的重置卡 |
| `w`（账号菜单） | 预热选中账号 |
| `l`（账号菜单） | 重新登录选中账号 |
| `n`（账号菜单） | 重命名选中账号 |
| `d`（账号菜单） | 删除选中账号（需确认） |
| `r` / `w` / `l` / `d`（批量菜单） | 刷新、预热、重新登录或删除已标记账号 |
| `h` | 显示帮助 |
| `Esc` | 清除搜索/标记，或关闭当前弹窗 |
| `q` | 退出 |

## 更新方式

除 TUI 启动外，更新检查都需要手动触发。`codex-switch tui` 启动时检查一次；普通启动、`list` 和 `use` 不会自动检查更新。

```bash
# 检查是否有新版本
codex-switch self-update --check

# 将直装版本更新到最新 release
codex-switch self-update

# 显式切换通道
codex-switch self-update --dev
codex-switch self-update --stable

# 安装指定正式版本（不支持降级）
codex-switch self-update --version 0.0.22
```

- Homebrew 安装不会被程序自行覆盖，请使用 `brew upgrade xjoker/tap/codex-switch`
- 直装版本会先校验 release 对应的 `.sha256`，再替换当前二进制。校验和与二进制同属一个 GitHub Release，因此只防下载损坏、不防 Release 本身被篡改；信任锚是 TLS 之上的 GitHub Releases，目前没有独立代码签名
- 不带参数的 `self-update` 会保持当前二进制所属的通道；使用 `--dev` 或 `--stable` 显式切换通道
- Homebrew 用户需先 `brew uninstall codex-switch` 才能使用 `--dev`

## 代理支持

代理优先级（从高到低）：

1. `--proxy` 命令行参数
2. `CS_PROXY` 环境变量
3. 配置文件 `~/.codex-switch/config.toml`
4. 标准环境变量（`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`）

### 支持的协议

| 协议 | DNS 解析 | 鉴权 |
|------|----------|------|
| `http://[user:pass@]host:port` | 本地 | 支持 |
| `https://[user:pass@]host:port` | 本地 | 支持 |
| `socks4://host:port` | 本地 | 不支持 |
| `socks5://[user:pass@]host:port` | 本地 | 支持 |
| `socks5h://[user:pass@]host:port` | 远程（代理端解析） | 支持 |

### 配置文件

`~/.codex-switch/config.toml`：

```toml
[proxy]
url = "socks5h://user:pass@127.0.0.1:1080"
no_proxy = "localhost,127.0.0.1"

[cache]
ttl = 300  # 缓存有效期（秒，默认 300）

[network]
max_concurrent = 20  # 最大并发请求数（默认 20）

[tui]
auto_refresh_interval_secs = 120  # 自动刷新间隔（秒，默认：120，最小：30）

[use]
safety_margin_7d = 20       # 7d 安全线：低于此剩余百分比开始惩罚（默认：20）
team_priority = true        # 优先使用 Team 账号，+500 层级加成（默认：true）

[daemon]
poll_interval_secs = 60         # 用量轮询间隔（秒，默认：60）
switch_threshold = 80           # 触发切换的 5h 用量百分比（默认：80）
cache_refresh_interval_secs = 300 # 刷新全部已保存账号缓存（秒，默认：300）
auto_warmup = false             # 刷新缓存时预热未激活窗口（默认：false）
token_check_interval_secs = 300 # Token 刷新检查间隔（秒，默认：300）
notify = false                  # 切换时桌面通知（macOS/Linux/Windows，默认：false）
log_level = "error"             # 守护进程日志级别（默认："error"）
defer_switch_while_codex_running = true # Codex 交互会话运行中时挂起切换（默认：true）

[launch]
restore_delay_secs = 3          # codex 启动后多少秒还原 auth.json（默认：3）
```

三个 daemon 间隔字段若设为 `0`，会按“未设置”处理并归一化为文档默认值：轮询 `60` 秒、缓存刷新 `300` 秒、Token 检查 `300` 秒。

`launch.restore_delay_secs` 是兼容性延迟，并非与 Codex 的握手。如果本机 Codex 在启动三秒后才读取 `auth.json`，请增大该值。

### 示例

```bash
# 命令行参数
codex-switch --proxy socks5h://127.0.0.1:1080 list

# 环境变量
export CS_PROXY="http://user:pass@proxy.corp.com:8080"
codex-switch list

# 标准环境变量（reqwest 自动读取）
export HTTPS_PROXY="http://proxy.corp.com:8080"
codex-switch list
```

## 常见使用场景

### 每次启动 Codex 前自动切换

```bash
# 加入 shell 配置文件（.zshrc / .bashrc）：
codex-switch use && codex
```

### 使用守护进程保持下一次会话就绪（Beta）

当你希望 `codex-switch` 持续监控当前账号并在后台准备好下一次 Codex 启动时，可使用 Beta 守护进程。0.0.22 实现在 macOS 安装 LaunchAgent、在 Linux 安装 systemd 用户服务、在 Windows 安装登录时触发的任务计划程序（Task Scheduler）任务。

```bash
# 启动后台守护进程
codex-switch daemon start

# 检查是否在运行
codex-switch daemon status

# 停止守护进程
codex-switch daemon stop

# 安装/卸载守护进程服务
# Windows：以下两条命令需在管理员 PowerShell 中执行。
codex-switch daemon install
codex-switch daemon uninstall
```

Beta 守护进程使用与 `codex-switch use` 相同的自适应评分逻辑。它在每次轮询时刷新当前账号，仅在 `daemon.switch_threshold` 达到或超过阈值且存在更好的候选账号时才切换；按 `daemon.cache_refresh_interval_secs` 刷新所有已保存账号的缓存，并在独立定时器上刷新即将过期的 Token。`daemon.auto_warmup = true` 还会预热未激活的配额窗口，默认关闭。daemon 切换无法交互确认：未跟踪的实时 `auth.json` 会在正常轮转备份后被替换；需要保留为可直接选择的账号时，应先保存或导入。守护进程为未来的 Codex 启动做准备；已运行的 Codex 进程在切换后仍需重启。

当交互式 Codex 会话（`codex`、`codex resume`、`codex exec`）正在运行时，daemon 会把切换挂起为 pending 并在下次轮询重试；MCP server、`app-server` 等常驻 Codex 基础设施不会阻塞切换。设置 `daemon.defer_switch_while_codex_running = false` 可无视会话立即切换。daemon 会把状态快照写入 `~/.codex-switch/daemon-state.json`（上次轮询/上次切换/挂起切换/最近错误），`codex-switch daemon status` 会展示；日志写入 `~/.codex-switch/logs/`，按天轮转、最多保留 7 个文件。

### 定时刷新 Token（可选）

通过 cron 定时刷新缓存和 Token，让 `codex-switch use` 即时响应：

```bash
# 编辑 crontab
crontab -e

# 每 5 分钟刷新所有账号用量
*/5 * * * * /usr/local/bin/codex-switch list --json > /dev/null 2>&1
```

此任务会定期执行 `codex-switch list`，刷新过期 Token 并缓存用量数据。**不会**自动切换账号。

### CI / 自动化场景

```bash
# 选择最佳账号，并把参数直接传给 Codex
codex-switch launch -- --model gpt-5.4
```

## 故障排查

优先按照错误信息操作：配置、登录和权限错误都会给出具体路径或下一条命令。

| 现象 | 处理方式 |
|------|----------|
| 没有已保存账号 | 运行 `codex-switch login` 或 `codex-switch import <路径>` |
| 凭据存储不是 file 模式 | 在 `$CODEX_HOME/config.toml` 设置 `cli_auth_credentials_store = "file"` |
| Windows 安装 daemon 提示拒绝访问 | 以管理员身份打开 PowerShell 后重试 |
| Git Bash 中 TUI 布局异常 | 改用 Windows Terminal 或 PowerShell |
| 误删了 profile | 参见[恢复已删除的 profile](#恢复已删除的-profile) |

遇到网络或 API 故障时，使用 `--debug` 重跑命令：

```bash
codex-switch --debug list
codex-switch --debug use
```

如果问题持续存在，请附上命令、操作系统、版本和脱敏后的 debug 输出[提交 Issue](https://github.com/xjoker/codex-switch/issues)。必须移除 Token、邮箱、account ID、工作区名称和代理凭据。

### 恢复已删除的 profile

删除操作可恢复：profile 目录会从 `profiles/` 移到 `deleted-profiles/`，不会直接擦除。先停止 daemon，将对应别名最新的备份目录移回，再确认账号出现：

```bash
codex-switch daemon stop
# 将 ~/.codex-switch/deleted-profiles/<别名>.backup-<时间戳>
# 移回 ~/.codex-switch/profiles/<别名>
codex-switch list
```

Windows 的对应目录位于 `%USERPROFILE%\.codex-switch`。如果设置了 `CODEX_SWITCH_HOME`，请改用该目录。

## 工作原理

### 文件位置

| 路径 | 说明 |
|------|------|
| `~/.codex/auth.json` | Codex CLI 认证文件（或 `$CODEX_HOME/auth.json`） |
| `~/.codex-switch/profiles/<别名>/auth.json` | 保存的账号数据 |
| `~/.codex-switch/deleted-profiles/<别名>.backup-<时间戳>/` | 可恢复的已删除 profile |
| `~/.codex-switch/current` | 当前激活的账号名 |
| `~/.codex-switch/auth.lock` | 文件锁（序列化 auth.json 切换操作） |
| `~/.codex-switch/config.toml` | 配置文件 |

### 自动探测

每次交互式启动时，codex-switch 会将当前 `~/.codex/auth.json` 与所有已保存的 profile 进行比对：

- **检测到新账号**（例如你运行了 `codex login`）— 提示保存为新 profile
- **已有账号的 Token 已刷新** — 提示更新对应 profile
- **非交互式环境**（管道、cron、CI）— 报告变更但不会静默修改状态

运行 `codex-switch list` 或 `codex-switch tui` 时，工具还会检查当前 `auth.json` 是否属于未追踪的账号，并自动保存为新 profile（使用邮箱用户名作为别名）。

### 去重机制

登录或导入时，工具通过 `account_id`（优先）或 `email`（备选）匹配账号。如果同一账号已以不同别名存在，会更新已有 profile 而非创建重复项。

### 导入校验

`codex-switch import` 会按阶段验证每个候选文件：

1. 文件格式 — 必须是合法 JSON
2. 结构校验 — 必须包含所需 `tokens` 字段，并且 `id_token` 可解码
3. 用量校验 — 调用 token 刷新和 usage 接口确认账号可用（测试可显式跳过）
4. 保存阶段 — 按身份去重，必要时自动分配不冲突别名

如果输入路径是目录，命令会递归扫描所有 `.json` 文件，并分别报告导入成功与跳过原因。

### 智能切换（`codex-switch use`）

不带别名调用时，`codex-switch use` 会先复用仍然新鲜的缓存，再只刷新过期账号，对每个账号使用统一的自适应算法评分。

算法采用**两阶段**方式：
1. **准入检查** — 过滤已耗尽、7d 配额严重不足（且重置遥远）或低于 Free 计划安全底线的账号。如果**所有**账号都不达标，则从中选最优的作为兜底。
2. **自适应评分** — 对通过准入的账号使用五个组件进行排名：

```text
score = tier_bonus + headroom + sustain + drain_value + recency
```

- `tier_bonus`（0 或 +500）— `team_priority = true` 时 Team 账号默认获得优先。这是优先级而非保证：已耗尽或不安全的 Team 账号仍可能落败或被过滤。
- `headroom`（0..1100）— 基于燃烧速率和重置时间的 5h 配速感知剩余容量，而非静态剩余百分比。
- `sustain`（-800..0）— 7d 每窗口预算安全惩罚。
- `drain_value`（0..300）— 对 60 分钟内即将重置的配额给予加分；权重根据池大小和耗尽比率自适应调整。
- `recency`（-60..0）— 轻微分散惩罚，避免反复使用同一账号。

v0.0.13+ 不再有模式选择。此统一算法替代了之前的 `max-remaining`、`drain-first` 和 `round-robin`。

> **注意：** 切换账号后，需要**重启 Codex** 才能加载新的 `auth.json`。Codex CLI 仅在启动时读取认证文件，不会监听文件变化。

#### 准入门槛

以下情况账号被标记为**不合格**：
- 5h 窗口已完全耗尽（>=100%）
- 7d 窗口已完全耗尽（>=100%）
- 7d 剩余低于临界阈值（`safety_margin_7d` 的 25%，最低 1%）且 7d 重置超过 48 小时
- Free 计划账号已低于内置的 5h 安全底线

不合格账号会被排除，除非所有账号都不合格，此时选择得分最高的作为最后手段。

### 配置选项

`[use]` 现在只有两个选项：

- `safety_margin_7d` — 7d 安全线，用于 sustain 组件和准入门槛
- `team_priority` — 默认 `true`；为 Team 账号提供 +500 层级加成

旧版 `mode` 和 `min_remaining` 在 v0.0.13+ 中被忽略并输出警告。

### 缓存行为

- 用量缓存按 profile alias 单独存储在 `~/.codex-switch/cache.json`
- 每条缓存都带自己的刷新时间，JSON 输出会通过 `usage.fetched_at` 暴露出来
- `list`、`use`、TUI 默认只刷新过期账号
- `list -f` 和 TUI 中的 `r` 会强制所有账号绕过缓存
- 目录导入会逐个文件验证，并显示整体进度

### Token 自动刷新

当用量查询返回 HTTP 401/403 时，工具自动尝试使用存储的 `refresh_token` 刷新 token。刷新成功后，新 token 会写回 profile 文件和当前的 auth.json。

### 安全说明

- CLI 和 TUI 都不允许删除当前激活账号
- 删除非激活账号前必须确认，随后会移动到私有的可恢复目录
- JSON 模式保证 stdout 只输出机器可读内容，进度和人类提示会走 stderr

## 平台说明

### macOS

- 默认 Codex 认证路径：`~/.codex/auth.json`
- 浏览器通过系统 `open` 命令打开
- 文件管理器通过 `open` 打开

### Linux

- 默认 Codex 认证路径：`~/.codex/auth.json`
- 浏览器通过 `xdg-open` 打开（确保已配置桌面浏览器）
- 文件管理器通过 `xdg-open` 打开
- WSL：浏览器打开可能需要 `wslu` 包（`sudo apt install wslu`）
- **无浏览器的服务器环境：** 使用 `codex-switch login --device` 进行设备码登录 — 会显示一个 URL 和验证码，在任何有浏览器的设备上完成授权即可

### Windows

- 默认 Codex 认证路径：`%USERPROFILE%\.codex\auth.json`
- 浏览器通过 `rundll32.exe url.dll,FileProtocolHandler` 打开
- 文件管理器通过 `explorer.exe` 打开
- 终端：支持 Windows Terminal、PowerShell 和 cmd.exe
- TUI 通过 `crossterm` 使用 Windows Console API 渲染
- `daemon install` 使用登录时触发的 Windows 任务计划程序，需以管理员 PowerShell 执行；可用 `daemon status` 检查安装和运行状态
- **推荐终端：[Windows Terminal](https://aka.ms/terminal)。** Git Bash（mintty）与 TUI 渲染存在已知兼容性问题，请使用 Windows Terminal 或 PowerShell

## JSON 输出

大多数命令都支持 `--json` 机器可读输出（`tui` 和 `open` 除外）：

```bash
# 以 JSON 列出所有账号
codex-switch --json list

# 切换账号并返回结果
codex-switch --json use alice

# JSON 模式检查更新
codex-switch --json self-update --check
```

## 编译

```bash
# Debug 构建
cargo build

# Release 构建（优化并去除符号）
cargo build --release

# 从 macOS 交叉编译 Linux
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# 从 macOS/Linux 交叉编译 Windows
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## 发布

仅维护者使用。完整流程见 [docs/RELEASE.md](docs/RELEASE.md)（dev 滚动 tag、stable tag、refspec 踩坑）。

## 更新日志

每个版本的详细变更记录请参见 [docs/CHANGELOG.md](docs/CHANGELOG.md)。

## 许可证

[MIT](LICENSE)
