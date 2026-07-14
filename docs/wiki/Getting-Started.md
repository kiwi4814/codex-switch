# Getting started

> **Required:** Codex must use `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml`.
>
> Canonical source: [README — Quick start](https://github.com/xjoker/codex-switch/blob/dev/README.md#quick-start).

Install the stable release:

```bash
curl -fsSL https://github.com/xjoker/codex-switch/releases/latest/download/install.sh | bash
codex-switch login
codex-switch tui
```

On macOS and Linux this installs to `$HOME/.local/bin`. PATH is configured for zsh, bash, and fish; other shells receive a manual instruction. An older direct install under `/usr/local/bin` is migrated once: the new user binary is installed first, then the installer removes the old copy with one elevated operation when required. Administrators can explicitly retain a system-wide install with `--system`.

> If the installer says `Installing to /usr/local/bin (requires sudo)` without an explicit `--system`, stop it: that is the retired script from the repository's old `master` branch. Use the Release URL above. The current script may ask for `sudo` once only when it must remove a root-owned legacy binary from `/usr/local/bin`; it still installs the replacement under `$HOME/.local/bin`.

Windows PowerShell:

```powershell
irm https://github.com/xjoker/codex-switch/releases/latest/download/install.ps1 | iex
codex-switch login
codex-switch tui
```

Use `codex-switch login --device` on a headless machine.

Existing stable versions `0.0.3` and newer can upgrade with `codex-switch self-update`. Development builds can return to stable with `codex-switch self-update --stable`; versions `0.0.1` and `0.0.2` should rerun the installer. Windows direct installs remain user-owned under `%LOCALAPPDATA%`; Homebrew installations must use Homebrew updates.

## Next steps

- Learn account, quota, launch, and daemon workflows in the [Feature guide](Feature-Guide).
- Opt into the next release with [Testing development releases](Development-Releases).
- Look up exact syntax in the [command reference](https://github.com/xjoker/codex-switch/blob/dev/docs/COMMANDS.md).
