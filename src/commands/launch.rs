use crate::output::{print_json, user_println};
use crate::{auth, config, profile};
use anyhow::{Context, Result};

pub(crate) async fn launch_cmd(
    alias: Option<&str>,
    args: Vec<String>,
    json: bool,
    consume_card: bool,
) -> Result<()> {
    use std::io::IsTerminal;

    let mut revival_hint = None;
    let target_alias = match alias {
        Some(alias) => {
            let profiles = profile::list_profiles()?;
            if !profiles.iter().any(|profile| profile == alias) {
                anyhow::bail!("Profile '{}' not found", alias);
            }
            alias.to_string()
        }
        None => {
            let card_policy = if consume_card {
                super::profile::CardPolicy::PreApproved
            } else if !json && std::io::stdin().is_terminal() {
                super::profile::CardPolicy::Prompt
            } else {
                super::profile::CardPolicy::Deny
            };
            let outcome = super::profile::select_best_profile(json, card_policy).await?;
            revival_hint = outcome.revival_hint;
            outcome.alias
        }
    };
    if let Some(hint) = &revival_hint
        && !json
    {
        user_println(&super::profile::revival_hint_message(hint));
    }

    match std::process::Command::new("codex")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(_) => {}
        Err(_) => anyhow::bail!("codex not found in PATH. Install: npm install -g @openai/codex"),
    }

    let codex_auth = auth::codex_auth_path()?;
    // Unique per-invocation backup name (PID + timestamp): prevents two
    // concurrent `launch` commands from clobbering each other's backup.
    let backup = codex_auth.with_extension(format!(
        "json.bak.{}.{}",
        std::process::id(),
        auth::now_unix_secs()
    ));

    // The dedicated launch lease covers only stage -> process start -> short
    // read window -> restore. It does not hold the auth write lock or wait for
    // the interactive child to exit.
    let launch_lease = tokio::task::spawn_blocking(profile::lock_launch_session)
        .await
        .context("launch lease task panicked")?
        .context("acquiring launch session lease")?;
    // All codex-switch writers acquire this lease before mutating live auth,
    // so the existence snapshot cannot race a concurrent switch.
    let had_original = codex_auth.exists();

    // Swap auth.json → start codex → wait for it to read auth → restore.
    // Codex CLI reads auth.json only at startup, so we only need to hold
    // the swapped state for a few seconds, not the entire session.
    let stage_result = {
        let codex_auth2 = codex_auth.clone();
        let backup2 = backup.clone();
        let target_alias2 = target_alias.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let _lock = profile::lock_live_auth().context("acquiring auth lock")?;

            if had_original {
                std::fs::copy(&codex_auth2, &backup2)
                    .with_context(|| format!("backing up {}", codex_auth2.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&backup2, std::fs::Permissions::from_mode(0o600));
                }
            }

            profile::stage_profile_auth(&target_alias2)?;
            Ok(())
        })
        .await
        .context("lock task panicked")?
    };
    if let Err(stage_err) = stage_result {
        if backup.exists() || !had_original {
            let codex_auth2 = codex_auth.clone();
            let backup2 = backup.clone();
            let alias2 = target_alias.clone();
            tokio::task::spawn_blocking(move || {
                restore_launch_auth(&codex_auth2, &backup2, had_original, &alias2)
            })
            .await
            .context("restore task panicked after launch staging failure")??;
        }
        drop(launch_lease);
        return Err(stage_err).context("staging launch auth");
    }
    // The auth lock is released here; the launch lease keeps other live-auth
    // writers out until the staged file is restored.

    if !json {
        user_println(&format!("Launching codex with profile '{target_alias}'..."));
    }

    let child_result = std::process::Command::new("codex")
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn();

    let mut child = match child_result {
        Ok(child) => child,
        Err(spawn_err) => {
            let codex_auth2 = codex_auth.clone();
            let backup2 = backup.clone();
            let alias2 = target_alias.clone();
            tokio::task::spawn_blocking(move || {
                restore_launch_auth(&codex_auth2, &backup2, had_original, &alias2)
            })
            .await
            .context("restore task panicked after Codex spawn failure")??;
            drop(launch_lease);
            return Err(spawn_err).context("Failed to start codex");
        }
    };

    // Give codex time to read auth.json, then restore immediately.
    // Configurable via [launch] restore_delay_secs (default: 3).
    let delay = config::get().launch.restore_delay_secs;
    // If the user interrupts (Ctrl+C) during this window, still restore the
    // original auth.json before exiting rather than leaving the staged profile
    // in place. tokio's ctrl_c handler overrides the default-terminate, so the
    // restore block below always runs.
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
        _ = tokio::signal::ctrl_c() => {
            if !json {
                user_println("Interrupted; restoring original auth.json...");
            }
        }
    }

    {
        let codex_auth2 = codex_auth.clone();
        let backup2 = backup.clone();
        let alias2 = target_alias.clone();
        tokio::task::spawn_blocking(move || {
            restore_launch_auth(&codex_auth2, &backup2, had_original, &alias2)
        })
        .await
        .context("lock task panicked")??;
    }
    drop(launch_lease);

    // Wait for codex to exit
    let status = child.wait().context("waiting for codex")?;

    // Compute exit code: prefer code(), fall back to 128+signal on Unix
    #[cfg(unix)]
    let exit_code = status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|s| 128 + s).unwrap_or(1)
    });
    #[cfg(not(unix))]
    let exit_code = status.code().unwrap_or(1);

    if json {
        let mut payload = serde_json::json!({
            "ok": status.success(),
            "alias": target_alias,
            "action": "launched",
            "exit_code": exit_code,
        });
        if let Some(hint) = &revival_hint {
            payload["hint"] = serde_json::Value::String(super::profile::revival_hint_message(hint));
        }
        print_json(&payload);
    } else {
        user_println("codex exited");
    }

    // Propagate codex exit code
    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Roll the staged profile back out of the live auth.json, keeping anything
/// Codex refreshed while it was staged.
///
/// `alias` is the profile that was staged, i.e. the owner of whatever Codex may
/// have rewritten in place.
fn restore_launch_auth(
    codex_auth: &std::path::Path,
    backup: &std::path::Path,
    had_original: bool,
    alias: &str,
) -> Result<()> {
    let _lock = profile::lock_live_auth().context("acquiring auth lock for restore")?;
    // Saving the refreshed copy must never block the rollback: leaving another
    // account's credentials live is worse than failing to archive a refresh.
    match preserve_refreshed_launch_auth(codex_auth, alias) {
        Ok(true) => user_println(&format!(
            "Codex refreshed the credentials of profile '{alias}'; saved them before restoring."
        )),
        Ok(false) => {}
        Err(err) => {
            tracing::warn!("launch restore could not save refreshed credentials: {err:#}");
            user_println(&format!(
                "Warning: could not preserve credentials Codex may have refreshed for profile '{alias}': {err:#}"
            ));
        }
    }
    if had_original {
        std::fs::copy(backup, codex_auth).with_context(|| {
            format!(
                "restoring launch auth backup {} -> {}",
                backup.display(),
                codex_auth.display()
            )
        })?;
        std::fs::remove_file(backup)
            .with_context(|| format!("removing launch auth backup {}", backup.display()))?;
    } else if codex_auth.exists() {
        std::fs::remove_file(codex_auth)
            .with_context(|| format!("removing staged launch auth {}", codex_auth.display()))?;
    }
    Ok(())
}

/// Fold credentials Codex refreshed in place back into the staged profile.
///
/// Codex CLI refreshes on startup when the staged `last_refresh` is old enough,
/// and OpenAI rotates `refresh_token` on every use: the moment Codex refreshes,
/// the copy still stored in the profile is revoked. Restoring the backup over
/// that write would leave the profile holding a dead token — unrecoverable
/// without a full re-login, and undetectable until the profile is next used.
///
/// Returns whether the profile was updated. Nothing is written unless the live
/// file proves it is both newer than the profile and the same account, so a
/// stale or foreign live copy can never overwrite good credentials.
///
/// Caller MUST hold the lock from `lock_live_auth()`.
fn preserve_refreshed_launch_auth(codex_auth: &std::path::Path, alias: &str) -> Result<bool> {
    if !codex_auth.exists() {
        return Ok(false);
    }
    let profile_path = profile::profile_auth_path(alias)?;
    if !profile_path.exists() {
        return Ok(false);
    }
    let saved = auth::read_auth(&profile_path)
        .with_context(|| format!("reading profile '{alias}' auth.json"))?;
    let live = auth::read_auth(codex_auth).with_context(|| {
        format!(
            "reading live auth.json {} before launch restore",
            codex_auth.display()
        )
    })?;
    if !live_is_newer(&saved, &live) {
        return Ok(false);
    }
    ensure_same_account(alias, &saved, &live)?;
    auth::write_auth(&profile_path, &live)
        .with_context(|| format!("saving refreshed credentials into profile '{alias}'"))?;
    Ok(true)
}

/// `last_refresh` is the same RFC3339 stamp Codex and codex-switch both write,
/// so a strictly later value is the evidence that Codex rotated the tokens.
/// A profile without a stamp loses to any live file that has one, because the
/// staged copy came from that profile and therefore had no stamp either.
fn live_is_newer(saved: &serde_json::Value, live: &serde_json::Value) -> bool {
    let Some(live_ts) = last_refresh(live) else {
        return false;
    };
    match last_refresh(saved) {
        Some(saved_ts) => live_ts > saved_ts,
        None => true,
    }
}

fn last_refresh(val: &serde_json::Value) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(val.get("last_refresh")?.as_str()?).ok()
}

/// Same rule as `profile::update_profile_from_live`: the email must be present
/// on both sides and equal, and account ids must agree when both are known.
fn ensure_same_account(
    alias: &str,
    saved: &serde_json::Value,
    live: &serde_json::Value,
) -> Result<()> {
    let saved = profile::extract_identity(saved);
    let live = profile::extract_identity(live);
    let email_matches = matches!(
        (&saved.email, &live.email),
        (Some(saved), Some(live)) if saved == live
    );
    let account_matches = match (&saved.account_id, &live.account_id) {
        (Some(saved), Some(live)) => saved == live,
        _ => true,
    };
    if email_matches && account_matches {
        return Ok(());
    }
    anyhow::bail!(
        "live auth.json was refreshed into a different account than profile '{alias}'; \
         leaving the profile untouched"
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::MutexGuard;

    use super::restore_launch_auth;

    struct TestAppHome {
        _lock: MutexGuard<'static, ()>,
        home: tempfile::TempDir,
        previous: Option<OsString>,
    }

    impl TestAppHome {
        fn new() -> Self {
            let lock = crate::profile::TEST_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let home = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("CODEX_SWITCH_HOME");
            unsafe {
                std::env::set_var("CODEX_SWITCH_HOME", home.path());
            }
            Self {
                _lock: lock,
                home,
                previous,
            }
        }

        fn path(&self) -> &std::path::Path {
            self.home.path()
        }
    }

    impl Drop for TestAppHome {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var("CODEX_SWITCH_HOME", value),
                    None => std::env::remove_var("CODEX_SWITCH_HOME"),
                }
            }
        }
    }

    #[test]
    fn restore_launch_auth_restores_original_and_removes_backup() {
        let home = TestAppHome::new();
        let codex_auth = home.path().join("codex/auth.json");
        let backup = home.path().join("auth.backup");
        std::fs::create_dir_all(codex_auth.parent().unwrap()).unwrap();
        std::fs::write(&codex_auth, b"staged profile").unwrap();
        std::fs::write(&backup, b"original auth").unwrap();

        restore_launch_auth(&codex_auth, &backup, true, "work").unwrap();

        assert_eq!(std::fs::read(&codex_auth).unwrap(), b"original auth");
        assert!(!backup.exists());
        assert!(home.path().join("auth.lock").exists());
    }

    #[test]
    fn restore_launch_auth_removes_staged_auth_without_original() {
        let home = TestAppHome::new();
        let codex_auth = home.path().join("codex/auth.json");
        let backup = home.path().join("auth.backup");
        std::fs::create_dir_all(codex_auth.parent().unwrap()).unwrap();
        std::fs::write(&codex_auth, b"staged profile").unwrap();

        restore_launch_auth(&codex_auth, &backup, false, "work").unwrap();

        assert!(!codex_auth.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn restore_launch_auth_without_original_or_staged_file_is_noop() {
        let home = TestAppHome::new();
        let codex_auth = home.path().join("codex/auth.json");
        let backup = home.path().join("auth.backup");

        restore_launch_auth(&codex_auth, &backup, false, "work").unwrap();

        assert!(!codex_auth.exists());
        assert!(!backup.exists());
    }

    // ── Codex-side refresh during the launch window ───────────────
    //
    // Codex CLI refreshes a staged auth.json whose `last_refresh` is old
    // enough, and OpenAI revokes the old refresh_token the moment it is used.
    // The restore must therefore fold a newer live copy back into the profile
    // instead of rolling the backup over it.

    fn jwt(payload: &serde_json::Value) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        format!(
            "x.{}.y",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap())
        )
    }

    /// `account` seeds both the email and the account id, so two calls with the
    /// same `account` describe the same ChatGPT account.
    fn auth_value(account: &str, refresh_token: &str, last_refresh: &str) -> serde_json::Value {
        let email = format!("{account}@example.com");
        let account_id = format!("acct-{account}");
        let claims = serde_json::json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_account_id": account_id,
                "chatgpt_user_id": format!("user_{account_id}"),
            }
        });
        serde_json::json!({
            "tokens": {
                "id_token": jwt(&claims),
                "access_token": format!("access-{refresh_token}"),
                "refresh_token": refresh_token,
                "account_id": account_id,
            },
            "last_refresh": last_refresh,
        })
    }

    /// Profile "work" plus a staged live file holding the same credentials,
    /// mirroring the state `stage_profile_auth` leaves behind.
    fn staged_launch(
        home: &TestAppHome,
        profile_value: &serde_json::Value,
    ) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let profile_path = crate::profile::profile_auth_path("work").unwrap();
        std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
        crate::auth::write_auth(&profile_path, profile_value).unwrap();

        let codex_auth = home.path().join("codex/auth.json");
        std::fs::create_dir_all(codex_auth.parent().unwrap()).unwrap();
        crate::auth::write_auth(&codex_auth, profile_value).unwrap();

        let backup = home.path().join("auth.backup");
        crate::auth::write_auth(
            &backup,
            &auth_value("other", "other-refresh", "2026-07-01T00:00:00Z"),
        )
        .unwrap();

        (profile_path, codex_auth, backup)
    }

    fn read_json(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn restore_saves_credentials_codex_refreshed_during_launch() {
        let home = TestAppHome::new();
        let staged = auth_value("a", "refresh-old", "2026-07-01T00:00:00Z");
        let (profile_path, codex_auth, backup) = staged_launch(&home, &staged);

        // Codex rotated the token in place while it was staged.
        let refreshed = auth_value("a", "refresh-new", "2026-07-20T10:00:00Z");
        crate::auth::write_auth(&codex_auth, &refreshed).unwrap();

        restore_launch_auth(&codex_auth, &backup, true, "work").unwrap();

        assert_eq!(
            read_json(&profile_path),
            refreshed,
            "the rotated refresh_token must survive the restore"
        );
        assert_eq!(
            read_json(&codex_auth)["tokens"]["refresh_token"],
            "other-refresh",
            "the original live credentials must still be restored"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn restore_saves_refreshed_credentials_when_there_was_no_original() {
        let home = TestAppHome::new();
        let staged = auth_value("a", "refresh-old", "2026-07-01T00:00:00Z");
        let (profile_path, codex_auth, backup) = staged_launch(&home, &staged);
        std::fs::remove_file(&backup).unwrap();

        let refreshed = auth_value("a", "refresh-new", "2026-07-20T10:00:00Z");
        crate::auth::write_auth(&codex_auth, &refreshed).unwrap();

        restore_launch_auth(&codex_auth, &backup, false, "work").unwrap();

        assert_eq!(read_json(&profile_path), refreshed);
        assert!(!codex_auth.exists());
    }

    #[test]
    fn restore_leaves_profile_untouched_when_codex_did_not_refresh() {
        let home = TestAppHome::new();
        let staged = auth_value("a", "refresh-old", "2026-07-01T00:00:00Z");
        let (profile_path, codex_auth, backup) = staged_launch(&home, &staged);

        restore_launch_auth(&codex_auth, &backup, true, "work").unwrap();

        assert_eq!(read_json(&profile_path), staged);
        assert_eq!(
            read_json(&codex_auth)["tokens"]["refresh_token"],
            "other-refresh"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn restore_ignores_live_credentials_older_than_the_profile() {
        let home = TestAppHome::new();
        let staged = auth_value("a", "refresh-new", "2026-07-20T10:00:00Z");
        let (profile_path, codex_auth, backup) = staged_launch(&home, &staged);

        // A stale copy of the same account must never be written back.
        crate::auth::write_auth(
            &codex_auth,
            &auth_value("a", "refresh-dead", "2026-07-01T00:00:00Z"),
        )
        .unwrap();

        restore_launch_auth(&codex_auth, &backup, true, "work").unwrap();

        assert_eq!(read_json(&profile_path), staged);
    }

    #[test]
    fn restore_ignores_live_credentials_from_another_account() {
        let home = TestAppHome::new();
        let staged = auth_value("a", "refresh-old", "2026-07-01T00:00:00Z");
        let (profile_path, codex_auth, backup) = staged_launch(&home, &staged);

        crate::auth::write_auth(
            &codex_auth,
            &auth_value("b", "refresh-b", "2026-07-20T10:00:00Z"),
        )
        .unwrap();

        restore_launch_auth(&codex_auth, &backup, true, "work").unwrap();

        assert_eq!(
            read_json(&profile_path),
            staged,
            "another account's credentials must not pollute this profile"
        );
    }
}
