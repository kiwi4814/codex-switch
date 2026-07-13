# Architecture overview

> Canonical source: [docs/ARCHITECTURE.md](https://github.com/xjoker/codex-switch/blob/master/docs/ARCHITECTURE.md).

`codex-switch` is a single Rust binary. `CODEX_HOME` owns the live Codex `auth.json`; `CODEX_SWITCH_HOME` owns saved profiles, cache, configuration, locks, logs, and daemon state.

The command layer coordinates focused modules for authentication, profiles, usage, login, updates, TUI, and daemon behavior. Live-auth writes are atomic and serialized by a dedicated lock. Usage parsing and account scoring are separated so the CLI and daemon share the same decision logic.

See the [full architecture document](https://github.com/xjoker/codex-switch/blob/master/docs/ARCHITECTURE.md) for the data-flow diagram, module map, state layout, safety invariants, and release architecture.
