# Troubleshooting

> Canonical source: [Troubleshooting](https://github.com/xjoker/codex-switch/blob/dev/docs/TROUBLESHOOTING.md).

Start with the complete error, its file path, and the command that produced it.

- Credential-store errors: set `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml`.
- No profiles: run `codex-switch login` or `codex-switch import <path>`.
- Windows daemon access denied: use elevated PowerShell for install or uninstall.
- Broken TUI in Git Bash: use Windows Terminal or PowerShell.
- Mistaken deletion: stop the daemon and restore the newest matching directory from `deleted-profiles/`.

For network failures, rerun with `--debug`, then redact tokens, emails, account IDs, workspace names, and proxy credentials before sharing output.

## Next steps

- Check short behavior and security answers in the [FAQ](FAQ).
- For full recovery procedures, open the [canonical troubleshooting guide](https://github.com/xjoker/codex-switch/blob/dev/docs/TROUBLESHOOTING.md).
- If the problem remains, report the redacted reproduction in the [GitHub issue tracker](https://github.com/xjoker/codex-switch/issues).
