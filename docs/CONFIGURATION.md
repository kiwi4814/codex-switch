# Configuration

`codex-switch` uses `~/.codex-switch` by default. Set `CODEX_SWITCH_HOME` to relocate its profiles, cache, locks, logs, and daemon state. This does not change Codex's own home; set `CODEX_HOME` for that.

## Authentication prerequisite

The live Codex credential store must be file-backed because switching replaces `$CODEX_HOME/auth.json` atomically. Add the following to `$CODEX_HOME/config.toml`:

```toml
cli_auth_credentials_store = "file"
```

Explicit `keyring`, `auto`, and `ephemeral` modes are rejected. A managed configuration with `forced_login_method = "api"` is also incompatible with ChatGPT login profiles.

## Paths

| Path | Purpose |
|---|---|
| `$CODEX_HOME/auth.json` | Live authentication read by Codex. |
| `$CODEX_SWITCH_HOME/profiles/<alias>/auth.json` | Saved profile authentication. |
| `$CODEX_SWITCH_HOME/deleted-profiles/` | Recoverable deleted profiles. |
| `$CODEX_SWITCH_HOME/cache.json` | Per-profile usage cache. |
| `$CODEX_SWITCH_HOME/config.toml` | Optional settings. |
| `$CODEX_SWITCH_HOME/daemon-state.json` | Last Beta daemon state snapshot. |

Unset variables default to `~/.codex` and `~/.codex-switch` respectively.

## Settings

```toml
[proxy]
url = "socks5h://user:pass@127.0.0.1:1080"
no_proxy = "localhost,127.0.0.1"

[cache]
ttl = 300

[network]
max_concurrent = 20

[tui]
auto_refresh_interval_secs = 120

[use]
safety_margin_7d = 20
team_priority = true

[daemon]
poll_interval_secs = 60
switch_threshold = 80
cache_refresh_interval_secs = 300
auto_warmup = false
token_check_interval_secs = 300
notify = false
log_level = "error"
defer_switch_while_codex_running = true

[launch]
restore_delay_secs = 3
```

The daemon interval fields normalize `0` to their documented defaults. `launch.restore_delay_secs` is a compatibility delay, not a handshake; increase it only if the local Codex process reads authentication later than three seconds after launch.

## Proxy precedence

Proxy settings resolve in this order:

1. `--proxy`
2. `CS_PROXY`
3. `[proxy]` in `config.toml`
4. `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY`

Supported schemes are authenticated HTTP/HTTPS, SOCKS4, SOCKS5, and SOCKS5H. SOCKS5H resolves DNS through the proxy. Do not commit credentials in configuration files.

## Platform integration

- macOS uses a LaunchAgent for the Beta daemon.
- Linux uses a systemd user service; headless login should use `login --device`.
- Windows uses Task Scheduler and requires elevated PowerShell for daemon installation. Windows Terminal or PowerShell is recommended for the TUI.
