use super::render::print_usage_line;
use crate::output::{self, ProgressReporter, account_to_json, print_json, usage_to_json};
use crate::{auth, cache, color, profile, usage};
use anyhow::Result;

/// Validation failed, but the auth server had already rotated the credentials
/// and they were rescued into a profile.
const STAGE_TOKEN_ROTATED: &str = "token_rotated";
/// Same rotation, but nothing could be written — the account is lost unless the
/// user acts.
const STAGE_TOKEN_ROTATION_LOST: &str = "token_rotation_lost";

/// Whether a failure also consumed the account's single-use `refresh_token`.
///
/// These entries look like any other line in a directory report, yet they mean
/// the source file is now worthless and a profile may have appeared — so they
/// get their own marker instead of blending into the skip list.
fn rotated_credentials(stage: &str) -> bool {
    stage == STAGE_TOKEN_ROTATED || stage == STAGE_TOKEN_ROTATION_LOST
}

/// Save credentials the auth server rotated during a validation that then
/// failed.
///
/// They go to the profile store rather than back to the source file: it is the
/// tool's own storage, so it stays writable when the imported dump is not (auth
/// dumps are routinely copied in read-only), it is where a successful import
/// would have put them anyway, and an existing profile for the same identity is
/// updated in place instead of duplicated. The source file keeps the consumed
/// token either way — that is unavoidable once the server has rotated it — so
/// the message has to steer the user away from re-importing it.
fn rescue_rotated_credentials(
    source: &std::path::Path,
    val: serde_json::Value,
    alias: Option<&str>,
    cause: &anyhow::Error,
) -> profile::ImportFailure {
    match profile::save_imported_auth_value(val, alias) {
        Ok(action) => profile::ImportFailure {
            source: source.to_path_buf(),
            stage: STAGE_TOKEN_ROTATED,
            error: format!(
                "validation failed ({cause}), but the auth server had already rotated this \
                 account's credentials, so they were {} as profile '{}'. {} now holds a dead \
                 refresh token — use the profile instead of importing that file again.",
                action.action(),
                action.alias(),
                source.display()
            ),
        },
        Err(save_error) => profile::ImportFailure {
            source: source.to_path_buf(),
            stage: STAGE_TOKEN_ROTATION_LOST,
            error: format!(
                "validation failed ({cause}) after the auth server rotated this account's \
                 credentials, and {}",
                unsaveable_rotation_reason(&save_error)
            ),
        },
    }
}

/// The rotated credentials could not be written anywhere.
///
/// The previous `refresh_token` is already dead server-side, so there is no
/// copy left that any server would accept — the only honest thing to report is
/// that the account needs a new login.
fn unsaveable_rotation(
    source: &std::path::Path,
    save_error: &anyhow::Error,
) -> profile::ImportFailure {
    profile::ImportFailure {
        source: source.to_path_buf(),
        stage: STAGE_TOKEN_ROTATION_LOST,
        error: format!(
            "the auth server rotated this account's credentials during validation, and {}",
            unsaveable_rotation_reason(save_error)
        ),
    }
}

fn unsaveable_rotation_reason(save_error: &anyhow::Error) -> String {
    format!(
        "saving them failed ({save_error:#}). The previous refresh token is already invalidated, \
         so this account has to sign in again."
    )
}

// ── import ───────────────────────────────────────────────

pub(crate) async fn import_cmd(path: &str, alias: Option<&str>, json: bool) -> Result<()> {
    let input = std::path::PathBuf::from(path);
    let files = profile::collect_import_files(&input)?;

    if input.is_dir() {
        if let Some(alias) = alias {
            anyhow::bail!(
                "alias '{alias}' can only be used when importing a single file, not a directory"
            );
        }
        if files.is_empty() {
            anyhow::bail!("no JSON files found under {}", input.display());
        }
    }

    if files.len() == 1 && input.is_file() {
        let imported = match import_one_file(&files[0], alias).await {
            Ok(imported) => imported,
            Err(failure) => anyhow::bail!("{}: {}", failure.stage, failure.error),
        };
        if json {
            print_json(&output::JsonOk {
                ok: true,
                alias: imported.alias,
                action: imported.action.to_string(),
            });
        } else {
            println!(
                "{}",
                color::success(&format!(
                    "Validated and {}: {} -> profile '{}'",
                    imported.action,
                    imported.source.display(),
                    imported.alias
                ))
            );
            print!("  ");
            print_usage_line(&imported.usage);
        }
        return Ok(());
    }

    let mut report = profile::ImportReport::default();
    let mut progress = if json {
        None
    } else {
        Some(ProgressReporter::new("Validating auth files", files.len()))
    };

    for (idx, file) in files.into_iter().enumerate() {
        match import_one_file(&file, None).await {
            Ok(success) => report.imported.push(success),
            Err(failure) => report.skipped.push(failure),
        }
        if let Some(progress) = progress.as_mut() {
            progress.advance(idx + 1);
        }
    }

    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }

    let all_skipped = report.imported.is_empty();
    let credentials_lost = report
        .skipped
        .iter()
        .any(|item| item.stage == STAGE_TOKEN_ROTATION_LOST);
    if json {
        print_json(&output::JsonImportReport {
            ok: !all_skipped,
            credentials_lost,
            imported: report
                .imported
                .iter()
                .map(|item| output::JsonImportEntry {
                    source: item.source.display().to_string(),
                    alias: item.alias.clone(),
                    action: item.action.to_string(),
                    account: account_to_json(&item.account, item.usage.plan_type.as_deref()),
                    usage: usage_to_json(Ok(&item.usage)),
                })
                .collect(),
            skipped: report
                .skipped
                .iter()
                .map(|item| output::JsonImportFailure {
                    source: item.source.display().to_string(),
                    stage: item.stage.to_string(),
                    error: item.error.clone(),
                })
                .collect(),
        });
        if all_skipped {
            return Err(super::super::OutputAlreadyReported.into());
        }
    } else {
        println!(
            "{}",
            color::success(&format!(
                "Imported {} profile(s); skipped {} file(s)",
                report.imported.len(),
                report.skipped.len()
            ))
        );

        for item in &report.imported {
            println!(
                "  {} {} -> {} ({})",
                color::status_tag("OK"),
                item.source.display(),
                item.alias,
                item.action
            );
            print!("    ");
            print_usage_line(&item.usage);
        }

        for item in &report.skipped {
            let line = format!(
                "  {} {} [{}] {}",
                color::status_tag(if rotated_credentials(item.stage) {
                    "Rotated"
                } else {
                    "Skip"
                }),
                item.source.display(),
                item.stage,
                item.error
            );
            if rotated_credentials(item.stage) {
                println!("{}", color::warn(&line));
            } else {
                println!("{line}");
            }
        }

        let rotated = report
            .skipped
            .iter()
            .filter(|item| rotated_credentials(item.stage))
            .count();
        if rotated > 0 {
            println!(
                "{}",
                color::warn(&format!(
                    "  {rotated} file(s) had their credentials rotated during validation; their \
                     refresh token is spent and importing those files again will fail."
                ))
            );
        }

        if all_skipped {
            anyhow::bail!(
                "no profiles imported; all {} files were skipped",
                report.skipped.len()
            );
        }
    }
    Ok(())
}

async fn import_one_file(
    source: &std::path::Path,
    alias: Option<&str>,
) -> std::result::Result<profile::ImportSuccess, profile::ImportFailure> {
    let mut val = auth::read_auth(source).map_err(|e| profile::ImportFailure {
        source: source.to_path_buf(),
        stage: "file_format",
        error: e.to_string(),
    })?;

    auth::validate_auth_value(&val).map_err(|e| profile::ImportFailure {
        source: source.to_path_buf(),
        stage: "structure",
        error: e.to_string(),
    })?;

    let validation = usage::validate_import_auth(&mut val).await;
    let rotated = validation.refreshed.is_some();
    let usage = match validation.result {
        Ok(usage) => usage,
        // A rotation already happened inside the validation, so `val` now holds
        // the only credentials the auth server still accepts. They must be
        // written somewhere durable before this failure is reported.
        Err(error) if rotated => {
            return Err(rescue_rotated_credentials(source, val, alias, &error));
        }
        Err(error) => {
            return Err(profile::ImportFailure {
                source: source.to_path_buf(),
                stage: "usage_validation",
                error: error.to_string(),
            });
        }
    };

    let mut account = auth::validate_auth_value(&val).map_err(|e| profile::ImportFailure {
        source: source.to_path_buf(),
        stage: "structure",
        error: e.to_string(),
    })?;
    cache::apply_workspace_name(&mut account);

    let action = profile::save_imported_auth_value(val, alias).map_err(|e| {
        // The validation consumed a rotation, so a failed write here loses the
        // same single-use credential — not just a file.
        if rotated {
            unsaveable_rotation(source, &e)
        } else {
            profile::ImportFailure {
                source: source.to_path_buf(),
                stage: "save",
                error: e.to_string(),
            }
        }
    })?;

    Ok(profile::ImportSuccess {
        source: source.to_path_buf(),
        alias: action.alias().to_string(),
        action: action.action(),
        account,
        usage,
    })
}
