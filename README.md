# codex-switch

**The multi-account manager for [OpenAI Codex CLI](https://github.com/openai/codex)** — manage unlimited accounts, monitor live quota across all of them, and auto-switch to the best one before each session.

[**中文文档 →**](README_CN.md)

> Latest stable release: `v0.0.21`.

## Get started in two minutes

You need the [Codex CLI](https://github.com/openai/codex) and a ChatGPT account that can sign in to Codex. `codex-switch` uses Codex's file-backed `auth.json`; if your Codex configuration selects an incompatible credential store, startup stops with instructions instead of changing your authentication.

Install the stable release:

**macOS / Linux**

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash
```

**Windows PowerShell**

```powershell
irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

Then add your first account and open the dashboard:

```bash
codex-switch login
codex-switch tui
```

Prefer plain commands? Run `codex-switch list` to inspect accounts and `codex-switch use` to select the best available account automatically.

> `codex-switch` stores account credentials locally. Do not share profile files or unredacted `--debug` output.

---

### TUI

![TUI](docs/tui.png)

### CLI

![CLI](docs/cli.png)

## Features

- **Profile Management** — Save, switch, rename, delete Codex accounts
- **Auto-Detection** — Automatically discovers and tracks the current `auth.json`
- **Usage Dashboard** — Live quota monitoring with color-coded 5h/7d progress bars for the main quota and every additional model quota pool returned by the API; the account page pairs quota visuals with the official model names and descriptions
- **Reset Cards (v0.0.20)** — Show Codex reset card counts and expiry times, then consume the earliest-expiring available card from CLI or TUI after confirmation
- **Adaptive Auto-Switch** — `codex-switch use` without arguments ranks accounts with a unified 5-component scoring algorithm, with Team accounts prioritized by default
- **Background Daemon (Beta)** — Optional `daemon` command uses LaunchAgent on macOS, a systemd user service on Linux, and Task Scheduler on Windows
- **Stale-Only Refresh** — `use`, `list`, and TUI refresh only accounts whose cached usage has expired
- **Progress Display** — Long-running `use`, `list`, and directory `import` operations show a single-line cross-platform progress indicator
- **Interactive TUI** — Full terminal UI with live usage data, color-coded status, and keyboard shortcuts
- **OAuth Login** — Built-in PKCE browser login flow, no manual token copying
- **Token Auto-Refresh** — Automatically refreshes expired tokens using refresh_token
- **Validated Bulk Import** — Import a single `auth.json` or recursively scan a directory, validate files, and auto-assign unique aliases
- **Pace Marker** — Visual indicator on usage bars showing expected consumption based on elapsed window time
- **Warmup** — `warmup` activates the main window and every model-specific `codex_*` quota pool that can be matched to the authenticated models response, while skipping already-active accounts
- **Manual Self-Update** — `self-update --check` checks GitHub Releases on demand; `self-update` installs the latest release (supports stable and dev channels)
- **Launch with Profile** — `launch` starts Codex CLI with a specific (or best) profile's auth, transparently forwarding all arguments. Auth is swapped only during startup, then immediately restored
- **Over-Pace Warning** — Red `!` indicator on 5h/7d columns when usage exceeds expected pace
- **Proxy Support** — HTTP/HTTPS/SOCKS4/SOCKS5/SOCKS5H with authentication
- **Cross-Platform** — macOS, Linux, Windows (full RGB color palette for consistent TUI rendering)
- **JSON Output** — `--json` flag for scripting and automation

## More installation options

The quick start above is recommended for most users. The alternatives below cover package-manager installs, development builds, manual downloads, and source builds.

### Homebrew (macOS / Linux)

```bash
brew install xjoker/tap/codex-switch
```

### Dev Build (Latest Development Version)

Development builds may be unstable and are intended for testing before the next stable release.

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash -s -- --dev
```

**Windows (PowerShell):**

```powershell
$env:CS_DEV="1"; irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

### Uninstall

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash -s -- --uninstall
```

**Windows (PowerShell):**

```powershell
$env:CS_UNINSTALL="1"; irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
```

### From GitHub Releases (Manual)

Download pre-built binaries from [Releases](https://github.com/xjoker/codex-switch/releases):

| Platform | Architecture | File |
|----------|-------------|------|
| macOS | Apple Silicon (M1/M2/M3) | `cs-darwin-arm64.tar.gz` |
| macOS | Intel | `cs-darwin-amd64.tar.gz` |
| Linux | x86_64 | `cs-linux-amd64.tar.gz` |
| Linux | ARM64 | `cs-linux-arm64.tar.gz` |
| Windows | x86_64 | `cs-windows-amd64.zip` |
| Windows | ARM64 | `cs-windows-arm64.zip` |

The install scripts download the matching `.sha256` asset and verify it before extracting the archive.

### From Source

Requires [Rust](https://rustup.rs/) 1.88+:

```bash
git clone https://github.com/xjoker/codex-switch.git
cd codex-switch
cargo build --release
# Binary: target/release/codex-switch (or target\release\codex-switch.exe on Windows)
sudo cp target/release/codex-switch /usr/local/bin/  # macOS/Linux
```

## Common tasks

| Goal | Command |
|------|---------|
| Add an account | `codex-switch login` |
| Add an account on a headless server | `codex-switch login --device` |
| View accounts and live quota | `codex-switch list` |
| Open the interactive dashboard | `codex-switch tui` |
| Switch to one account | `codex-switch use <alias>` |
| Select the best available account | `codex-switch use` |
| Launch Codex with the best account | `codex-switch launch` |
| Import existing auth files | `codex-switch import <path>` |
| Check for updates | `codex-switch self-update --check` |

### Authentication and storage requirements

`codex-switch` switches Codex's file-backed `auth.json`. Codex must therefore use its default file credential store, or explicitly set `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml` (normally `~/.codex/config.toml`). Explicit `keyring`, `auto`, or `ephemeral` modes are rejected because they can bypass the live file. An empty `CODEX_HOME` falls back to `~/.codex`; a non-empty value selects the Codex home used for both `auth.json` and `config.toml`.

`CODEX_SWITCH_HOME` optionally relocates codex-switch's own profiles, cache, locks, and daemon state from `~/.codex-switch`; it does not change Codex's `auth.json` location.

ChatGPT login is required. A managed Codex configuration with `forced_login_method = "api"` is incompatible and fails with an actionable error instead of modifying authentication state.

Use aliases such as `work` or `personal` in place of `<alias>`. Run `codex-switch <command> --help` for command-specific options and examples.

## Commands

| Command | Description |
|---------|-------------|
| `codex-switch use [alias] [--consume-card]` | Switch to a profile. Omit alias to auto-select with the adaptive scoring algorithm; when the pool is exhausted, `--consume-card` (or an interactive y/N prompt) consumes the earliest-expiring reset card to revive an account instead of leaving it exhausted (ignored when an alias is given) |
| `codex-switch list [-f]` | List all profiles with account info, usage, and availability (`-f` force refresh) |
| `codex-switch reset-card <alias> [--yes]` | Consume the earliest-expiring available Codex reset card for a profile. Prompts for confirmation unless `--yes` is used; JSON mode requires `--yes` |
| `codex-switch launch [alias] [--consume-card] [-- args...]` | Launch Codex CLI with a profile's auth. Omit alias to auto-select with adaptive scoring, with the same `--consume-card` reset-card revival behavior as `use`. All arguments after `--` are forwarded to codex |
| `codex-switch warmup [alias]` | Send a minimal request to start the 5h/7d quota window countdown. Omit alias to warm up all profiles |
| `codex-switch login [--device] [alias]` | Log in via OAuth (`--device` for headless servers). If alias exists, re-authorizes |
| `codex-switch rename <old> <new>` | Rename a profile |
| `codex-switch delete <alias> [--yes]` | Remove an inactive profile from the account list and archive it for recovery; prompts by default |
| `codex-switch import <path> [alias]` | Import one auth.json file, or recursively validate and import all JSON files under a directory |
| `codex-switch daemon start [--foreground]` | Start the auto-switch daemon (Beta). Detached by default; use `--foreground` for service managers |
| `codex-switch daemon stop` | Stop a running Beta daemon |
| `codex-switch daemon status` | Show Beta daemon status and platform support details |
| `codex-switch daemon install` | Install the Beta daemon (macOS LaunchAgent / Linux systemd user service / Windows Task Scheduler; Windows requires elevated PowerShell) |
| `codex-switch daemon uninstall` | Remove the Beta daemon user service |
| `codex-switch self-update [--check] [--dev\|--stable] [--version <VERSION>]` | Check or update a direct install. Without a channel flag it stays on the current stable/dev channel; `--version` selects an exact stable release |
| `codex-switch tui` | Launch the interactive terminal UI |
| `codex-switch open` | Open the config directory in file manager |

### Global Options

| Option | Description |
|--------|-------------|
| `--json` | Output as compact JSON (for scripting/pipes) |
| `--json-pretty` | Output as pretty-printed JSON |
| `--proxy <URL>` | Set proxy (see [Proxy](#proxy-support) section) |
| `--color <auto\|always\|never>` | Color output mode (default: auto) |
| `--debug` | Enable debug logging (shows HTTP requests, API responses, cache status) |
| `-V, --version` | Print version |

## TUI Keyboard Shortcuts

Press `Enter` to open the selected account menu. If accounts are marked, `Enter` opens the batch menu instead.

| Key | Action |
|-----|--------|
| `j` / `k` or `Up` / `Down` | Navigate accounts |
| `Enter` | Open account or batch action menu |
| `/` | Search / filter accounts |
| `r` | Refresh visible accounts |
| `a` | Add a new account |
| `t` | Toggle auto-refresh |
| `W` | Toggle auto-warmup for accounts whose 5h window has expired |
| `i` | Show / hide the account detail panel |
| `s` | Cycle sort mode (name / quota / status) |
| `Space` | Mark / unmark account for batch operations |
| `u` (account menu) | Switch to selected account |
| `c` (account menu) | Confirm and consume the earliest-expiring reset card |
| `w` (account menu) | Warm up selected account |
| `l` (account menu) | Re-login selected account |
| `n` (account menu) | Rename selected profile |
| `d` (account menu) | Delete selected profile (with confirmation) |
| `r` / `w` / `l` / `d` (batch menu) | Refresh, warm up, re-login, or delete marked accounts |
| `h` | Show help |
| `Esc` | Clear search/marks or close the active popup |
| `q` | Quit |

## Updating

Update checks are manual except for TUI launch. `codex-switch tui` checks once at startup; `startup`, `list`, and `use` do not check automatically.

```bash
# Check whether a newer release exists
codex-switch self-update --check

# Update a direct install to the latest release
codex-switch self-update

# Change channels explicitly
codex-switch self-update --dev
codex-switch self-update --stable

# Install an exact stable release (downgrades are rejected)
codex-switch self-update --version 20260712.1.0
```

- Homebrew installs are not self-overwritten. Use `brew upgrade xjoker/tap/codex-switch`.
- Direct installs verify the release `.sha256` before replacing the current executable. The checksum ships in the same GitHub Release as the binary, so this guards against corrupted downloads — not against a compromised Release. The trust anchor is GitHub Releases over TLS; there is no independent code signature yet.
- Without flags, `self-update` stays on the channel encoded by the current binary. Use `--dev` or `--stable` to change channels explicitly.
- Homebrew users must `brew uninstall codex-switch` before using `--dev`.

## Proxy Support

Proxy resolution priority (highest to lowest):

1. `--proxy` CLI flag
2. `CS_PROXY` environment variable
3. Config file `~/.codex-switch/config.toml`
4. Standard environment variables (`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`)

### Supported Protocols

| Protocol | DNS Resolution | Auth |
|----------|---------------|------|
| `http://[user:pass@]host:port` | Local | Yes |
| `https://[user:pass@]host:port` | Local | Yes |
| `socks4://host:port` | Local | No |
| `socks5://[user:pass@]host:port` | Local | Yes |
| `socks5h://[user:pass@]host:port` | Remote (via proxy) | Yes |

### Config File

`~/.codex-switch/config.toml`:

```toml
[proxy]
url = "socks5h://user:pass@127.0.0.1:1080"
no_proxy = "localhost,127.0.0.1"

[cache]
ttl = 300  # Cache TTL in seconds (default: 300)

[network]
max_concurrent = 20  # Max concurrent usage requests (default: 20)

[tui]
auto_refresh_interval_secs = 120  # Auto-refresh interval in seconds (default: 120, minimum: 30)

[use]
safety_margin_7d = 20       # Weekly safety margin used by adaptive scoring (default: 20)
team_priority = true        # Prefer Team accounts with a +500 tier bonus (default: true)

[daemon]
poll_interval_secs = 60         # Usage poll interval (default: 60)
switch_threshold = 80           # Switch when current 5h usage >= this % (default: 80)
cache_refresh_interval_secs = 300 # Refresh all saved profile caches (default: 300)
auto_warmup = false             # Warm inactive windows during cache refresh (default: false)
token_check_interval_secs = 300 # Background token refresh check interval (default: 300)
notify = false                  # Desktop notification on switch (macOS/Linux/Windows, default: false)
log_level = "error"             # Daemon log level (default: "error")
defer_switch_while_codex_running = true # Hold switches while an interactive Codex session runs (default: true)

[launch]
restore_delay_secs = 3          # Seconds to wait before restoring auth.json after codex starts (default: 3)
```

For the three daemon interval fields, `0` is treated as unset and normalized to the documented defaults: polling `60`, cache refresh `300`, and token check `300` seconds.

`launch.restore_delay_secs` is a compatibility delay, not a handshake with Codex. Increase it if Codex on the local machine reads `auth.json` more than three seconds after launch.

### Examples

```bash
# CLI flag
codex-switch --proxy socks5h://127.0.0.1:1080 list

# Environment variable
export CS_PROXY="http://user:pass@proxy.corp.com:8080"
codex-switch list

# Standard env var (reqwest reads this automatically)
export HTTPS_PROXY="http://proxy.corp.com:8080"
codex-switch list
```

## Common Usage Scenarios

### Auto-switch before each Codex session

```bash
# Add to your shell profile (.zshrc / .bashrc):
codex-switch use && codex
```

### Keep the next session ready with the daemon (Beta)

Use the Beta daemon when you want `codex-switch` to monitor the current account continuously and prepare the next Codex launch in the background. The current implementation installs a LaunchAgent on macOS, a systemd user service on Linux, or an on-logon Task Scheduler task on Windows.

```bash
# Start a detached daemon
codex-switch daemon start

# Check whether it is running
codex-switch daemon status

# Stop it
codex-switch daemon stop

# Install/remove the daemon service
# Windows: run these two commands from elevated PowerShell.
codex-switch daemon install
codex-switch daemon uninstall
```

The Beta daemon uses the same adaptive scoring logic as `codex-switch use`. It refreshes the current account on each poll, switches only when `daemon.switch_threshold` is met or exceeded and a better candidate exists, refreshes all saved profile caches on `daemon.cache_refresh_interval_secs`, and refreshes expiring tokens on a separate timer. `daemon.auto_warmup = true` additionally warms inactive quota windows; it is off by default. Daemon switching is non-interactive: an untracked live `auth.json` may be replaced after its normal rotating backup is created. Save or import that account first if it must remain directly selectable. The daemon prepares future Codex launches; an already-running Codex process still needs to be restarted after a switch.

While an interactive Codex session (`codex`, `codex resume`, `codex exec`) is running, the daemon holds the switch as pending and retries on the next poll; long-lived Codex infrastructure such as MCP servers and `app-server` hosts does not block switching. Set `daemon.defer_switch_while_codex_running = false` to switch immediately regardless. The daemon writes a state snapshot to `~/.codex-switch/daemon-state.json` (last poll, last switch, pending switch, last error) — shown by `codex-switch daemon status` — and logs to `~/.codex-switch/logs/` with daily rotation capped at 7 files.

### Scheduled token refresh via cron (optional)

Keep cached usage data and tokens fresh in the background so `codex-switch use` is instant:

```bash
# Edit crontab
crontab -e

# Refresh all account usage every 5 minutes
*/5 * * * * /usr/local/bin/codex-switch list --json > /dev/null 2>&1
```

This runs `codex-switch list` periodically, which refreshes stale tokens and caches usage data. It does **not** switch accounts automatically.

### Use in CI / automation

```bash
# Select the best account and pass arguments directly to Codex
codex-switch launch -- --model gpt-5.4
```

## Troubleshooting

Start with the error message: configuration, login, and permission failures include the path or next command to use.

| Symptom | What to do |
|---------|------------|
| No saved profiles | Run `codex-switch login` or `codex-switch import <path>` |
| Credential store is not file-backed | Set `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml` |
| Windows daemon install reports access denied | Open PowerShell as Administrator and run the command again |
| TUI layout is broken in Git Bash | Use Windows Terminal or PowerShell |
| A profile was deleted by mistake | See [Recover a deleted profile](#recover-a-deleted-profile) |

For network or API failures, rerun the command with `--debug`:

```bash
codex-switch --debug list
codex-switch --debug use
```

If the issue persists, [open an issue](https://github.com/xjoker/codex-switch/issues) with the command, operating system, version, and redacted debug output. Remove tokens, email addresses, account IDs, workspace names, and proxy credentials.

### Recover a deleted profile

Delete is recoverable: the profile directory is moved from `profiles/` to `deleted-profiles/` instead of being erased. Stop the daemon, move the newest matching backup directory back, then confirm it appears:

```bash
codex-switch daemon stop
# Move ~/.codex-switch/deleted-profiles/<alias>.backup-<timestamp>
# back to ~/.codex-switch/profiles/<alias>
codex-switch list
```

On Windows, the same directories are under `%USERPROFILE%\.codex-switch`. If `CODEX_SWITCH_HOME` is set, use that directory instead.

## How It Works

### File Locations

| Path | Description |
|------|-------------|
| `~/.codex/auth.json` | Live Codex CLI auth file (or `$CODEX_HOME/auth.json`) |
| `~/.codex-switch/profiles/<alias>/auth.json` | Saved profile data |
| `~/.codex-switch/deleted-profiles/<alias>.backup-<timestamp>/` | Recoverable deleted profiles |
| `~/.codex-switch/current` | Currently active profile name |
| `~/.codex-switch/auth.lock` | File lock that serializes live `auth.json` switches |
| `~/.codex-switch/config.toml` | Configuration file |

### Auto-Detection

On every interactive launch, codex-switch compares the live `~/.codex/auth.json` against all saved profiles:

- **New account detected** (e.g., you ran `codex login`) — prompts to save as a new profile
- **Tokens refreshed** for an existing account — prompts to update that profile
- **Non-interactive environments** (pipes, cron, CI) — reports the change but never mutates state silently

When you run `codex-switch list` or `codex-switch tui`, the tool also checks if the live auth.json belongs to an untracked account and automatically saves it as a new profile (using the email username as alias).

### Deduplication

On login or import, the tool matches accounts by `account_id` (primary) or `email` (fallback). If the same account already exists under a different alias, it updates the existing profile instead of creating a duplicate.

### Import Validation

`codex-switch import` validates every candidate file in stages:

1. File format — valid JSON
2. Structure — required `tokens` fields and a decodable `id_token`
3. Usage validation — token refresh and usage API check, unless explicitly skipped in tests
4. Save — deduplicate by identity and assign a unique alias if needed

If the input path is a directory, the command scans recursively for `.json` files and reports imported vs skipped files.

### Smart Auto-Switch (`codex-switch use`)

When called without an alias, `codex-switch use` ranks every account with a single adaptive algorithm. It first reuses fresh cache entries and only refreshes stale accounts.

The algorithm uses a **two-phase** approach:
1. **Eligibility gate** — filters accounts that are exhausted, weekly-critical with a distant reset, or below the Free-plan safety floor. If **all** accounts are ineligible, the highest-scoring one is used as a fallback.
2. **Adaptive score** — ranks the remaining accounts with five components:

```text
score = tier_bonus + headroom + sustain + drain_value + recency
```

- `tier_bonus` (0 or +500) — Team accounts are preferred by default when `team_priority = true`. This is a priority, not a guarantee: exhausted or unsafe Team accounts can still lose or be filtered out.
- `headroom` (0..1100) — Pace-aware 5h capacity based on burn rate and time-to-reset, not just static remaining%.
- `sustain` (-800..0) — 7d budget-per-window safety penalty.
- `drain_value` (0..300) — Bonus for spending quota that will reset within 60 minutes; the weight adapts to pool size and exhaustion ratio.
- `recency` (-60..0) — Small spread penalty to avoid repeatedly hammering the same account.

This replaces `max-remaining`, `drain-first`, and `round-robin`. There is no mode selection in v0.0.13+.

> **Note:** After switching accounts, you must **restart Codex** for it to pick up the new `auth.json`. The Codex CLI reads `auth.json` at startup and does not watch for file changes.

#### Eligibility Gate

Accounts are marked **ineligible** when:
- 5h window is fully exhausted (>=100%)
- 7d window is fully exhausted (>=100%)
- 7d remaining is below the critical threshold (`25%` of `safety_margin_7d`, minimum `1%`) and the 7d reset is more than 48 hours away
- Free plan account has fallen below the built-in 5h safety floor

Ineligible accounts are excluded from selection unless ALL accounts are ineligible, in which case the best-scoring one is used as a last resort.

### Auto-Switch Configuration

`[use]` now has only two knobs:

- `safety_margin_7d` — Weekly safety threshold used by the sustain component and the eligibility gate
- `team_priority` — Default `true`; grants Team accounts a `+500` tier bonus

Legacy `mode` and `min_remaining` are ignored with a warning in v0.0.13+.

### Cache Behavior

- Usage cache is stored per profile alias in `~/.codex-switch/cache.json`
- Each cached entry keeps its own refresh timestamp; JSON output exposes it as `usage.fetched_at`
- `list`, `use`, and the TUI only refresh stale accounts by default
- `list -f` and TUI `r` bypass cache and force a refresh for all accounts
- Directory import always validates every file and shows progress as it advances

### Token Auto-Refresh

When a usage query returns HTTP 401/403, the tool automatically attempts to refresh the token using the stored `refresh_token`. If successful, the new tokens are persisted back to the profile and the live auth.json.

### Safety Notes

- CLI and TUI both refuse to delete the active profile
- Deleting an inactive profile requires confirmation and moves it to private recoverable storage
- JSON mode keeps stdout machine-readable; human progress/messages go to stderr instead

## Platform Notes

### macOS

- Default Codex auth path: `~/.codex/auth.json`
- Browser opens via system `open` command
- File manager opens via `open`

### Linux

- Default Codex auth path: `~/.codex/auth.json`
- Browser opens via `xdg-open` (ensure a desktop browser is configured)
- File manager opens via `xdg-open`
- WSL: browser opening may require `wslu` package (`sudo apt install wslu`)
- **Headless servers (no browser):** Use `codex-switch login --device` for device code flow — displays a URL and code to enter on any device with a browser

### Windows

- Default Codex auth path: `%USERPROFILE%\.codex\auth.json`
- Browser opens via `rundll32.exe url.dll,FileProtocolHandler` (fallback: `webbrowser` crate)
- File manager opens via `explorer.exe`
- Terminal: works with Windows Terminal, PowerShell, and cmd.exe
- TUI rendering uses Windows Console API via `crossterm`
- `daemon install` uses an on-logon Windows Task Scheduler task and requires elevated PowerShell; use `daemon status` to inspect whether it is installed and running
- **Recommended terminal: [Windows Terminal](https://aka.ms/terminal).** Git Bash (mintty) has known compatibility issues with TUI rendering — use Windows Terminal or PowerShell instead

## JSON Output

Most commands support `--json` for machine-readable output (except `tui` and `open`):

```bash
# List profiles as JSON
codex-switch --json list

# Switch and get result
codex-switch --json use alice

# Check updates in JSON mode
codex-switch --json self-update --check
```

## Building

```bash
# Debug build
cargo build

# Release build (optimized, stripped)
cargo build --release

# Cross-compile for Linux (from macOS)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# Cross-compile for Windows (from macOS/Linux)
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Releasing

Maintainer-only. See [docs/RELEASE.md](docs/RELEASE.md) for the full procedure (dev rolling tag, stable tag, refspec gotchas).

## Changelog

See [docs/CHANGELOG.md](docs/CHANGELOG.md) for a detailed list of changes in each release.

## License

[MIT](LICENSE)
