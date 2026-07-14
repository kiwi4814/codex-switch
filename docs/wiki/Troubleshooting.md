# Troubleshooting

Start with the complete error message, its file path, and the command that produced it. Configuration, login, update, and permission failures include the path or next command when recovery is known.

| Symptom | Action |
|---|---|
| No saved profiles | Run `codex-switch login` or `codex-switch import <path>`. |
| Credential store is not file-backed | Set `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml`. |
| Headless login cannot open a browser | Run `codex-switch login --device`. |
| Windows daemon installation is denied | Open PowerShell as Administrator and retry. |
| TUI layout is broken in Git Bash | Use Windows Terminal or PowerShell. |
| Direct update does not replace a Homebrew binary | Run `brew upgrade xjoker/tap/codex-switch`. |
| A Homebrew installation cannot switch to dev | Run `brew uninstall codex-switch`, then follow [Testing development releases](Development-Releases#install-the-rolling-dev-build). |
| A direct dev installation should return to Homebrew | Run the direct uninstaller, keep the data directory when prompted, then run `brew install xjoker/tap/codex-switch`. |
| macOS/Linux self-update reports that the install directory is not writable | Rerun the current installer once to migrate a legacy `/usr/local/bin` direct install to `$HOME/.local/bin`; see [Updating](Updating#legacy-direct-installs). Use `sudo codex-switch self-update` only for an intentional `--system` install. |
| A dev build should return to stable | Run `codex-switch self-update --stable`. |
| An installed daemon ignores `CODEX_SWITCH_HOME` | The generated service forwards only `HOME` and `CODEX_HOME`; add `CODEX_SWITCH_HOME` to the service definition manually. See [Configuration](Configuration#platform-integration). |

For network or API failures, rerun the smallest failing command with `--debug`:

```bash
codex-switch --debug list
codex-switch --debug self-update --check
```

Debug output can contain account or infrastructure identifiers. Before opening an issue, remove tokens, email addresses, account IDs, workspace names, filesystem paths that reveal identity, and proxy credentials.

## Recover a deleted profile

Deletion moves an inactive profile into recoverable storage rather than erasing it. Stop the daemon, move the newest matching directory back into `profiles/`, and confirm that it appears:

```bash
codex-switch daemon stop
# Move deleted-profiles/<alias>.backup-<timestamp> to profiles/<alias>
codex-switch list
```

The base directory is `~/.codex-switch`, `%USERPROFILE%\.codex-switch` on Windows, or the value of `CODEX_SWITCH_HOME`.

## Reset-card outcome is uncertain

If a reset-card request reports that consumption may have occurred, do not immediately retry. Refresh the account state and verify the card count and quota first. This warning means the request reached the service but the client could not prove the final result.

## Report an issue

Include the operating system, terminal, `codex-switch --version`, exact command, expected behavior, actual behavior, and redacted diagnostic output. Use the [GitHub issue tracker](https://github.com/xjoker/codex-switch/issues).

## Next steps

- Check short behavior and security answers in the [FAQ](FAQ).
- Review paths and settings in [Configuration](Configuration).
- If the problem remains, report the redacted reproduction in the [GitHub issue tracker](https://github.com/xjoker/codex-switch/issues).
