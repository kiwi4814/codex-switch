# Command reference

This page describes the stable command surface. The installed binary remains authoritative: use `codex-switch --help` and `codex-switch <command> --help` for exact flags and examples.

## Commands

| Command | Purpose |
|---|---|
| `login [--device] [alias]` | Add or reauthorize a profile through browser PKCE or device-code login. |
| `import <path> [alias]` | Validate and import one `auth.json` or recursively scan a directory. |
| `list [-f]` | Show profiles, usage, and availability; `-f` bypasses the cache. |
| `use [alias] [--consume-card]` | Switch explicitly or select the best eligible profile. |
| `launch [alias] [--consume-card] -- [args]` | Start Codex with a profile and forward all trailing arguments. |
| `reset-card <alias> [--yes]` | Consume the earliest-expiring reset card after confirmation. |
| `warmup [alias]` | Activate inactive main and eligible model-specific quota windows. |
| `rename <old> <new>` | Rename a saved profile. |
| `delete <alias> [--yes]` | Move an inactive profile into recoverable deleted storage. |
| `daemon start [--foreground]` | Start the Beta monitor in the foreground or detached mode. |
| `daemon stop` | Stop the Beta daemon. |
| `daemon status` | Report daemon support, service, process, and pending-switch state. |
| `daemon install` | Install the native user service; Windows requires elevated PowerShell. |
| `daemon uninstall` | Remove the native user service. |
| `self-update [--check] [--dev\|--stable] [--version <VERSION>]` | Check or update a direct installation. |
| `tui` | Open the interactive terminal dashboard. |
| `open` | Open the codex-switch data directory in the platform file manager. |

## Global options

| Option | Behavior |
|---|---|
| `--json` | Emit compact structured output for supported commands. |
| `--json-pretty` | Emit indented structured output. |
| `--proxy <URL>` | Override proxy configuration for this process. |
| `--color <auto\|always\|never>` | Control terminal color. |
| `--debug` | Emit diagnostic information to stderr; redact it before sharing. |
| `-V`, `--version` | Print the binary version. |

## Automation contract

- Structured data is written to stdout; progress and diagnostics are written to stderr.
- JSON and other non-interactive execution never consumes a reset card or deletes a profile without an explicit opt-in flag.
- `launch` treats everything after `--` as Codex CLI arguments.
- A manual `use` affects the next Codex process. Restart an already-running Codex process to load the new `auth.json`.
- Update checks are manual except for the one check performed when the TUI starts.

Examples:

```bash
codex-switch --json list
codex-switch --json use work
codex-switch launch work -- --model gpt-5.4
codex-switch self-update --check
```

## TUI essentials

Use arrow keys or `j`/`k` to navigate, `Enter` to open an account, `/` to filter, `r` to refresh, `Space` to mark accounts, `h` for the complete shortcut list, and `q` to quit. Destructive or consumptive actions require confirmation.
