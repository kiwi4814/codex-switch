# Development guide

This guide is for engineers and coding agents taking over `codex-switch`. Read [Architecture](ARCHITECTURE.md) before changing authentication, switching, the daemon, or release behavior.

## Prepare the repository

Requirements:

- Rust 1.88 or newer
- Git
- `cargo-audit` for the full local quality gate
- Bash for Unix installer validation; PowerShell for Windows installer validation

```bash
git clone https://github.com/xjoker/codex-switch.git
cd codex-switch
git checkout dev
cargo test --all
```

Development and pull requests normally target `dev`. The `master` branch represents stable releases. Do not confuse the `dev` branch with the rolling `dev` tag.

## Find the change boundary

| Change | Primary files | Related verification |
|---|---|---|
| CLI shape | `src/cli.rs`, `src/commands/` | CLI integration tests and `--help` smoke test |
| Authentication or storage | `src/auth.rs`, `src/profile.rs` | Unit tests plus isolated-home integration tests |
| Usage parsing/API | `src/usage/api.rs`, `src/usage/parse.rs` | Mock HTTP and parser tests |
| Account selection | `src/usage/scoring.rs`, `src/commands/profile.rs` | Pure scoring tests and end-to-end scoring tests |
| TUI behavior | `src/tui/` | State/render unit tests and terminal smoke test |
| Daemon | `src/daemon/` | Unit tests, daemon integration tests, three-host CI |
| Installer/update | `scripts/`, `src/update.rs`, release workflow | Distribution contract tests and release artifact checks |
| Configuration | `src/config.rs`, README configuration section | Parsing/default/warning tests |

Search for the current signature and existing tests before assuming an API or path. Preserve module ownership: command modules coordinate work; domain modules implement reusable behavior.

## Develop behavior test-first

For a bug or a behavior contract, add the smallest test that fails for the intended reason before changing the implementation:

```bash
cargo test descriptive_test_name
# Confirm the new test fails for the expected reason.

# Implement the minimum change.
cargo test descriptive_test_name
```

Pure documentation, configuration-only edits, and visual-only TUI changes do not require an artificial red test. Explain how they were verified instead.

Tests live beside pure module logic and under `tests/` for process, network-mock, daemon, distribution, and scoring integration behavior. Tests that touch user state must override `HOME`, `CODEX_HOME`, or `CODEX_SWITCH_HOME` with isolated temporary directories.

## Run the quality gate

Before opening a pull request:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo audit
bash -n scripts/install.sh
```

On Windows, also parse the installer without executing it:

```powershell
$tokens = $null
$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
  (Resolve-Path 'scripts/install.ps1'),
  [ref]$tokens,
  [ref]$errors
) | Out-Null
if ($errors.Count -gt 0) { $errors; exit 1 }
```

GitHub Actions repeats the core checks on Linux, macOS, and Windows. A local pass does not replace the three-host gate for platform-sensitive changes.

## Preserve safety contracts

- Never log or commit tokens, profile files, emails, account IDs, workspace names, or proxy credentials.
- Validate external JSON and CLI input at their boundaries; do not scatter duplicate internal checks.
- Keep JSON stdout machine-readable. Route progress and diagnostics to stderr.
- Serialize live-auth mutations with `auth.lock` and temporary launch staging with `launch.lock`.
- Use atomic replacement for credentials, cache, and daemon state.
- Keep profile deletion recoverable and refuse deletion of the active profile.
- Do not remove a daemon PID file without proving lock ownership.
- Treat GitHub Release checksums as corruption detection, not an independent signature.

## Update documentation

Behavior changes must update the closest canonical document:

- User-visible commands or behavior: `README.md` and/or `docs/FEATURES.md`
- Module boundaries or data flow: `docs/ARCHITECTURE.md`
- Contributor workflow: `CONTRIBUTING.md` or this guide
- Release behavior: `docs/RELEASE.md`
- Release notes: the Unreleased section in `docs/CHANGELOG.md`

Wiki navigation pages are sourced from `docs/wiki/`. Do not publish a detailed second copy of canonical documentation.

## Diagnose failures

Use a single failing test or command to reduce the feedback loop. If an assumed path, method, or upstream response shape fails, stop and inspect the source or official upstream contract before retrying.

For HTTP behavior, use the existing mock server and response transformers rather than live personal credentials. For platform service behavior, keep platform-specific command construction testable separately from the real service manager.

## Release handoff

Contributors do not move release tags. Maintainers follow [the release process](RELEASE.md), including independent review, local verification, branch CI, GitHub Actions artifact construction, checksum validation, and post-release smoke tests.
