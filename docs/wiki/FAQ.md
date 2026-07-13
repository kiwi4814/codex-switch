# FAQ

## Does codex-switch support keyring-backed Codex credentials?

No. Its core operation is atomic replacement of `$CODEX_HOME/auth.json`, so Codex must use `cli_auth_credentials_store = "file"`.

## Does switching affect an already-running Codex session?

No. Codex reads authentication at startup. Restart Codex, or use `codex-switch launch` for a new process.

## Where is account data stored?

Saved profiles and application state default to `~/.codex-switch`; the live Codex file defaults to `~/.codex/auth.json`. `CODEX_SWITCH_HOME` and `CODEX_HOME` relocate them independently.

## Is profile deletion permanent?

No. Inactive profiles are archived under `deleted-profiles/`. The active profile cannot be deleted.

## Is the daemon required?

No. It is an optional Beta feature. `codex-switch use`, `list`, `launch`, and the TUI work without it.

## How do I test the next release?

Use the rolling dev channel only when you are prepared to test prerelease behavior. Follow [Testing development releases](Development-Releases) for installation, verification, rollback, and issue-reporting steps.

## Are release binaries independently signed?

Not currently. Archives are checked against SHA256 files from the same GitHub Release, which detects corruption but shares the Release trust domain.

## Where should implementation details be updated?

In the main repository documentation. The Wiki is a curated navigation layer; see the [documentation index](https://github.com/xjoker/codex-switch/blob/master/docs/README.md).
