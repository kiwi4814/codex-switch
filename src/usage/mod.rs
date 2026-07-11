use std::time::Duration;

use serde::{Deserialize, Serialize};

mod api;
mod parse;
mod reset_credits;
mod scoring;

pub(crate) use api::{apply_account_routing_headers, do_refresh_token};
pub use api::{
    fetch_usage_retried, fetch_usage_retried_force, refresh_expiring_tokens, validate_import_auth,
};
// Re-exported for the lib target's public API (used by integration tests via
// `codex_switch::usage::X`); the binary target doesn't call these through this
// path itself, so they'd otherwise look unused there.
#[allow(unused_imports)]
pub use api::fetch_usage_with_refresh;
#[allow(unused_imports)]
pub use parse::parse_usage;
pub use reset_credits::{consume_earliest_reset_credit, earliest_reset_credit};
pub use scoring::{
    is_available, is_candidate_eligible, pace_percent, pick_switch_target, score_candidates,
    usage_has_active_warmup_window, visible_pace_percent,
};
#[allow(unused_imports)]
pub use scoring::{score_unified, warmup_window_active};

#[derive(Debug, Default, Clone)]
pub struct WindowUsage {
    pub used_percent: Option<f64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ResetCredit {
    pub id: String,
    pub granted_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConsumedResetCredit {
    pub credit: ResetCredit,
    pub code: Option<String>,
    pub windows_reset: Option<u64>,
    pub redeemed_at: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct UsageInfo {
    pub fetched_at: Option<i64>,
    pub primary: Option<WindowUsage>,   // 5h window
    pub secondary: Option<WindowUsage>, // 7d window
    pub credits_balance: Option<f64>,
    pub unlimited_credits: Option<bool>,
    /// plan_type from usage API response (authoritative; overrides JWT claims when present)
    pub plan_type: Option<String>,
    pub reset_credits_available_count: Option<u64>,
    pub reset_credits: Vec<ResetCredit>,
    pub reset_credits_error: Option<String>,
    /// Explicit account/workspace-level restriction reported by the API.
    pub account_limited: bool,
}

/// All data needed to score an account. Pure data, no I/O.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub alias: String,
    pub used_5h: f64,
    pub resets_at_5h: Option<i64>,
    pub used_7d: f64,
    pub resets_at_7d: Option<i64>,
    pub has_5h_data: bool,
    pub has_7d_data: bool,
    pub is_team: bool,
    pub is_free: bool,
    pub last_used: i64,
    pub now: i64,
    // Pool-level signals (set by caller after building all candidates)
    pub pool_size: usize,
    pub pool_exhausted: usize,
    pub team_priority: bool,
}

impl Candidate {
    /// Build from UsageInfo + metadata. `now` should be shared across all candidates.
    pub fn from_usage(
        alias: String,
        u: &UsageInfo,
        is_team: bool,
        is_free: bool,
        last_used: i64,
        now: i64,
    ) -> Self {
        let force_exhausted = u.account_limited;
        Self {
            alias,
            used_5h: if force_exhausted {
                100.0
            } else {
                u.primary
                    .as_ref()
                    .and_then(|w| w.used_percent)
                    .unwrap_or(0.0)
            },
            resets_at_5h: (!force_exhausted)
                .then(|| u.primary.as_ref().and_then(|w| w.resets_at))
                .flatten(),
            used_7d: if force_exhausted {
                100.0
            } else {
                u.secondary
                    .as_ref()
                    .and_then(|w| w.used_percent)
                    .unwrap_or(0.0)
            },
            resets_at_7d: (!force_exhausted)
                .then(|| u.secondary.as_ref().and_then(|w| w.resets_at))
                .flatten(),
            has_5h_data: u.primary.is_some() || force_exhausted,
            has_7d_data: u.secondary.is_some() || force_exhausted,
            is_team,
            is_free,
            last_used,
            now,
            pool_size: 1,
            pool_exhausted: 0,
            team_priority: false,
        }
    }

    /// Reset-aware effective 5h usage: 0.0 if window has already reset.
    pub fn effective_used_5h(&self) -> f64 {
        if self.resets_at_5h.is_some_and(|ts| ts <= self.now) {
            0.0
        } else {
            self.used_5h
        }
    }

    /// Reset-aware effective 7d usage: 0.0 if window has already reset.
    pub fn effective_used_7d(&self) -> f64 {
        if self.resets_at_7d.is_some_and(|ts| ts <= self.now) {
            0.0
        } else {
            self.used_7d
        }
    }
}

/// Window durations in seconds (used for pace calculation).
pub const WINDOW_5H_SECS: i64 = 5 * 3600;
pub const WINDOW_7D_SECS: i64 = 7 * 86400;

/// Free plan accounts become ineligible below this 5h remaining%.
pub const FREE_FLOOR_PCT: f64 = 35.0;

/// Minimum elapsed time before a quota window proves that warmup truly stuck.
pub const MIN_WARMUP_ELAPSED_SECS: i64 = 5 * 60;

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

pub struct RefreshedTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// Structured error for usage fetch failures.
#[derive(Debug, Clone)]
pub struct UsageError {
    /// Short summary for user-facing display (e.g. "HTTP 401 Unauthorized")
    pub summary: String,
    /// Full detail for debug/log (e.g. "Usage API failed (HTTP 401), token refresh also failed: ...")
    pub detail: String,
}

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

/// One scored candidate. Pure data, no I/O.
pub struct ScoredCandidate {
    pub candidate: Candidate,
    pub usage: UsageInfo,
    pub score: f64,
}
