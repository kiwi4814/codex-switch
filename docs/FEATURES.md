# Feature guide

`codex-switch` manages multiple file-backed Codex CLI logins, observes their quota state, and selects an account for the next Codex process.

> **Authentication prerequisite:** Codex must use the file credential store. Set `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml`. Explicit `keyring`, `auto`, and `ephemeral` stores are rejected because they can bypass the `auth.json` file that `codex-switch` switches.

## Manage accounts

Add accounts with browser or device-code login:

```bash
codex-switch login work
codex-switch login --device server
```

Existing `auth.json` files can be imported individually or from a directory. Imports are parsed, identity-checked, validated against the usage service, deduplicated by account identity, and assigned collision-free aliases.

```bash
codex-switch import ~/auth-backups
```

Profile deletion is recoverable. An inactive profile is moved under `deleted-profiles/` after confirmation; the active profile cannot be deleted. See [recovery instructions](TROUBLESHOOTING.md#recover-a-deleted-profile).

## Observe quota and account state

Use the CLI for scripts and quick inspection, or the TUI for an interactive dashboard:

```bash
codex-switch list
codex-switch --json list
codex-switch tui
```

The usage model includes the main 5-hour and 7-day windows, additional model-specific pools, reset cards, spend limits, account restrictions, and model capabilities returned by the authenticated service. Cached entries are scoped by profile alias and retain their own fetch time.

Normal reads refresh only stale entries. Use `list -f` or the TUI refresh action when a fresh network read is required.

## Select an account

Select an explicit profile:

```bash
codex-switch use work
```

Or let the adaptive selector rank all profiles:

```bash
codex-switch use
```

Selection first applies eligibility gates for exhausted or unsafe accounts, then ranks eligible candidates using tier preference, current headroom, weekly sustainability, quota that is close to resetting, and recent use. If every account is ineligible, the best fallback is reported instead of pretending an account is healthy.

Switching replaces the live `$CODEX_HOME/auth.json` atomically while holding a process lock. Restart Codex after a manual switch because Codex reads the file at startup.

## Launch Codex with a profile

`launch` selects or stages a profile, starts Codex, then restores the previous live authentication after the configured compatibility delay:

```bash
codex-switch launch work -- --model gpt-5.4
codex-switch launch -- --full-auto
```

The launch lock serializes overlapping launch sessions. The restore delay is configurable because Codex does not expose an authentication-read handshake.

## Recover exhausted accounts

When the whole candidate pool is exhausted, an interactive `use` or `launch` can offer to consume the earliest-expiring reset card. Automation must opt in explicitly:

```bash
codex-switch use --consume-card
codex-switch reset-card work --yes
```

JSON or non-interactive execution never consumes a card without the explicit flag.

## Warm quota windows

`warmup` sends minimal requests to activate inactive main and model-specific quota windows discovered from the official model response:

```bash
codex-switch warmup
codex-switch warmup work
```

Model names are discovered at runtime rather than maintained as a hardcoded compatibility list. Already-active or unavailable pools are skipped.

## Run the background daemon

The Beta daemon monitors the current profile, refreshes cached usage and expiring tokens, and prepares a better account when the configured threshold is reached.

```bash
codex-switch daemon install
codex-switch daemon status
```

Service integration is platform-native: LaunchAgent on macOS, a systemd user service on Linux, and Task Scheduler on Windows. Windows installation requires elevated PowerShell.

By default, a switch is deferred while an interactive Codex process is running. The daemon records pending switches and operational state in `daemon-state.json`; long-lived MCP or app-server processes do not block a switch.

## Update the binary

Direct installs support stable and rolling development channels:

```bash
codex-switch self-update --check
codex-switch self-update
codex-switch self-update --dev
```

Downloaded archives are checked against the `.sha256` file in the same GitHub Release. This detects corruption, but it is not an independent signature: GitHub Releases over TLS remains the trust anchor. Homebrew installations must be updated with Homebrew.

Direct-install ownership is platform-specific:

- macOS and Linux default to `$HOME/.local/bin`; the installer manages a removable PATH block for zsh, bash, and fish, and prints manual guidance for other shells. `--system` explicitly selects `/usr/local/bin` and may require `sudo` for future updates.
- Windows installs under `%LOCALAPPDATA%\Programs\codex-switch` and updates the user PATH. Installing the optional Task Scheduler daemon remains a separate administrator-level action.
- Homebrew owns its Cellar binary and must remain Homebrew-managed.

Before downloading an archive, self-update verifies that it can create a replacement in the current executable's directory. Legacy Unix installs in `/usr/local/bin` can be migrated by rerunning the installer; the installer installs the user-owned copy before removing the legacy binary with one elevated operation.

Existing stable versions `0.0.3` through `0.0.21` can upgrade directly with `self-update`. A development build can move to stable with `self-update --stable`. Versions `0.0.1` and `0.0.2` should rerun the installer because their updater predates this migration path. The move to calendar versions preserves profiles and configuration; it is not a reset or downgrade.

## Automate safely

Most non-interactive commands support `--json` or `--json-pretty`. Structured output stays on stdout; progress and diagnostic messages use stderr. Commands that can consume a reset card or delete a profile require explicit non-interactive confirmation.

Never publish profile files, `auth.json`, unredacted debug output, proxy credentials, account IDs, email addresses, or workspace names.
