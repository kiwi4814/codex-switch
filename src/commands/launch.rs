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
            tokio::task::spawn_blocking(move || {
                restore_launch_auth(&codex_auth2, &backup2, had_original)
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
            tokio::task::spawn_blocking(move || {
                restore_launch_auth(&codex_auth2, &backup2, had_original)
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
        tokio::task::spawn_blocking(move || {
            restore_launch_auth(&codex_auth2, &backup2, had_original)
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

fn restore_launch_auth(
    codex_auth: &std::path::Path,
    backup: &std::path::Path,
    had_original: bool,
) -> Result<()> {
    let _lock = profile::lock_live_auth().context("acquiring auth lock for restore")?;
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
