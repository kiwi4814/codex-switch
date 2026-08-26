# codex-switch

**A multi-account manager for [OpenAI Codex CLI](https://github.com/openai/codex).** Save local Codex logins, monitor quota, and select the best account before the next session.

[中文说明](README_CN.md) · [**Documentation (Wiki)**](https://github.com/xjoker/codex-switch/wiki) · [Releases](https://github.com/xjoker/codex-switch/releases)

> `codex-switch` manages local authentication files. Never publish profiles, `auth.json`, tokens, proxy credentials, or unredacted debug output.

## Fork note

This is a fork of [xjoker/codex-switch](https://github.com/xjoker/codex-switch). It adds:

- weekly automatic warmup detection (`daemon.weekly_auto_warmup`);
- scheduled 5-hour-window warmup (`daemon.five_hour_warmup_times`), defaulting to `05:00` / `10:10` / `15:20` local time;
- a Docker Compose deployment for a host-installed Codex CLI plus a containerized `codex-switch` daemon.

**Upstream installation** — the `install.sh` / `install.ps1` / Homebrew commands below install xjoker's official build. That build does **not** contain the scheduled-warmup changes in this fork.

**This fork** — use the [Docker Compose deployment](#docker-compose-deployment-single-host-ubuntu), or build from source on the `feature/scheduled-warmup` branch:

```bash
git clone -b feature/scheduled-warmup https://github.com/kiwi4814/codex-switch.git
cd codex-switch
cargo build --locked --release   # target/release/codex-switch
```

## Quick start

Codex must use its file credential store. If needed, add this to `$CODEX_HOME/config.toml` (normally `~/.codex/config.toml`); a managed configuration with `forced_login_method = "api"` is incompatible:

```toml
cli_auth_credentials_store = "file"
```

Install the stable release — macOS / Linux:

```bash
curl -fsSL https://github.com/xjoker/codex-switch/releases/latest/download/install.sh | bash
```

Windows PowerShell:

```powershell
irm https://github.com/xjoker/codex-switch/releases/latest/download/install.ps1 | iex
```

Homebrew users: `brew install xjoker/tap/codex-switch`.

> **Note:** this project is not distributed on crates.io — do not `cargo install codex-switch`; that package name belongs to an unrelated project.

Then add an account and open the dashboard:

```bash
codex-switch login        # use --device on a headless machine
codex-switch tui          # interactive dashboard
codex-switch use          # switch to the best eligible account
codex-switch launch       # start Codex with the best account
```

![TUI](docs/tui.png)

## What it does

- Saves, imports, renames, switches, and recoverably deletes Codex profiles.
- Displays the main and model-specific quota pools in CLI and TUI views.
- Selects an eligible account with adaptive, pace-aware scoring, and launches Codex with it.
- Supports reset cards, quota warmup, JSON output, proxies, and a Beta background daemon (LaunchAgent, systemd, or Windows Task Scheduler; tune `cache_refresh_interval_secs` and `auto_warmup`).
- Refreshes expiring tokens and updates direct installs: `self-update`, `self-update --stable`, `self-update --version <VERSION>`, or the rolling dev channel via `self-update --dev` — new dev installs use [install.sh](https://github.com/xjoker/codex-switch/releases/download/dev/install.sh) / [install.ps1](https://github.com/xjoker/codex-switch/releases/download/dev/install.ps1) from the `dev` release.
- Direct `self-update` verifies both SHA-256 and GitHub build provenance with `gh attestation verify`; install a current [GitHub CLI](https://cli.github.com/) before using it.
- Runs on macOS, Linux, and Windows.

> **Upgrading from a `0.0.x` install?** This release line intentionally breaks two conventions: versions are now calendar-based (`YYYYMMDD.N.0`, so updates sort and read by date), and Unix installs moved from `/usr/local/bin` to the user-owned `$HOME/.local/bin` so `self-update` never needs `sudo`. A normal `self-update` or one installer rerun migrates you; profiles and configuration are preserved. All breaking changes and reasons: [Updating](https://github.com/xjoker/codex-switch/wiki/Updating).

## Docker Compose deployment (single-host Ubuntu)

Codex CLI stays on the host; only the `codex-switch` daemon runs in the container. Both share the host's real `~/.codex` and `~/.codex-switch` through bind mounts, so a switch made inside the container is what the next host `codex` run picks up.

```text
Ubuntu host
├── OpenAI Codex CLI            -> reads ~/.codex/auth.json
└── Docker Compose
    └── codex-switch            -> daemon start --foreground
        /data/codex             <- bind mount of ~/.codex
        /data/codex-switch      <- bind mount of ~/.codex-switch
```

### Host prerequisites

Install [OpenAI Codex CLI](https://github.com/openai/codex) on the host, and make sure `~/.codex/config.toml` contains:

```toml
cli_auth_credentials_store = "file"
```

`codex-switch` reads and writes `~/.codex/auth.json`; with the keychain store there is no file to manage. Docker does not install or run Codex CLI.

### Configure

```bash
cp .env.example .env
```

Fill in the four values — `id -u` for `PUID`, `id -g` for `PGID`, `echo "$HOME"` for `HOST_HOME`:

```env
PUID=1000
PGID=1000
HOST_HOME=/home/ubuntu
TZ=Asia/Shanghai
```

Then validate and build:

```bash
docker compose config -q
docker compose build
```

### Accounts and one-off commands

Every one-off command runs in a throwaway container against the same bind-mounted state:

```bash
docker compose run --rm codex-switch login --device account-1
docker compose run --rm codex-switch login --device account-2
docker compose run --rm codex-switch list -f
docker compose run --rm -it codex-switch tui
docker compose run --rm codex-switch warmup account-1
docker compose run --rm codex-switch use account-2
```

`--device` is the device-code flow, which is what a headless container needs. Because `~/.codex` is bind-mounted, `use account-2` rewrites the host's `~/.codex/auth.json` — the next `codex` you start on the host uses that account.

### Run the daemon

```bash
docker compose up -d
docker compose ps
docker compose logs -f codex-switch
docker compose down
```

The container runs `codex-switch daemon start --foreground` and Compose owns its lifecycle with `restart: unless-stopped`. Do **not** run `codex-switch daemon install` in Docker mode; systemd user services, launchd, and `enable-linger` belong to the bare-metal install only.

`docker compose down` removes the container. It does not touch `~/.codex` or `~/.codex-switch` — those are host directories. `docker compose down --volumes` and `docker system prune` are not part of normal operation.

### Configuration and timezone

Keep editing `~/.codex-switch/config.toml` on the host:

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

`five_hour_warmup_times` is matched against **container** local time, which is set by `TZ` in `.env`. Without `TZ` the container is UTC and those hours fire at the wrong wall-clock time.

### Why `pid: host`

`defer_switch_while_codex_running = true` works by scanning the process table for an interactive Codex session (`src/daemon/codex_process.rs` reads `/proc/*/cmdline` on Linux). Codex CLI runs on the host while the daemon runs in the container, so in Docker's default PID namespace the daemon sees no Codex process and would replace `auth.json` mid-conversation. `pid: host` is what keeps that detection working.

The trade-off: the container can see the host's full process list, including other users' command lines. That is the only host integration this deployment uses — there is no `privileged: true`, no Docker socket mount, no host networking, and no published port (`codex-switch` serves no network traffic).

### Permissions

`user: "${PUID}:${PGID}"` makes the container process run as your host user, so `auth.json`, `config.toml`, and `profiles/*` created inside the container stay owned by that user. Leaving `PUID`/`PGID` empty would run as root and leave root-owned files in your home directory.

### Upgrade and backup

```bash
git fetch
git pull --ff-only
docker compose build --pull
docker compose up -d
```

`~/.codex` and `~/.codex-switch` are host-persistent and survive image rebuilds. Back up before upgrading:

```bash
cp -a ~/.codex-switch/config.toml ~/.codex-switch/config.toml.bak
cp -a ~/.codex-switch/profiles ~/.codex-switch/profiles.bak
```

## Documentation

The **[GitHub Wiki](https://github.com/xjoker/codex-switch/wiki)** is the complete documentation — getting started, feature guide, command reference, configuration, updating and channels, troubleshooting, FAQ, and the contributor guides (architecture, onboarding). Its sources live in [`docs/wiki/`](docs/wiki) and are reviewed with the code.

Maintainer documents: [release process](docs/RELEASE.md) · [changelog](docs/CHANGELOG.md) · [contributing](CONTRIBUTING.md).

## Development

Requires Rust 1.88 or newer:

```bash
cargo build
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
```

See the [developer onboarding](https://github.com/xjoker/codex-switch/wiki/Developer-Onboarding) and [architecture](https://github.com/xjoker/codex-switch/wiki/Architecture-Overview) Wiki pages before changing authentication, storage, selection, update, or daemon behavior.

## License

[MIT](LICENSE)
