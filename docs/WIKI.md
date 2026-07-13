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

Do not edit the published Wiki directly except to recover from a publishing failure. A direct fix must be copied back to `docs/wiki/` immediately.

## Initialize the Wiki

GitHub requires the first page to be created in the web interface before the separate Wiki Git repository exists. After creating the initial `Home` page, publish the reviewed sources:

```bash
git clone https://github.com/xjoker/codex-switch.wiki.git ../codex-switch.wiki
cp docs/wiki/*.md ../codex-switch.wiki/
git -C ../codex-switch.wiki add --all
git -C ../codex-switch.wiki commit -m "docs: sync project wiki"
git -C ../codex-switch.wiki push origin HEAD
```

Pushing the Wiki is a remote publication and requires maintainer authorization. Review the source-repository diff before syncing.

## Verify publication

After the push:

1. Open the Wiki Home page and each sidebar link.
2. Confirm canonical links resolve to the default branch.
3. Confirm `_Sidebar.md` renders as navigation rather than a normal page.
4. Compare the Wiki commit with the reviewed `docs/wiki/` source.
5. Record the Wiki sync in the release checklist when documentation changed.

Manual sync is intentionally used for the first version. Automated token or workflow publication has not been adopted because it adds credentials and failure modes without removing the need to review canonical documentation.
