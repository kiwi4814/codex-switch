# Documentation

This directory is the canonical documentation for `codex-switch`. Documentation changes should be reviewed with the code they describe. The GitHub Wiki provides a curated navigation layer; it is not a second source of truth.

## Choose a starting point

| Reader | Start here | Continue with |
|---|---|---|
| New user | [README](../README.md#quick-start) | [Feature guide](FEATURES.md) |
| Operator | [Configuration](CONFIGURATION.md) | [Troubleshooting](TROUBLESHOOTING.md) |
| Contributor | [Contributing](../CONTRIBUTING.md) | [Development guide](DEVELOPMENT.md) |
| Maintainer | [Architecture](ARCHITECTURE.md) | [Release process](RELEASE.md) |
| Release reader | [Changelog](CHANGELOG.md) | [GitHub Releases](https://github.com/xjoker/codex-switch/releases) |

## Canonical documents

- [Feature guide](FEATURES.md) explains user-visible behavior, safety boundaries, and the main workflows.
- [Command reference](COMMANDS.md) lists commands, global flags, and automation contracts.
- [Configuration](CONFIGURATION.md) documents paths, settings, proxy precedence, and platform behavior.
- [Troubleshooting](TROUBLESHOOTING.md) provides safe diagnostics and recovery procedures.
- [Architecture](ARCHITECTURE.md) explains data ownership, module boundaries, switching, refresh, and daemon flows.
- [Development guide](DEVELOPMENT.md) covers the local toolchain, test layout, common change paths, and verification.
- [Contributing](../CONTRIBUTING.md) defines the pull request contract.
- [Release process](RELEASE.md) defines the maintainer-only release gates.
- [Changelog](CHANGELOG.md) records release-level behavior changes.
- [Wiki maintenance](WIKI.md) explains why the Wiki is a navigation layer and how to publish it.

## Documentation contract

- Write documentation in English.
- Keep warnings and prerequisites near the top.
- Describe observed behavior, not planned behavior.
- Link to source files for implementation details that may change.
- Update the relevant canonical document in the same pull request as a behavior change.
- Do not edit the published Wiki as an independent copy; update `docs/wiki/` and sync it.
