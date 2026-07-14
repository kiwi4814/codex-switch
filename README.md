# codex-switch

**A multi-account manager for [OpenAI Codex CLI](https://github.com/openai/codex).** Save local Codex logins, monitor quota, and select the best account before the next session.

[中文说明](README_CN.md) · [Documentation](docs/README.md) · [GitHub Wiki](https://github.com/xjoker/codex-switch/wiki) · [Releases](https://github.com/xjoker/codex-switch/releases)

> `codex-switch` manages local authentication files. Never publish profiles, `auth.json`, tokens, proxy credentials, or unredacted debug output.

## Quick start

Codex must use its file credential store. If needed, add this to `$CODEX_HOME/config.toml` (normally `~/.codex/config.toml`):

```toml
cli_auth_credentials_store = "file"
```

A managed Codex configuration with `forced_login_method = "api"` is incompatible because codex-switch manages ChatGPT login profiles.

Install the stable release.

macOS / Linux:

```bash
curl -fsSL https://github.com/xjoker/codex-switch/releases/latest/download/install.sh | bash
```

The Unix installer uses `$HOME/.local/bin` by default and configures PATH for zsh, bash, and fish when needed; other shells receive a manual PATH instruction. Use `--system` only when an administrator intentionally wants `/usr/local/bin`; system installs may require `sudo` for later updates.

Windows PowerShell:

```powershell
irm https://github.com/xjoker/codex-switch/releases/latest/download/install.ps1 | iex
```

Add an account and open the dashboard:

```bash
codex-switch login
codex-switch tui
```

For a headless machine, use `codex-switch login --device`. Homebrew users can install with `brew install xjoker/tap/codex-switch`.

## What it does

- Saves, imports, renames, switches, and recoverably deletes Codex profiles.
- Displays the main and model-specific quota pools in CLI and TUI views.
- Selects an eligible account with adaptive, pace-aware scoring.
- Launches Codex with a chosen profile and restores the previous live authentication.
- Supports reset cards, quota warmup, JSON output, proxies, and a Beta background daemon using LaunchAgent, systemd, or Windows Task Scheduler. Daemon cache refresh and optional warmup use `cache_refresh_interval_secs` and `auto_warmup`; see the configuration guide for details.
- Refreshes expiring tokens and updates direct installs from stable or rolling dev releases.
- Runs on macOS, Linux, and Windows.

![TUI](docs/tui.png)

## Common commands

| Goal | Command |
|---|---|
| Add an account | `codex-switch login [alias]` |
| Import existing authentication | `codex-switch import <path>` |
| Inspect accounts and quota | `codex-switch list` |
| Open the interactive dashboard | `codex-switch tui` |
| Select the best account | `codex-switch use` |
| Select one account | `codex-switch use <alias>` |
| Launch Codex | `codex-switch launch [alias] -- [codex args]` |
| Check for updates | `codex-switch self-update --check` |

Run `codex-switch <command> --help` for the authoritative options supported by the installed version.

## Updating existing installations

```bash
# Stable 0.0.3 and newer direct installs
codex-switch self-update

# Move a development build back to the stable channel
codex-switch self-update --stable

# Install an exact stable version
codex-switch self-update --version <VERSION>

# Homebrew installation
brew upgrade xjoker/tap/codex-switch
```

Homebrew distributes stable releases only and must retain ownership of its Cellar binary. To test the rolling dev build, remove the Homebrew package first, then use the direct dev installer:

```bash
brew uninstall codex-switch
curl -fsSL https://github.com/xjoker/codex-switch/releases/download/dev/install.sh | bash -s -- --dev
```

On Windows, install the rolling dev build from the matching dev release asset:

```powershell
$env:CS_DEV="1"; irm https://github.com/xjoker/codex-switch/releases/download/dev/install.ps1 | iex
```

Use `codex-switch self-update --stable` to keep a direct installation but return to the stable channel. To return to Homebrew ownership, run the direct uninstaller, keep the data directory when prompted, then reinstall with `brew install xjoker/tap/codex-switch`.

Versions `0.0.1` and `0.0.2` should rerun the installer because their updater predates the supported migration path. The release workflow continuously verifies direct self-update from `v0.0.19` on macOS, Linux, and Windows. Calendar-version releases are a normal upgrade from `0.0.x`; configuration and profiles remain in place.

Older macOS/Linux direct installs in `/usr/local/bin` should rerun the current installer once. The script verifies the download, validates one-time `sudo` access, installs the user-owned binary, and removes the legacy copy so PATH cannot keep selecting it. Profiles and configuration under `~/.codex-switch` are preserved. If an old updater first upgrades in place with `sudo`, the new updater detects the unmarked legacy location and prints this installer command before any later network check. Explicit `--system` installs carry a marker and continue to use system-level updates.

```bash
curl -fsSL https://github.com/xjoker/codex-switch/releases/latest/download/install.sh | bash
```

## Documentation

For users:

- [Feature guide](docs/FEATURES.md) — workflows and behavior boundaries.
- [Command reference](docs/COMMANDS.md) — commands, global flags, and automation behavior.
- [Configuration](docs/CONFIGURATION.md) — paths, proxy settings, daemon settings, and platform notes.
- [Troubleshooting](docs/TROUBLESHOOTING.md) — safe diagnostics and profile recovery.

For developers:

- [Architecture](docs/ARCHITECTURE.md) — state ownership and module flows.
- [Development guide](docs/DEVELOPMENT.md) — toolchain, tests, and change map.
- [Contributing](CONTRIBUTING.md) — pull request and verification contract.
- [Release process](docs/RELEASE.md) — maintainer gates and dev-to-stable promotion.

The repository documents are canonical and reviewable with the code. The Wiki provides task-oriented navigation for both audiences.

## Development

Requires Rust 1.88 or newer:

```bash
cargo build
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
```

See the [development guide](docs/DEVELOPMENT.md) before changing authentication, storage, selection, update, or daemon behavior.

## License

[MIT](LICENSE)
