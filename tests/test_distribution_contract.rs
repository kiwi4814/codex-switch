use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_file(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    normalize_line_endings(&text)
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn assert_before(text: &str, first: &str, second: &str) {
    let first_pos = text
        .find(first)
        .unwrap_or_else(|| panic!("missing required content: {first}"));
    let second_pos = text
        .find(second)
        .unwrap_or_else(|| panic!("missing required content: {second}"));
    assert!(
        first_pos < second_pos,
        "expected `{first}` to appear before `{second}`"
    );
}

fn markdown_links(text: &str) -> Vec<&str> {
    let mut links = Vec::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("](") {
        remaining = &remaining[start + 2..];
        let Some(end) = remaining.find(')') else {
            break;
        };
        links.push(remaining[..end].trim());
        remaining = &remaining[end + 1..];
    }
    links
}

fn github_heading_slug(heading: &str) -> String {
    heading
        .trim()
        .trim_end_matches('#')
        .trim()
        .chars()
        .filter_map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                Some(character.to_ascii_lowercase())
            } else if character.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

fn assert_markdown_anchor_exists(path: &Path, anchor: &str) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    assert!(
        text.lines()
            .filter_map(|line| line.strip_prefix('#'))
            .any(|line| {
                let heading = line.trim_start_matches('#').trim();
                github_heading_slug(heading) == anchor
            }),
        "missing anchor `#{anchor}` in {}",
        path.display()
    );
}

#[test]
fn repository_text_normalizes_windows_line_endings() {
    assert_eq!(
        normalize_line_endings("first\r\nsecond\r\n"),
        "first\nsecond\n"
    );
}

#[test]
fn version_file_is_the_release_source_of_truth() {
    let version = repo_file("VERSION").trim().to_string();
    let manifest = repo_file("Cargo.toml");
    let release = repo_file(".github/workflows/release.yml");

    // Pin the documented YYYYMMDD.V.0 shape rather than one literal version, which
    // every release had to edit. This also catches the two forms RELEASE.md warns
    // about: the two-component `20260712.1`, and YYYYDDMM, which sorts wrongly.
    let (date, rest) = version.split_once('.').expect("version needs a date part");
    let (sequence, patch) = rest.split_once('.').expect("version must be YYYYMMDD.V.0");
    assert!(
        date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()),
        "version must start with an 8-digit YYYYMMDD date, got {date:?}"
    );
    let month: u32 = date[4..6].parse().expect("month must be numeric");
    let day: u32 = date[6..8].parse().expect("day must be numeric");
    assert!(
        (1..=12).contains(&month) && (1..=31).contains(&day),
        "version date must be YYYYMMDD, got month {month} day {day} in {date:?}"
    );
    assert!(
        !sequence.is_empty()
            && sequence.chars().all(|c| c.is_ascii_digit())
            && !sequence.starts_with('0'),
        "release sequence must be a positive integer starting at 1, got {sequence:?}"
    );
    assert_eq!(patch, "0", "the third component is always 0 for SemVer");
    assert!(manifest.contains(&format!("version = \"{version}\"")));
    assert!(release.contains("BASE=$(cat VERSION)"));
    assert!(!release.contains("BASE=$(grep '^version' Cargo.toml"));
}

#[test]
fn ci_covers_dev_and_all_supported_hosts() {
    let workflow = repo_file(".github/workflows/ci.yml");

    for required in [
        "push:",
        "pull_request:",
        "workflow_dispatch:",
        "dev",
        "master",
        "ubuntu-latest",
        "macos-latest",
        "windows-latest",
    ] {
        assert!(
            workflow.contains(required),
            "CI workflow must contain `{required}`"
        );
    }
}

#[test]
fn ci_runs_build_test_lint_format_audit_and_script_parsers() {
    let workflow = repo_file(".github/workflows/ci.yml");

    for command in [
        "cargo test --all",
        "cargo clippy --all-targets -- -D warnings",
        "cargo build",
        "cargo fmt --check",
        "cargo audit",
        "bash -n scripts/install.sh",
    ] {
        assert!(
            workflow.contains(command),
            "CI workflow must execute `{command}`"
        );
    }
    assert!(
        workflow.contains("Parser]::ParseFile") && workflow.contains("scripts/install.ps1"),
        "Windows CI must parse install.ps1 with the PowerShell parser"
    );
}

#[test]
fn wiki_sync_uses_the_reviewed_repository_sources() {
    let workflow = repo_file(".github/workflows/wiki.yml");

    for required in [
        "workflow_dispatch:",
        "branches: [dev]",
        "docs/wiki/**",
        "permissions:",
        "contents: write",
        "${{ secrets.GITHUB_TOKEN }}",
        "${GITHUB_REPOSITORY}.wiki.git",
        "refs/heads/dev:refs/remotes/origin/dev",
        "git diff --quiet \"$GITHUB_SHA\" refs/remotes/origin/dev -- docs/wiki .github/workflows/wiki.yml",
        "for page in docs/wiki/*.md; do",
        "[[ -f \"$page\" && ! -L \"$page\" ]]",
        "cp -- \"$page\" \"$wiki_dir\"/",
        "push origin HEAD:master",
    ] {
        assert!(
            workflow.contains(required),
            "Wiki workflow must contain `{required}`"
        );
    }
    assert!(!workflow.contains("WIKI_TOKEN"));
    assert!(!workflow.contains("pull_request:"));
    assert!(!workflow.contains("branches: [dev, master]"));
}

#[test]
fn wiki_links_resolve_to_reviewed_pages_and_sources() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let wiki_dir = root.join("docs/wiki");
    let page_slugs: BTreeSet<String> = fs::read_dir(&wiki_dir)
        .expect("failed to list Wiki sources")
        .map(|entry| entry.expect("failed to read Wiki source entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    let repository_prefix = "https://github.com/xjoker/codex-switch/";

    for entry in fs::read_dir(&wiki_dir).expect("failed to list Wiki sources") {
        let path = entry.expect("failed to read Wiki source entry").path();
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for link in markdown_links(&text) {
            if let Some(target) = link.strip_prefix(repository_prefix) {
                let Some(target) = target
                    .strip_prefix("blob/dev/")
                    .or_else(|| target.strip_prefix("tree/dev/"))
                else {
                    if target.starts_with("blob/") || target.starts_with("tree/") {
                        panic!(
                            "{} links to an unreviewed repository branch: {link}",
                            path.display()
                        );
                    }
                    continue;
                };
                let (target_path, anchor) = target
                    .split_once('#')
                    .map_or((target, None), |(file, anchor)| (file, Some(anchor)));
                let local_path = root.join(target_path);
                assert!(
                    local_path.exists(),
                    "{} links to missing repository source: {link}",
                    path.display()
                );
                if let Some(anchor) = anchor {
                    assert_markdown_anchor_exists(&local_path, anchor);
                }
                continue;
            }
            if let Some(anchor) = link.strip_prefix('#') {
                assert_markdown_anchor_exists(&path, anchor);
                continue;
            }
            if link.contains("://") {
                continue;
            }

            let (page, anchor) = link
                .split_once('#')
                .map_or((link, None), |(page, anchor)| (page, Some(anchor)));
            let slug = page.trim_end_matches(".md");
            assert!(
                page_slugs.contains(slug),
                "{} links to missing Wiki page: {link}",
                path.display()
            );
            if let Some(anchor) = anchor {
                assert_markdown_anchor_exists(&wiki_dir.join(format!("{slug}.md")), anchor);
            }
        }
    }
}

#[test]
fn wiki_navigation_is_task_oriented_and_progressive() {
    let home = repo_file("docs/wiki/Home.md");
    let sidebar = repo_file("docs/wiki/_Sidebar.md");

    for required in ["## Start here", "## Choose your task", "## Contribute"] {
        assert!(
            home.contains(required),
            "Wiki Home must contain `{required}`"
        );
    }
    for required in [
        "## Start here",
        "## Use codex-switch",
        "## Get help",
        "## Contribute",
    ] {
        assert!(
            sidebar.contains(required),
            "Wiki sidebar must contain `{required}`"
        );
    }
    assert!(!sidebar.contains("### Canonical sources"));

    for page in [
        "Architecture-Overview.md",
        "Chinese-Guide.md",
        "Command-Reference.md",
        "Configuration.md",
        "Contributing.md",
        "Developer-Onboarding.md",
        "Development-Releases.md",
        "FAQ.md",
        "Feature-Guide.md",
        "Getting-Started.md",
        "Troubleshooting.md",
        "Updating.md",
    ] {
        assert!(
            repo_file(&format!("docs/wiki/{page}")).contains("## Next steps"),
            "Wiki page {page} must end with explicit next steps"
        );
    }
}

#[test]
fn release_rejects_shell_metacharacters_in_tags_and_uses_env_in_scripts() {
    let workflow = repo_file(".github/workflows/release.yml");

    assert!(workflow.contains("TAG_PATTERN="));
    assert!(workflow.contains("BASE_PATTERN=\"${BASE//./\\\\.}\""));
    assert!(workflow.contains("[[ ! \"$TAG\" =~ $TAG_PATTERN ]]"));
    assert!(!workflow.contains("VERSION=\"${{ needs.meta.outputs.version }}\""));
    assert!(!workflow.contains("${{ github.ref }}` at ${{ github.sha }}"));
    assert!(workflow.contains("RELEASE_VERSION: ${{ needs.meta.outputs.version }}"));
    assert!(workflow.contains("persist-credentials: false"));
}

#[test]
fn unix_installer_verifies_checksum_before_extracting() {
    let script = repo_file("scripts/install.sh");

    assert!(script.contains("${DOWNLOAD_URL}.sha256"));
    assert!(script.contains("EXPECTED_SHA256"));
    assert!(script.contains("sha256sum") && script.contains("shasum -a 256"));
    assert_before(&script, "EXPECTED_SHA256", "tar xzf");
    for required in [
        "USER_INSTALL_DIR=\"${HOME}/.local/bin\"",
        "SYSTEM_INSTALL_DIR=\"/usr/local/bin\"",
        "--system",
        "LEGACY_BIN",
        "install -m 0755",
        "sudo install -m 0755",
    ] {
        assert!(
            script.contains(required),
            "Unix installer must contain `{required}`"
        );
    }
}

#[test]
fn unix_installer_preserves_migration_and_path_lifecycle() {
    let script = repo_file("scripts/install.sh");

    for required in [
        "*/fish)",
        "PROFILE_FILE=\"${HOME}/.config/fish/config.fish\"",
        "# >>> codex-switch PATH >>>",
        "# <<< codex-switch PATH <<<",
        "remove_managed_path_blocks",
        "remove_path_block \"${HOME}/.zprofile\"",
        "remove_path_block \"${HOME}/.bash_profile\"",
        "remove_path_block \"${HOME}/.profile\"",
        "remove_path_block \"${HOME}/.config/fish/config.fish\"",
        "!seen_begin || !seen_end || inside",
    ] {
        assert!(
            script.contains(required),
            "Unix installer must contain `{required}`"
        );
    }

    assert_before(&script, "tar xzf", "sudo -v");
    assert_before(
        &script,
        "mkdir -p \"$INSTALL_DIR\"",
        "sudo rm -f \"$LEGACY_BIN\"",
    );
    assert!(script.contains(
        "if [ \"$SYSTEM_INSTALL\" = false ]; then\n    remove_managed_path_blocks\n  fi"
    ));
}

#[test]
fn unix_installer_records_and_cleans_explicit_system_install_intent() {
    let script = repo_file("scripts/install.sh");

    for required in [
        "SYSTEM_INSTALL_MARKER",
        ".codex-switch-system-install-v1",
        "install -m 0644 /dev/null \"$SYSTEM_INSTALL_MARKER\"",
        "sudo install -m 0644 /dev/null \"$SYSTEM_INSTALL_MARKER\"",
        "rm -f \"$LEGACY_BIN\" \"$SYSTEM_INSTALL_MARKER\"",
        "sudo rm -f \"$LEGACY_BIN\" \"$SYSTEM_INSTALL_MARKER\"",
    ] {
        assert!(
            script.contains(required),
            "Unix installer must preserve system-install marker lifecycle: `{required}`"
        );
    }
}

#[test]
fn windows_installer_verifies_checksum_before_extracting() {
    let script = repo_file("scripts/install.ps1");

    assert!(script.contains("$ChecksumUrl"));
    assert!(script.contains("Get-FileHash"));
    assert!(script.contains("SHA256"));
    assert_before(&script, "Get-FileHash", "Expand-Archive");
    assert!(
        script.contains("Checksum mismatch"),
        "Windows installer must fail clearly on checksum mismatch"
    );
    assert!(script.contains("$env:LOCALAPPDATA"));
    assert!(script.contains("SetEnvironmentVariable(\"Path\", $NewPath, \"User\")"));
}

#[test]
fn self_update_checks_replace_permission_before_archive_download() {
    let update = repo_file("src/update.rs");

    assert_before(
        &update,
        "ensure_replace_parent_writable(&executable, platform, &release.tag_name)?",
        "download_file(&client, &archive_asset.browser_download_url",
    );
    assert!(!update.contains("permission denied? try: sudo codex-switch self-update"));
    assert!(!update.contains("retry from PowerShell as Administrator"));
}

#[test]
fn self_update_gates_markerless_system_installs_before_network_checks() {
    let command = repo_file("src/commands/update.rs");

    assert_before(
        &command,
        "ensure_legacy_system_install_migrated(use_dev, version)",
        "if check",
    );
}

#[test]
fn release_notes_prompt_legacy_users_to_run_the_matching_installer() {
    let workflow = repo_file(".github/workflows/release.yml");

    assert!(workflow.contains("Older macOS/Linux direct install"));
    assert!(workflow.contains("releases/download/dev/install.sh | bash -s -- --dev"));
    assert!(
        workflow.contains("releases/download/v${{ needs.meta.outputs.version }}/install.sh | bash")
    );
}

#[test]
fn release_verifies_archives_before_creating_a_release() {
    let workflow = repo_file(".github/workflows/release.yml");

    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(workflow.contains("release:\n") && workflow.contains("contents: write"));
    for archive in [
        "cs-linux-amd64.tar.gz",
        "cs-linux-arm64.tar.gz",
        "cs-darwin-amd64.tar.gz",
        "cs-darwin-arm64.tar.gz",
        "cs-windows-amd64.zip",
        "cs-windows-arm64.zip",
    ] {
        assert!(
            workflow.contains(archive),
            "release verification must require `{archive}`"
        );
    }
    assert!(workflow.contains("sha256sum --check"));
    assert_before(
        &workflow,
        "Verify release checksums",
        "Create GitHub Release (dev)",
    );
    assert_before(
        &workflow,
        "Verify release checksums",
        "Create GitHub Release (stable)",
    );
}

#[test]
fn release_retests_v0019_upgrade_on_all_supported_hosts() {
    let workflow = repo_file(".github/workflows/release.yml");

    for required in [
        "legacy-upgrade:",
        "needs: [meta, release]",
        "if: needs.meta.outputs.is_dev == 'true' || needs.meta.outputs.prerelease == 'false'",
        "ubuntu-latest",
        "macos-latest",
        "windows-latest",
        "releases/download/v0.0.19",
        "self-update --dev",
        "self-update --stable",
        "@(& $bin --version)[0]",
        "3589fdac3d480aea83ab61dd4fb0a7592c018a44415842083df2d5f1d0bb0d2f",
        "5db981cc5f1380f3bf9ac2d66484c0cc67d06712e617c2cd0457e3058dcb12b0",
        "cbc4229285c5e8ea02c9463b7868cd03f2021f5125f5b187ff99e3bebe16e278",
        "2e365dc8273c04ee634d593eeada338a46ba7c7db2a1ca8f5c2aff30aec57a98",
        "9ff8a3f8794517771ef6959632417fdf1dfbc85b7fdf4c344998223985582e63",
    ] {
        assert!(
            workflow.contains(required),
            "Release workflow must test legacy upgrade contract: `{required}`"
        );
    }
}

#[test]
fn legacy_upgrade_retries_authenticated_metadata_during_github_release_propagation() {
    let workflow = repo_file(".github/workflows/release.yml");

    for required in [
        "for attempt in 1 2 3 4 5; do",
        "foreach ($attempt in 1..5)",
        "Retrying authenticated release metadata fetch after propagation delay",
        "sleep 5",
        "Start-Sleep -Seconds 5",
    ] {
        assert!(
            workflow.contains(required),
            "legacy upgrade must tolerate GitHub Release propagation: `{required}`"
        );
    }
}

#[test]
fn legacy_upgrade_uses_authenticated_local_release_metadata() {
    let workflow = repo_file(".github/workflows/release.yml");

    for required in [
        "GH_TOKEN: ${{ github.token }}",
        "gh api",
        "unset GH_TOKEN",
        "Remove-Item Env:GH_TOKEN -ErrorAction SilentlyContinue",
        "CS_GITHUB_API_URL=http://127.0.0.1:8765",
        "python3 -m http.server 8765 --bind 127.0.0.1",
        "\"-m\", \"http.server\", \"8765\", \"--bind\", \"127.0.0.1\"",
        "macos-26",
    ] {
        assert!(
            workflow.contains(required),
            "legacy upgrade must avoid anonymous GitHub API access: `{required}`"
        );
    }

    assert!(
        !workflow.contains("os: [ubuntu-latest, macos-latest, windows-latest]"),
        "legacy upgrade must pin the macOS 26 runner during the latest-label migration"
    );
}

#[test]
fn dev_release_uses_the_short_calendar_prerelease_version() {
    let workflow = repo_file(".github/workflows/release.yml");

    assert!(workflow.contains("version=${BASE}-dev"));
    assert!(!workflow.contains("TIMESTAMP"));
    assert!(!workflow.contains("-dev.${TIMESTAMP}"));
}

#[test]
fn readmes_describe_current_cli_and_codex_requirements() {
    for path in ["README.md", "README_CN.md"] {
        let readme = repo_file(path);
        assert!(!readme.contains("use --force"), "stale command in {path}");
        assert!(!readme.contains("codex --quiet"), "stale command in {path}");
        for required in [
            "self-update --stable",
            "self-update --version",
            "Task Scheduler",
            "cli_auth_credentials_store",
            "CODEX_HOME",
            "forced_login_method",
            "cache_refresh_interval_secs",
            "auto_warmup",
        ] {
            assert!(
                readme.contains(required),
                "{path} must document `{required}`"
            );
        }
    }
}

#[test]
fn installer_instructions_use_channel_matched_release_assets() {
    let stable_unix = "https://github.com/xjoker/codex-switch/releases/latest/download/install.sh";
    let stable_windows =
        "https://github.com/xjoker/codex-switch/releases/latest/download/install.ps1";
    let dev_unix = "https://github.com/xjoker/codex-switch/releases/download/dev/install.sh";
    let dev_windows = "https://github.com/xjoker/codex-switch/releases/download/dev/install.ps1";

    for path in [
        "README.md",
        "README_CN.md",
        "scripts/install.sh",
        "scripts/install.ps1",
        ".github/workflows/release.yml",
    ] {
        let text = repo_file(path);
        assert!(
            !text.contains("raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install"),
            "{path} must not direct users to the stale installer on the master branch"
        );
    }

    for path in ["README.md", "README_CN.md"] {
        let readme = repo_file(path);
        for required in [stable_unix, stable_windows, dev_unix, dev_windows] {
            assert!(
                readme.contains(required),
                "{path} must contain channel-matched installer URL `{required}`"
            );
        }
    }

    let unix_installer = repo_file("scripts/install.sh");
    assert!(unix_installer.contains(stable_unix));
    assert!(unix_installer.contains(dev_unix));

    let windows_installer = repo_file("scripts/install.ps1");
    assert!(windows_installer.contains(stable_windows));
    assert!(windows_installer.contains(dev_windows));

    let workflow = repo_file(".github/workflows/release.yml");
    assert!(workflow.contains(stable_unix));
    assert!(workflow.contains(stable_windows));
    assert!(workflow.contains(dev_unix));
    assert!(workflow.contains(dev_windows));
}

#[test]
fn self_update_help_limits_automatic_checks_to_tui_startup() {
    let cli = repo_file("src/cli.rs");

    assert!(cli.contains("Only the TUI checks automatically at startup"));
    assert!(cli.contains("Other commands never check automatically"));
}

#[test]
fn plain_self_update_keeps_dev_installs_on_the_dev_channel() {
    let command = repo_file("src/commands/update.rs");

    assert!(command.contains("update::is_dev_version(update::current_version())"));
    assert!(command.contains("update::check_for_dev_update().await?"));
    assert!(command.contains("update::self_update_dev(show_progress).await"));
    assert!(command.contains("else if stable || version.is_some()"));
    assert_before(&command, "if dev", "else if stable || version.is_some()");
}

#[test]
fn release_docs_describe_platform_specific_archive_formats() {
    let release = repo_file("docs/RELEASE.md");

    assert!(
        release.contains("Linux / macOS") && release.contains("`.tar.gz`"),
        "release docs must describe Unix tar.gz artifacts"
    );
    assert!(
        release.contains("Windows") && release.contains("`.zip`"),
        "release docs must describe Windows zip artifacts"
    );
    assert!(
        !release.contains("6 平台 tarball"),
        "release docs must not call Windows zip artifacts tarballs"
    );
}

#[test]
fn changelog_tracks_the_calendar_version_development_cycle() {
    let changelog = repo_file("docs/CHANGELOG.md");
    assert!(
        changelog.contains("## v20260713.2.0 — 2026-07-13"),
        "the final dev candidate must carry the stable release heading before zero-drift acceptance"
    );
}
