use anyhow::Result;

use super::state::{self, DaemonState, PendingSwitch, SwitchRecord};
use crate::{auth, cache, config, profile, usage, warmup};

/// Outcome of one monitor poll.
enum PollOutcome {
    NoAction,
    Switched {
        from: String,
        to: String,
        score: f64,
    },
    Deferred {
        to: String,
    },
}

/// Backoff after `consecutive_failures` failed polls, capped at 16 poll intervals.
fn poll_backoff_secs(poll_secs: u64, consecutive_failures: u32) -> u64 {
    poll_secs * 2u64.pow(consecutive_failures.min(4))
}

/// Main daemon event loop: periodically checks usage and switches account when needed.
pub async fn run_daemon_loop() -> Result<()> {
    // Registered before anything else can block: from here on every signal is
    // recorded, even while a branch body is busy.
    let mut shutdown = ShutdownListener::new()?;

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

    let mut st = DaemonState {
        pid: std::process::id(),
        started_at: auth::now_unix_secs(),
        ..DaemonState::default()
    };
    state::write(&mut st);

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
                // Failure backoff suspends polling only; token and cache
                // timers keep running.
                let now = auth::now_unix_secs();
                if let Some(until) = st.backoff_until {
                    if now < until {
                        tracing::debug!("Poll suspended by backoff for {}s more", until - now);
                        continue;
                    }
                    st.backoff_until = None;
                }

                match check_and_switch().await {
                    Ok(outcome) => {
                        st.consecutive_failures = 0;
                        st.last_error = None;
                        st.last_poll_at = Some(auth::now_unix_secs());
                        match outcome {
                            PollOutcome::Switched { from, to, score } => {
                                tracing::info!("Account switch completed");
                                st.pending_switch = None;
                                st.last_switch = Some(SwitchRecord {
                                    from,
                                    to,
                                    at: auth::now_unix_secs(),
                                    score,
                                });
                            }
                            PollOutcome::Deferred { to } => {
                                // Keep the original `since` while the same target stays pending.
                                let since = st
                                    .pending_switch
                                    .as_ref()
                                    .filter(|p| p.to == to)
                                    .map(|p| p.since)
                                    .unwrap_or_else(auth::now_unix_secs);
                                st.pending_switch = Some(PendingSwitch { to, since });
                            }
                            PollOutcome::NoAction => {
                                st.pending_switch = None;
                            }
                        }
                    }
                    Err(e) => {
                        st.consecutive_failures += 1;
                        st.last_poll_at = Some(auth::now_unix_secs());
                        st.last_error = Some(e.to_string());
                        let backoff_secs = poll_backoff_secs(poll_secs, st.consecutive_failures);
                        st.backoff_until = Some(auth::now_unix_secs() + backoff_secs as i64);
                        tracing::error!(
                            "Monitor cycle failed ({}x): {e}, backing off {backoff_secs}s",
                            st.consecutive_failures
                        );
                    }
                }
                state::write(&mut st);
            }
            _ = token_interval.tick() => {
                // Runs unattended on a timer: a lost write here bricks the
                // profile with nobody watching, so it gets ERROR, not debug.
                for failure in usage::refresh_expiring_tokens().await {
                    // `detail` already opens with `[alias]` and carries the
                    // underlying IO/permission cause; the field makes the
                    // affected profile filterable in structured log output.
                    tracing::error!(alias = %failure.alias, "{}", failure.error.detail);
                }
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
                st.last_cache_refresh_at = Some(auth::now_unix_secs());
                state::write(&mut st);
            }
            _ = shutdown.recv() => {
                tracing::info!("Received shutdown signal, exiting daemon loop");
                break;
            }
        }
    }
    Ok(())
}

/// Check current account usage and switch to a better candidate if threshold exceeded.
async fn check_and_switch() -> Result<PollOutcome> {
    let profiles = profile::list_profiles()?;
    if profiles.len() < 2 {
        return Ok(PollOutcome::NoAction);
    }

    let current = profile::read_current();
    if current.is_empty() {
        return Ok(PollOutcome::NoAction);
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
        return Ok(PollOutcome::NoAction);
    }

    tracing::info!(
        "Current account '{}' at {:.1}%, above threshold {:.1}% -- searching for better candidate",
        current,
        current_used,
        threshold,
    );

    // 3. Fetch all other candidates concurrently
    let team_priority = cfg.use_cfg.team_priority;
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

    // 4. Score everything uniformly (same helper as CLI `use`); the current
    // account goes first so it can be split back off after scoring.
    let mut items = vec![(
        current.clone(),
        current_usage.clone(),
        auth::read_account_info(&current_path),
        cache::get_last_used(&current),
    )];
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
        let last_used = cache::get_last_used(&alias);
        items.push((alias, u, info, last_used));
    }

    let mut scored = usage::score_candidates(items, now, safety_7d, team_priority);
    let current_score = scored.remove(0).score;

    // 5. Switch if a better candidate was found
    if let Some((best_alias, best_score)) =
        usage::pick_switch_target(current_score, &scored, safety_7d)
    {
        let (best_alias, best_score) = (best_alias.to_string(), best_score);
        // A switch replaces the live auth.json; doing that under an active
        // Codex session would swap accounts mid-conversation. Hold the
        // switch and let the next poll retry once the session ends.
        if cfg.daemon.defer_switch_while_codex_running
            && super::codex_process::codex_process_running()
        {
            tracing::info!(
                "Deferring switch '{}' -> '{}': a Codex session is running",
                current,
                best_alias,
            );
            return Ok(PollOutcome::Deferred { to: best_alias });
        }

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
        return Ok(PollOutcome::Switched {
            from: current,
            to: best_alias,
            score: best_score,
        });
    }

    tracing::debug!("No better candidate found");
    Ok(PollOutcome::NoAction)
}

#[cfg(test)]
mod tests {
    use super::poll_backoff_secs;

    #[test]
    fn poll_backoff_doubles_and_caps_at_sixteen_intervals() {
        assert_eq!(poll_backoff_secs(60, 1), 120);
        assert_eq!(poll_backoff_secs(60, 2), 240);
        assert_eq!(poll_backoff_secs(60, 4), 960);
        assert_eq!(poll_backoff_secs(60, 10), 960);
    }

    /// `daemon stop` sends a single SIGTERM. The daemon's select loop spends a
    /// large share of every second inside a branch body (the poll branch does
    /// HTTP round trips), and during that time nothing polls the shutdown
    /// branch. Tokio drops a delivered signal outright when no listener is
    /// registered at broadcast time, so the listener has to survive across
    /// loop iterations rather than be rebuilt inside `select!`.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_listener_catches_a_signal_raised_while_it_is_not_polled() {
        use super::ShutdownListener;
        use std::time::Duration;
        use tokio::signal::unix::{SignalKind, signal};

        let mut listener = ShutdownListener::new().expect("shutdown listener");

        // A second listener registered up front turns "tokio has finished
        // broadcasting the signal" into an awaitable event, so the assertion
        // below never depends on sleeping long enough.
        let mut witness = signal(SignalKind::terminate()).expect("witness listener");

        // SAFETY: raising SIGTERM at our own process. Both listeners above are
        // registered first, so tokio's handler is installed and the default
        // terminate action cannot fire.
        assert_eq!(unsafe { libc::raise(libc::SIGTERM) }, 0);
        witness.recv().await;

        // `listener` was never polled before the broadcast -- exactly the state
        // the daemon loop is in while a poll body runs.
        tokio::time::timeout(Duration::from_secs(5), listener.recv())
            .await
            .expect("a SIGTERM delivered while the loop was busy must not be lost");
    }
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

/// Shutdown signal listener, registered once and kept alive for the whole
/// daemon loop.
///
/// It must outlive the loop rather than be created inside `select!`: tokio
/// only delivers a signal to listeners that are registered at the moment it
/// broadcasts, and drops it for good otherwise. A listener built inside the
/// select is torn down every time another branch wins, leaving the daemon deaf
/// for the entire duration of each poll / cache-refresh body — and those do
/// HTTP round trips. A `daemon stop` landing in that window used to be lost
/// outright, leaving the daemon running with no second chance.
struct ShutdownListener {
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
    #[cfg(unix)]
    sigint: tokio::signal::unix::Signal,
    #[cfg(not(unix))]
    ctrl_c: tokio::signal::windows::CtrlC,
}

impl ShutdownListener {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                sigterm: signal(SignalKind::terminate())?,
                sigint: signal(SignalKind::interrupt())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                ctrl_c: tokio::signal::windows::ctrl_c()?,
            })
        }
    }

    /// Resolves once a shutdown signal has been received. Safe to cancel: the
    /// registration lives in `self`, so a signal that arrives while this future
    /// is not being polled is still observed by the next call.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.sigterm.recv() => {},
                _ = self.sigint.recv() => {},
            }
        }
        #[cfg(not(unix))]
        {
            self.ctrl_c.recv().await;
        }
    }
}
