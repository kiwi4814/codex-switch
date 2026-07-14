# Developer onboarding

> Canonical source: [docs/DEVELOPMENT.md](https://github.com/xjoker/codex-switch/blob/dev/docs/DEVELOPMENT.md).

```bash
git clone https://github.com/xjoker/codex-switch.git
cd codex-switch
git checkout dev
cargo test --all
```

Normal development targets `dev`; `master` tracks stable releases. Before changing authentication, switching, daemon, or release behavior, read the [architecture](https://github.com/xjoker/codex-switch/blob/dev/docs/ARCHITECTURE.md).

The [development guide](https://github.com/xjoker/codex-switch/blob/dev/docs/DEVELOPMENT.md) maps change types to source modules and tests, defines the local quality gate, and lists credential and data-safety contracts.

## Next steps

- Learn the system boundaries in [Architecture overview](Architecture-Overview).
- Before opening a pull request, follow [Contributing](Contributing).
