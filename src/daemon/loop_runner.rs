use anyhow::Result;

use crate::{auth, cache, config, profile, usage, warmup};

/// Main daemon event loop: periodically checks usage and switches account when needed.
pub async fn run_daemon_loop() -> Result<()> {
    let cfg = config::get();
    let poll_secs = cfg.daemon.poll_interval_secs;
    let token_secs = cfg.daemon.token_check_interval_secs;
    let cache_refresh_secs = cfg.daemon.cache_refresh_interval_secs;
    let auto_warmup = cfg.daemon.auto_warmup;

    let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(poll_secs));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut token_interval = tokio::time::interval(std::time::Duration::from_secs(token_secs));
    token_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let cache_refresh_period = std::time::Duration::from_secs(cache_refresh_secs);
    let mut cache_refresh_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + cache_refresh_period,
        cache_refresh_period,
    );
    cache_refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut consecutive_failures: u32 = 0;

    tracing::info!(
        "Daemon loop started: poll={}s, token_check={}s, cache_refresh={}s, auto_warmup={}, threshold={}%",
        poll_secs,
        token_secs,
        cache_refresh_secs,
        auto_warmup,
        cfg.daemon.switch_threshold,
    );

    loop {
        tokio::select! {
            _ = poll_interval.tick() => {
                match check_and_switch().await {
                    Ok(switched) => {
                        consecutive_failures = 0;
                        if switched {
                            tracing::info!("Account switch completed");
                        }
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        let backoff_secs = poll_secs * 2u64.pow(consecutive_failures.min(4));
                        tracing::error!(
                            "Monitor cycle failed ({consecutive_failures}x): {e}, backing off {backoff_secs}s"
                        );
                        // Backoff with nested select! so shutdown signal is still responsive
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                            _ = shutdown_signal() => {
                                tracing::info!("Received shutdown signal during backoff, exiting");
                                break;
                            }
                        }
                    }
                }
            }
            _ = token_interval.tick() => {
                usage::refresh_expiring_tokens().await;
            }
            _ = cache_refresh_interval.tick() => {
                match refresh_profile_cache(auto_warmup).await {
                    Ok(summary) => tracing::debug!(
                        "Cache refresh completed: refreshed={}, warmed={}, failed={}",
                        summary.refreshed,
                        summary.warmed,
                        summary.failed
                    ),
                    Err(e) => tracing::warn!("Cache refresh skipped: {e}"),
                }
            }
            _ = shutdown_signal() => {
                tracing::info!("Received shutdown signal, exiting daemon loop");
                break;
            }
        }
    }
    Ok(())
}

/// Check current account usage and switch to a better candidate if threshold exceeded.
///
/// Returns `true` if a switch was performed.
async fn check_and_switch() -> Result<bool> {
    let profiles = profile::list_profiles()?;
    if profiles.len() < 2 {
        return Ok(false);
    }

    let current = profile::read_current();
    if current.is_empty() {
        return Ok(false);
    }

    let cfg = config::get();
    let safety_7d = cfg.use_cfg.safety_margin_7d;
    let threshold = cfg.daemon.switch_threshold;
    let now = auth::now_unix_secs();

    // 1. Force-fetch current account's usage (bypass cache)
    let current_path = profile::profile_auth_path(&current)?;
    let current_usage = usage::fetch_usage_retried_force(&current, &current_path, &current)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.detail))?;

    // 2. Check if current account exceeds threshold
    // Free accounts have no primary window (7d is remapped to secondary),
    // so fall back to secondary when primary is absent.
    let current_used = current_usage
        .primary
        .as_ref()
        .or(current_usage.secondary.as_ref())
        .and_then(|w| w.used_percent)
        .unwrap_or(0.0);

    if current_used < threshold {
        tracing::debug!(
            "Current account '{}' at {:.1}%, below threshold {:.1}%",
            current,
            current_used,
            threshold,
        );
        return Ok(false);
    }

    tracing::info!(
        "Current account '{}' at {:.1}%, above threshold {:.1}% -- searching for better candidate",
        current,
        current_used,
        threshold,
    );

    // 3. Score current account using adaptive algorithm
    let team_priority = cfg.use_cfg.team_priority;
    let pool_size = profiles.len();

    let current_info = profile::profile_auth_path(&current)
        .map(|p| auth::read_account_info(&p))
        .unwrap_or_default();
    let mut current_candidate = usage::Candidate::from_usage(
        current.clone(),
        &current_usage,
        current_info.is_team(),
        current_info.is_free(),
        cache::get_last_used(&current),
        now,
    );
    current_candidate.pool_size = pool_size;
    current_candidate.team_priority = team_priority;

    // 4. Fetch all other candidates concurrently, then compute pool_exhausted and score
    let mut tasks = tokio::task::JoinSet::new();

    for alias in &profiles {
        if alias == &current {
            continue;
        }
        let path = match profile::profile_auth_path(alias) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let alias = alias.clone();
        let current = current.clone();
        tasks.spawn(async move {
            let u = usage::fetch_usage_retried(&alias, &path, &current).await;
            (alias, path, u)
        });
    }

    let mut other_candidates: Vec<(usage::Candidate, String)> = Vec::new();
    while let Some(res) = tasks.join_next().await {
        let (alias, path, u) = match res {
            Ok(v) => v,
            Err(_) => continue,
        };
        let u = match u {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("[{alias}] fetch failed: {}", e.summary);
                continue;
            }
        };
        let info = auth::read_account_info(&path);
        let mut candidate = usage::Candidate::from_usage(
            alias.clone(),
            &u,
            info.is_team(),
            info.is_free(),
            cache::get_last_used(&alias),
            now,
        );
        candidate.pool_size = pool_size;
        candidate.team_priority = team_priority;
        other_candidates.push((candidate, alias));
    }

    // Compute pool_exhausted across all accounts (including current)
    let pool_exhausted = other_candidates
        .iter()
        .filter(|(c, _)| c.effective_used_5h() >= 100.0)
        .count()
        + if current_candidate.effective_used_5h() >= 100.0 {
            1
        } else {
            0
        };

    // Patch pool_exhausted into current candidate and re-score
    current_candidate.pool_exhausted = pool_exhausted;
    let current_score = usage::score_unified(&current_candidate, safety_7d);

    // Two-phase: prefer eligible candidates, fallback to best ineligible
    let mut best_eligible: Option<(String, f64)> = None;
    let mut best_ineligible: Option<(String, f64)> = None;
    let mut any_eligible = false;

    for (mut candidate, alias) in other_candidates {
        candidate.pool_exhausted = pool_exhausted;
        let s = usage::score_unified(&candidate, safety_7d);
        let eligible = usage::is_candidate_eligible(&candidate, safety_7d);

        if eligible {
            any_eligible = true;
            if s > current_score && best_eligible.as_ref().is_none_or(|(_, bs)| s > *bs) {
                best_eligible = Some((alias, s));
            }
        } else if s > current_score && best_ineligible.as_ref().is_none_or(|(_, bs)| s > *bs) {
            best_ineligible = Some((alias, s));
        }
    }

    // Use eligible candidate if available, otherwise fallback to best ineligible
    let best = best_eligible.or(if !any_eligible { best_ineligible } else { None });

    // 5. Switch if a better candidate was found
    if let Some((best_alias, best_score)) = best {
        tracing::info!(
            "Switching: '{}' (score {:.1}) -> '{}' (score {:.1})",
            current,
            current_score,
            best_alias,
            best_score,
        );
        profile::switch_profile(&best_alias)?;
        cache::set_last_used(&best_alias)?;

        if cfg.daemon.notify {
            super::notify::send_notification(&format!(
                "Switched to '{}' (score: {:.0})",
                best_alias, best_score
            ));
        }
        return Ok(true);
    }

    tracing::debug!("No better candidate found");
    Ok(false)
}

#[derive(Default)]
struct CacheRefreshSummary {
    refreshed: usize,
    warmed: usize,
    failed: usize,
}

async fn refresh_profile_cache(auto_warmup: bool) -> Result<CacheRefreshSummary> {
    let profiles = profile::list_profiles()?;
    if profiles.is_empty() {
        return Ok(CacheRefreshSummary::default());
    }

    let current = profile::read_current();
    let now = auth::now_unix_secs();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(
        config::get().network.max_concurrent,
    ));
    let mut tasks = tokio::task::JoinSet::new();

    for alias in profiles {
        let current = current.clone();
        let sem = semaphore.clone();
        tasks.spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return (
                    alias,
                    false,
                    false,
                    Some("usage limiter closed".to_string()),
                );
            };
            let path = match profile::profile_auth_path(&alias) {
                Ok(path) => path,
                Err(e) => return (alias, false, false, Some(e.to_string())),
            };

            let usage = match usage::fetch_usage_retried_force(&alias, &path, &current).await {
                Ok(usage) => usage,
                Err(e) => return (alias, false, false, Some(e.summary)),
            };

            if !auto_warmup || usage::usage_has_active_warmup_window(&usage, now) {
                return (alias, true, false, None);
            }

            if let Err(e) = warmup::warmup_account(&alias, &path).await {
                return (alias, true, false, Some(format!("warmup failed: {e}")));
            }

            if let Err(e) = usage::fetch_usage_retried_force(&alias, &path, &current).await {
                tracing::warn!("[{alias}] post-warmup cache refresh failed: {}", e.summary);
            }
            (alias, true, true, None)
        });
    }

    let mut summary = CacheRefreshSummary::default();
    while let Some(res) = tasks.join_next().await {
        let (alias, refreshed, warmed, err) = match res {
            Ok(value) => value,
            Err(e) => {
                summary.failed += 1;
                tracing::warn!("Cache refresh worker failed: {e}");
                continue;
            }
        };
        if refreshed {
            summary.refreshed += 1;
        }
        if warmed {
            summary.warmed += 1;
        }
        if let Some(err) = err {
            summary.failed += 1;
            tracing::warn!("[{alias}] cache refresh failed: {err}");
        }
    }

    Ok(summary)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("Ctrl+C handler");
    }
}
