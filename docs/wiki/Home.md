# codex-switch Wiki

`codex-switch` manages multiple local OpenAI Codex CLI logins, shows quota state, and selects an account for the next Codex session.

> **Required:** Codex must use `cli_auth_credentials_store = "file"`. Start with [Getting started](Getting-Started) before importing or switching accounts.

## Start here

- New users: [install codex-switch and add the first account](Getting-Started).
- Existing users: [choose a task](#choose-your-task).
- 中文读者：[从中文指南开始](Chinese-Guide)。

## Choose your task

| I want to… | Start here |
|---|---|
| Add, import, inspect, or switch accounts | [Feature guide](Feature-Guide) |
| Install or test the rolling `dev` build | [Testing development releases](Development-Releases) |
| Diagnose an error or recover a profile | [Troubleshooting](Troubleshooting) |
| Check a short behavior or security answer | [FAQ](FAQ) |
| Read the complete command and configuration docs | [Documentation index](https://github.com/xjoker/codex-switch/blob/dev/docs/README.md) |

## Contribute

1. [Prepare a development environment](Developer-Onboarding).
2. [Understand state ownership and safety boundaries](Architecture-Overview).
3. [Follow the contribution and verification contract](Contributing).

## Documentation model

The Wiki is a concise navigation layer. Detailed behavior lives in the reviewed [`dev` branch documentation](https://github.com/xjoker/codex-switch/tree/dev/docs), which is published alongside these Wiki sources. Stable installers and binaries come from [GitHub Releases](https://github.com/xjoker/codex-switch/releases).

Do not publish auth files, profile files, tokens, unredacted debug output, proxy credentials, account IDs, email addresses, or workspace names.
