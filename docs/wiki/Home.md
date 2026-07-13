# codex-switch Wiki

`codex-switch` is the multi-account manager for OpenAI Codex CLI. It stores multiple local file-backed Codex logins, monitors quota, selects the best account for the next session, and offers CLI, TUI, daemon, import, warmup, reset-card, and self-update workflows.

> Canonical documentation lives in the [main repository](https://github.com/xjoker/codex-switch/tree/master/docs). Wiki pages are curated entry points and should not be treated as a second implementation specification.

## Using codex-switch

- [Getting started](Getting-Started) — install, add an account, and open the dashboard.
- [Testing development releases](Development-Releases) — install the rolling dev build, verify it, and return to stable.
- [Feature guide](Feature-Guide) — understand the supported workflows and safety boundaries.
- [Troubleshooting](Troubleshooting) — diagnose common failures and recover a profile.
- [FAQ](FAQ) — concise answers to user and project questions.

中文读者可以从[中文指南](Chinese-Guide)开始；详细行为仍以英文主文档为准。

## Developing codex-switch

- [Architecture overview](Architecture-Overview) — understand state ownership and module flow.
- [Developer onboarding](Developer-Onboarding) — prepare a development environment and find the right change boundary.
- [Contributing](Contributing) — tests, pull request expectations, and documentation rules.

## Important prerequisite

Codex must use its file credential store because `codex-switch` switches `$CODEX_HOME/auth.json`. Set this in `$CODEX_HOME/config.toml`:

```toml
cli_auth_credentials_store = "file"
```

Do not publish auth files, profile files, tokens, unredacted debug output, proxy credentials, account IDs, email addresses, or workspace names.
