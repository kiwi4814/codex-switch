# GitHub Wiki maintenance

The `codex-switch` Wiki is a curated entry point for users, contributors, and coding agents. Canonical technical content remains in the main repository so it can be reviewed in pull requests, protected with the code, and included in an ordinary clone.

## Documentation decision

Three approaches were evaluated against discoverability, review history, clone access, and maintenance cost:

| Approach | Decision | Reason |
|---|---|---|
| Repository documents only | Not selected | Strongest review and clone behavior, but misses the requested Wiki navigation and Wiki search experience. |
| Wiki as the primary documentation | Rejected | The Wiki is a separate Git repository and can drift from reviewed code; a normal source clone does not include it. |
| Canonical repository documents plus Wiki navigation | Selected | Keeps one reviewed source of truth while providing a task-oriented Wiki entry point. |

GitHub documents Wiki page history, diffs, reverts, sidebars, and local Git editing. It does not document the Wiki as having the same pull request and branch-protection workflow as the main repository, so this project does not rely on that assumption.

Official references, checked 2026-07-13:

- [About wikis](https://docs.github.com/en/communities/documenting-your-project-with-wikis/about-wikis)
- [Adding or editing Wiki pages](https://docs.github.com/en/communities/documenting-your-project-with-wikis/adding-or-editing-wiki-pages)
- [Viewing Wiki history](https://docs.github.com/en/communities/documenting-your-project-with-wikis/viewing-a-wikis-history-of-changes)
- [Creating a Wiki sidebar or footer](https://docs.github.com/en/communities/documenting-your-project-with-wikis/creating-a-footer-or-sidebar-for-your-wiki)
- [Searching wikis](https://docs.github.com/en/search-github/searching-on-github/searching-wikis)
- [About README files](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes)
- [Setting contributor guidelines](https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/setting-guidelines-for-repository-contributors)

## Edit the Wiki source

Wiki source pages live in `docs/wiki/`. Keep them concise and link to canonical documents for details. Each page should answer one navigation question and identify the canonical source.

English is the primary Wiki language. A Chinese companion page may provide a quick path for Chinese readers, but detailed behavior remains in the English repository documents. When behavior changes, update the canonical English document first, then adjust the companion summary if its navigation or examples are affected.

Do not edit the published Wiki directly except to recover from a publishing failure. A direct fix must be copied back to `docs/wiki/` immediately.

## Publish the Wiki

The Wiki repository has been initialized with its first `Home` page. `.github/workflows/wiki.yml` publishes the reviewed `docs/wiki/*.md` sources automatically when they change on `dev`, the single Wiki publication branch. A maintainer can also run the `Sync Wiki` workflow manually from `dev`.

The workflow uses the job-scoped `GITHUB_TOKEN` with only `contents: write`; no long-lived personal access token is configured. It replaces the Wiki pages with the reviewed repository sources and commits only when content changed. Before publishing, it compares the run against the latest `dev` Wiki sources and skips stale runs.

Do not push a local Wiki clone during the normal documentation workflow. If automation fails, inspect the `Sync Wiki` run before considering a manual recovery push.

## Verify publication

After the workflow succeeds:

1. Open the Wiki Home page and each sidebar link.
2. Confirm canonical links resolve to the default branch.
3. Confirm `_Sidebar.md` renders as navigation rather than a normal page.
4. Compare the Wiki commit with the reviewed `docs/wiki/` source.
5. Record the Wiki sync in the release checklist when documentation changed.
