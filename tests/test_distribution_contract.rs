use std::fs;
use std::path::PathBuf;

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

#[test]
fn repository_text_normalizes_windows_line_endings() {
    assert_eq!(
        normalize_line_endings("first\r\nsecond\r\n"),
        "first\nsecond\n"
    );
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
        "ensure_legacy_system_install_migrated(use_dev, version)?",
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
fn legacy_upgrade_retries_during_github_release_propagation() {
    let workflow = repo_file(".github/workflows/release.yml");

    for required in [
        "for attempt in 1 2 3 4 5; do",
        "foreach ($attempt in 1..5)",
        "Retrying legacy self-update after GitHub Release propagation delay",
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
