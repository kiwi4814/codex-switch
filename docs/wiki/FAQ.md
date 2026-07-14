# FAQ

## Does codex-switch support keyring-backed Codex credentials?

No, and this is permanent by design. OS keyrings provide no locking or atomic-replace semantics, and Codex's keyring entry format is an undocumented internal layout that has already changed between versions and platforms. Codex must use `cli_auth_credentials_store = "file"`; if it previously used a keyring store, switch the setting and log in again. See [why only the file store is supported](Configuration#why-only-the-file-store-is-supported).

## Does switching affect an already-running Codex session?

No. Codex reads authentication at startup. Restart Codex, or use `codex-switch launch` for a new process.

## Where is account data stored?

Saved profiles and application state default to `~/.codex-switch`; the live Codex file defaults to `~/.codex/auth.json`. `CODEX_SWITCH_HOME` and `CODEX_HOME` relocate them independently.

## Is profile deletion permanent?

No. Inactive profiles are archived under `deleted-profiles/`. The active profile cannot be deleted.

## Is the daemon required?

No. It is an optional Beta feature. `codex-switch use`, `list`, `launch`, and the TUI work without it.

## What do the version numbers mean?

Releases use calendar versions in the form `YYYYMMDD.N.0`. Rolling dev builds end in `-dev`. Calendar versions are a normal upgrade from the earlier `0.0.x` series; see [Updating](Updating).

## How do I test the next release?

Use the rolling dev channel only when you are prepared to test prerelease behavior. Follow [Testing development releases](Development-Releases) for installation, verification, rollback, and issue-reporting steps.

## Are release binaries independently signed?

Not currently. Archives are checked against SHA256 files from the same GitHub Release, which detects corruption but shares the Release trust domain.

## Where should documentation fixes go?

These Wiki pages are generated from [`docs/wiki/` on the `dev` branch](https://github.com/xjoker/codex-switch/tree/dev/docs/wiki). Open a pull request against those sources; do not edit the published Wiki directly.

## Next steps

- New installation: [Getting started](Getting-Started).
- Daily workflows: [Feature guide](Feature-Guide).
- Errors and recovery: [Troubleshooting](Troubleshooting).
