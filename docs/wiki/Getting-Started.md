# Getting started

> Canonical source: [README — Quick start](https://github.com/xjoker/codex-switch/blob/master/README.md#quick-start).

Install the stable release:

```bash
curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash
codex-switch login
codex-switch tui
```

On macOS and Linux this installs to `$HOME/.local/bin`. PATH is configured for zsh, bash, and fish; other shells receive a manual instruction. An older direct install under `/usr/local/bin` is migrated once: the new user binary is installed first, then the installer removes the old copy with one elevated operation when required. Administrators can explicitly retain a system-wide install with `--system`.

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.ps1 | iex
codex-switch login
codex-switch tui
```

Codex must use `cli_auth_credentials_store = "file"` in `$CODEX_HOME/config.toml`. Use `codex-switch login --device` on a headless machine.

Existing stable versions `0.0.3` and newer can upgrade with `codex-switch self-update`. Development builds can return to stable with `codex-switch self-update --stable`; versions `0.0.1` and `0.0.2` should rerun the installer. Windows direct installs remain user-owned under `%LOCALAPPDATA%`; Homebrew installations must use Homebrew updates.

To opt into the rolling build before the next stable release, follow [Testing development releases](Development-Releases). Otherwise, continue with the [Feature guide](Feature-Guide) or the full [command reference](https://github.com/xjoker/codex-switch/blob/master/docs/COMMANDS.md).
