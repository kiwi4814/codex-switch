use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::auth::{self, CLIENT_ID, format_reqwest_error};

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

/// Returns true only when usage data proves a warmup-opened window is active.
pub fn warmup_window_active(w: &WindowUsage, window_secs: i64, now: i64) -> bool {
    let resets_at = match w.resets_at {
        Some(t) if t > now => t,
        _ => return false,
    };
    if w.used_percent.unwrap_or(0.0) <= 0.0 {
        return false;
    }
    let elapsed = window_secs - (resets_at - now);
    elapsed >= MIN_WARMUP_ELAPSED_SECS
}

/// Decide whether warmup should be skipped because the relevant window is already active.
///
/// Paid accounts have both 5h (primary) and 7d (secondary) windows; only the 5h window
/// is what warmup is meant to (re)open, so a still-active 7d window must NOT suppress
/// warmup once the 5h window has closed. Free accounts only have the 7d window, so it
/// is the only signal available.
pub fn usage_has_active_warmup_window(u: &UsageInfo, now: i64) -> bool {
    match u.primary.as_ref() {
        Some(w) => warmup_window_active(w, WINDOW_5H_SECS, now),
        None => u
            .secondary
            .as_ref()
            .is_some_and(|w| warmup_window_active(w, WINDOW_7D_SECS, now)),
    }
}

/// Calculate pace: the expected used_percent if consumption were even across the window.
/// Returns None if resets_at is unavailable.
pub fn pace_percent(w: &WindowUsage, window_secs: i64) -> Option<f64> {
    let resets_at = w.resets_at?;
    let now = auth::now_unix_secs();
    let remaining_secs = (resets_at - now).max(0) as f64;
    let elapsed_secs = (window_secs as f64 - remaining_secs).clamp(0.0, window_secs as f64);
    Some((elapsed_secs / window_secs as f64 * 100.0).clamp(0.0, 100.0))
}

/// Pace marker for UI/text rendering.
/// Hide it when the UI would already render `0% left`, because there is no meaningful
/// remaining quota to pace against.
pub fn visible_pace_percent(w: &WindowUsage, window_secs: i64) -> Option<f64> {
    let used = w.used_percent.unwrap_or(0.0).min(100.0);
    let remaining = (100.0 - used).max(0.0);
    if remaining.round() <= 0.0 {
        None
    } else {
        pace_percent(w, window_secs)
    }
}

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

pub(crate) fn apply_account_routing_headers(
    mut builder: reqwest::RequestBuilder,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> reqwest::RequestBuilder {
    if let Some(account_id) = account_id.filter(|value| !value.trim().is_empty()) {
        builder = builder.header("ChatGPT-Account-ID", account_id);
    }
    if is_fedramp {
        builder = builder.header("X-OpenAI-Fedramp", "true");
    }
    builder
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<String>,
}

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

fn usage_url() -> String {
    std::env::var("CS_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string())
}

fn reset_credits_url() -> String {
    if let Ok(url) = std::env::var("CS_RESET_CREDITS_URL") {
        return url;
    }
    if let Ok(url) = std::env::var("CS_USAGE_URL")
        && let Some(base) = url.strip_suffix("/usage")
    {
        return format!("{base}/rate-limit-reset-credits");
    }
    RESET_CREDITS_URL.to_string()
}

fn reset_credits_consume_url() -> String {
    if let Ok(url) = std::env::var("CS_RESET_CREDITS_CONSUME_URL") {
        return url;
    }
    if std::env::var("CS_RESET_CREDITS_URL").is_ok() {
        return format!("{}/consume", reset_credits_url().trim_end_matches('/'));
    }
    RESET_CREDITS_CONSUME_URL.to_string()
}

/// Extract a short summary from an error message for user-facing display.
/// Looks for "HTTP <status>" patterns; falls back to first line truncated.
fn extract_error_summary(err: &str) -> String {
    // Look for "HTTP 4xx ..." or "HTTP 5xx ..." pattern
    if let Some(pos) = err.find("HTTP ") {
        let rest = &err[pos..];
        // Take until comma, closing paren, or end
        let end = rest.find([',', ')']).unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    // Fallback: first line, truncated
    let first_line = err.lines().next().unwrap_or(err);
    let mut chars = first_line.chars();
    let preview: String = chars.by_ref().take(60).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        first_line.to_string()
    }
}

/// High-level: fetch usage with retry, token refresh, and disk cache.
/// Set `force` to true to bypass cache (e.g., manual refresh).
pub async fn fetch_usage_retried(
    alias: &str,
    profile_path: &Path,
    current_alias: &str,
) -> std::result::Result<UsageInfo, UsageError> {
    fetch_usage_retried_inner(alias, profile_path, current_alias, false).await
}

/// Same as `fetch_usage_retried` but with explicit force flag.
pub async fn fetch_usage_retried_force(
    alias: &str,
    profile_path: &Path,
    current_alias: &str,
) -> std::result::Result<UsageInfo, UsageError> {
    fetch_usage_retried_inner(alias, profile_path, current_alias, true).await
}

fn persist_refreshed_tokens(alias: &str, profile_path: &Path, new_tokens: &RefreshedTokens) {
    if let Err(err) = crate::profile::update_profile_tokens_and_live_if_current(
        alias,
        &new_tokens.id_token,
        &new_tokens.access_token,
        &new_tokens.refresh_token,
    ) {
        warn!(
            "[{alias}] Failed to atomically persist refreshed tokens to {} and live auth: {err}",
            profile_path.display()
        );
    }
}

fn resolve_refreshed_tokens(
    response: RefreshResponse,
    current_id_token: Option<&str>,
    current_access_token: Option<&str>,
    current_refresh_token: &str,
) -> Result<RefreshedTokens> {
    if let Some(err) = response.error {
        anyhow::bail!("token refresh failed: {err}");
    }

    let id_token = response
        .id_token
        .or_else(|| current_id_token.map(str::to_string))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "token refresh response omitted id_token and no existing id_token is available"
            )
        })?;
    let access_token = response
        .access_token
        .or_else(|| current_access_token.map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!("token refresh response omitted access_token and no existing access_token is available"))?;
    let refresh_token = response
        .refresh_token
        .unwrap_or_else(|| current_refresh_token.to_string());

    Ok(RefreshedTokens {
        id_token,
        access_token,
        refresh_token,
    })
}

async fn fetch_usage_retried_inner(
    alias: &str,
    profile_path: &Path,
    _current_alias: &str,
    force: bool,
) -> std::result::Result<UsageInfo, UsageError> {
    if !force {
        if let Some(cached) = crate::cache::get_async(alias).await {
            debug!("{alias}: cache hit");
            return Ok(cached);
        }
        debug!("{alias}: cache miss, fetching from API");
    } else {
        debug!("{alias}: force refresh, bypassing cache");
    }

    let val = auth::read_auth(profile_path).map_err(|e| {
        let detail = format!("failed to read auth file {}: {e}", profile_path.display());
        UsageError {
            summary: "auth file unreadable".into(),
            detail,
        }
    })?;
    let account_info = crate::jwt::parse_account_info(&val);
    let account_id = account_info.account_id;
    let is_fedramp = account_info.is_fedramp;
    let id_token = auth::extract_id_token(&val);
    let (access_token, refresh_token) = auth::extract_tokens(&val);

    let at = match access_token {
        Some(t) => t,
        None => {
            return Err(UsageError {
                summary: "no access_token".into(),
                detail: "no access_token in auth file".into(),
            });
        }
    };

    let mut last_err = String::new();
    let mut last_summary = String::new();
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            debug!("[{alias}] retry attempt {}/{MAX_RETRIES}", attempt + 1);
            tokio::time::sleep(RETRY_DELAY).await;
        }
        match fetch_usage_with_refresh(
            alias,
            &at,
            id_token.as_deref(),
            refresh_token.as_deref(),
            account_id.as_deref(),
            is_fedramp,
        )
        .await
        {
            Ok((usage, refreshed)) => {
                if let Some(new_tokens) = refreshed {
                    persist_refreshed_tokens(alias, profile_path, &new_tokens);
                }
                crate::cache::put_async(alias, &usage).await;
                return Ok(usage);
            }
            Err(e) => {
                let msg = e.to_string();
                warn!(
                    "[{alias}] attempt {}/{MAX_RETRIES} failed: {msg}",
                    attempt + 1
                );
                last_summary = extract_error_summary(&msg);
                last_err = msg;
            }
        }
    }
    Err(UsageError {
        summary: last_summary,
        detail: last_err,
    })
}

/// Fetch usage; on 401/403 automatically refresh the token and retry once.
pub async fn fetch_usage_with_refresh(
    alias: &str,
    access_token: &str,
    id_token: Option<&str>,
    refresh_token: Option<&str>,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> Result<(UsageInfo, Option<RefreshedTokens>)> {
    let client = auth::build_http_client()?;
    let usage_url = usage_url();

    // Pre-refresh: if access_token expires within 60 seconds, refresh proactively.
    if let Some(rt) = refresh_token
        && crate::jwt::is_token_expiring(access_token, 60).unwrap_or(false)
    {
        info!("[{alias}] access token expiring soon, proactively refreshing");

        match do_refresh_token(alias, &client, id_token, Some(access_token), rt).await {
            Ok(new_tokens) => {
                let resp = apply_account_routing_headers(
                    client.get(&usage_url).header(
                        "Authorization",
                        format!("Bearer {}", new_tokens.access_token),
                    ),
                    account_id,
                    is_fedramp,
                )
                .send()
                .await
                .map_err(|e| format_reqwest_error("Usage API request failed", &e))?;

                let status = resp.status();
                debug!("[{alias}] Usage API (after proactive refresh): HTTP {status}");
                if status.is_success() {
                    let body: Value = resp.json().await.map_err(|e| {
                        anyhow::anyhow!("failed to parse usage response (HTTP {status}): {e}")
                    })?;
                    debug!(
                        "[{alias}] Usage API raw body (proactive): {}",
                        crate::auth::redact_sensitive_log_body(&body)
                    );
                    let mut usage = parse_usage_checked(&body)?;
                    enrich_reset_credits(
                        alias,
                        &client,
                        &new_tokens.access_token,
                        account_id,
                        &mut usage,
                    )
                    .await;
                    return Ok((usage, Some(new_tokens)));
                }
                anyhow::bail!("Usage API failed (HTTP {status}) after proactive token refresh");
            }
            Err(e) => {
                info!("[{alias}] proactive token refresh failed, trying with existing token: {e}");
            }
        }
    }

    let resp = apply_account_routing_headers(
        client
            .get(&usage_url)
            .header("Authorization", format!("Bearer {access_token}")),
        account_id,
        is_fedramp,
    )
    .send()
    .await
    .map_err(|e| format_reqwest_error("Usage API request failed", &e))?;

    let status = resp.status();
    debug!("[{alias}] Usage API: HTTP {status}");
    if status.is_success() {
        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("failed to parse usage response (HTTP {status}): {e}"))?;
        debug!(
            "[{alias}] Usage API raw body: {}",
            crate::auth::redact_sensitive_log_body(&body)
        );
        let mut usage = parse_usage_checked(&body)?;
        enrich_reset_credits(alias, &client, access_token, account_id, &mut usage).await;
        return Ok((usage, None));
    }

    // If 401/403 and we have a refresh_token, try to refresh
    if let Some(rt) = refresh_token
        && (status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN)
    {
        info!("[{alias}] got HTTP {status}, attempting token refresh");

        match do_refresh_token(alias, &client, id_token, Some(access_token), rt).await {
            Ok(new_tokens) => {
                let resp2 = apply_account_routing_headers(
                    client.get(&usage_url).header(
                        "Authorization",
                        format!("Bearer {}", new_tokens.access_token),
                    ),
                    account_id,
                    is_fedramp,
                )
                .send()
                .await
                .map_err(|e| format_reqwest_error("Usage API retry request failed", &e))?;

                let status2 = resp2.status();
                debug!("[{alias}] Usage API (after token refresh): HTTP {status2}");
                if status2.is_success() {
                    let body: Value = resp2.json().await.map_err(|e| {
                        anyhow::anyhow!(
                            "failed to parse usage response after refresh (HTTP {status2}): {e}"
                        )
                    })?;
                    let mut usage = parse_usage_checked(&body)?;
                    enrich_reset_credits(
                        alias,
                        &client,
                        &new_tokens.access_token,
                        account_id,
                        &mut usage,
                    )
                    .await;
                    return Ok((usage, Some(new_tokens)));
                }
                anyhow::bail!("Usage API still failed (HTTP {status2}) after token refresh");
            }
            Err(e) => {
                info!("[{alias}] token refresh failed: {e}");
                anyhow::bail!("Usage API failed (HTTP {status}), token refresh also failed: {e}");
            }
        }
    }

    anyhow::bail!("Usage API failed (HTTP {status}), no refresh_token available");
}

pub async fn validate_import_auth(
    val: &mut serde_json::Value,
) -> Result<(UsageInfo, Option<RefreshedTokens>)> {
    if std::env::var("CS_IMPORT_SKIP_USAGE_VALIDATION")
        .ok()
        .as_deref()
        == Some("1")
    {
        return Ok((UsageInfo::default(), None));
    }

    let (access_token, refresh_token) = auth::extract_tokens(val);
    let id_token = auth::extract_id_token(val);
    let account_info = crate::jwt::parse_account_info(val);
    let account_id = account_info.account_id;
    let is_fedramp = account_info.is_fedramp;

    let alias = "import";
    match (access_token, refresh_token) {
        (Some(at), rt) => {
            let (usage, refreshed) = fetch_usage_with_refresh(
                alias,
                &at,
                id_token.as_deref(),
                rt.as_deref(),
                account_id.as_deref(),
                is_fedramp,
            )
            .await?;
            if let Some(tokens) = &refreshed {
                auth::apply_tokens(
                    val,
                    &tokens.id_token,
                    &tokens.access_token,
                    &tokens.refresh_token,
                )?;
            }
            if let Err(err) = crate::workspace::refresh_for_auth(val).await {
                debug!("workspace metadata unavailable while importing: {err}");
            }
            Ok((usage, refreshed))
        }
        (None, Some(rt)) => {
            let client = auth::build_http_client()?;
            let refreshed =
                do_refresh_token(alias, &client, id_token.as_deref(), None, &rt).await?;
            auth::apply_tokens(
                val,
                &refreshed.id_token,
                &refreshed.access_token,
                &refreshed.refresh_token,
            )?;
            let account_id = crate::jwt::parse_account_info(val).account_id;
            let (usage, refreshed_again) = fetch_usage_with_refresh(
                alias,
                &refreshed.access_token,
                Some(&refreshed.id_token),
                Some(&refreshed.refresh_token),
                account_id.as_deref(),
                is_fedramp,
            )
            .await?;
            if let Some(tokens) = &refreshed_again {
                auth::apply_tokens(
                    val,
                    &tokens.id_token,
                    &tokens.access_token,
                    &tokens.refresh_token,
                )?;
            }
            if let Err(err) = crate::workspace::refresh_for_auth(val).await {
                debug!("workspace metadata unavailable while importing: {err}");
            }
            Ok((usage, refreshed_again.or(Some(refreshed))))
        }
        (None, None) => anyhow::bail!("auth.json missing access_token and refresh_token"),
    }
}

/// Build the token refresh request. Codex 0.144.1 sends a JSON body
/// ({client_id, grant_type, refresh_token}) — keep the same shape so the
/// auth server sees requests identical to the real client's.
pub(crate) fn build_refresh_request(
    client: &reqwest::Client,
    token_url: &str,
    refresh_token: &str,
) -> reqwest::RequestBuilder {
    client.post(token_url).json(&serde_json::json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    }))
}

pub(crate) async fn do_refresh_token(
    alias: &str,
    client: &reqwest::Client,
    current_id_token: Option<&str>,
    current_access_token: Option<&str>,
    refresh_token: &str,
) -> Result<RefreshedTokens> {
    let token_url = auth::token_url();
    debug!("[{alias}] sending token refresh request to {token_url}");

    let resp = build_refresh_request(client, &token_url, refresh_token)
        .send()
        .await
        .map_err(|e| format_reqwest_error("token refresh request failed", &e))?;

    let status = resp.status();
    debug!("[{alias}] token refresh response: HTTP {status}");

    // Read raw body first so we can log it on parse failure
    let body_text = resp.text().await.map_err(|e| {
        anyhow::anyhow!("failed to read token refresh response body (HTTP {status}): {e}")
    })?;

    let r: RefreshResponse = serde_json::from_str(&body_text).map_err(|e| {
        // A token refresh body may contain access/refresh/id tokens; redact them
        // before logging so `--debug` output is safe to share in bug reports.
        let redacted = serde_json::from_str::<Value>(&body_text)
            .map(|v| crate::auth::redact_sensitive_log_body(&v))
            .unwrap_or_else(|_| format!("<non-JSON body, {} bytes>", body_text.len()));
        debug!("[{alias}] token refresh parse failure, raw body: {redacted}");
        anyhow::anyhow!("Failed to parse token refresh response (HTTP {status}): {e}")
    })?;

    let refreshed =
        resolve_refreshed_tokens(r, current_id_token, current_access_token, refresh_token)
            .with_context(|| format!("[{alias}] token refresh HTTP {status}"))?;
    info!("[{alias}] token refresh succeeded");
    Ok(refreshed)
}

async fn enrich_reset_credits(
    alias: &str,
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    usage: &mut UsageInfo,
) {
    match fetch_reset_credits(client, access_token, account_id).await {
        Ok((available_count, credits)) => {
            if available_count.is_some() {
                usage.reset_credits_available_count = available_count;
            }
            if !credits.is_empty() {
                usage.reset_credits = credits;
            }
            usage.reset_credits_error = None;
        }
        Err(err) => {
            let msg = err.to_string();
            debug!("[{alias}] reset credits fetch failed: {msg}");
            usage.reset_credits_error = Some(extract_error_summary(&msg));
        }
    }
}

async fn fetch_reset_credits(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
) -> Result<(Option<u64>, Vec<ResetCredit>)> {
    let mut req = client
        .get(reset_credits_url())
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("OpenAI-Beta", "codex-1")
        .header("Originator", "Codex Desktop");

    if let Some(account_id) = account_id.filter(|s| !s.trim().is_empty()) {
        req = req.header("Chatgpt-Account-Id", account_id);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format_reqwest_error("reset credits request failed", &e))?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("reset credits request failed (HTTP {status})");
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("failed to parse reset credits response: {e}"))?;
    let (available_count, credits, valid_shape) = parse_reset_credits_summary(&body);
    if !valid_shape {
        anyhow::bail!("reset credits response missing expected fields");
    }
    Ok((available_count, credits))
}

pub fn earliest_reset_credit(credits: &[ResetCredit]) -> Option<&ResetCredit> {
    credits.iter().min_by_key(|credit| {
        credit
            .expires_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(i64::MAX)
    })
}

pub async fn consume_earliest_reset_credit(
    alias: &str,
    profile_path: &Path,
) -> Result<ConsumedResetCredit> {
    let val = auth::read_auth(profile_path)?;
    let (access_token, _) = auth::extract_tokens(&val);
    let access_token = access_token
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{alias}: auth.json missing access_token"))?;
    let account_id = crate::jwt::parse_account_info(&val).account_id;
    let client = auth::build_http_client()?;

    let (_, credits) = fetch_reset_credits(&client, &access_token, account_id.as_deref()).await?;
    let credit = earliest_reset_credit(&credits)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{alias}: no available reset cards"))?;

    consume_reset_credit(&client, &access_token, account_id.as_deref(), credit).await
}

async fn consume_reset_credit(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    credit: ResetCredit,
) -> Result<ConsumedResetCredit> {
    consume_reset_credit_at_url(
        client,
        access_token,
        account_id,
        credit,
        &reset_credits_consume_url(),
    )
    .await
}

async fn consume_reset_credit_at_url(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    credit: ResetCredit,
    url: &str,
) -> Result<ConsumedResetCredit> {
    // Generate once per user action. Any retry after an ambiguous transport/server
    // failure must identify the same logical redemption to the backend.
    let request_id = redeem_request_id();
    for attempt in 0..MAX_RETRIES {
        let mut req = client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .header("OpenAI-Beta", "codex-1")
            .header("Originator", "Codex Desktop")
            .json(&serde_json::json!({
                "credit_id": &credit.id,
                "redeem_request_id": &request_id,
            }));

        if let Some(account_id) = account_id.filter(|s| !s.trim().is_empty()) {
            req = req.header("Chatgpt-Account-Id", account_id);
        }

        let resp = match req.send().await {
            Ok(resp) => resp,
            Err(error) if attempt + 1 < MAX_RETRIES => {
                debug!(
                    "reset credit consume attempt {}/{} failed before response: {}",
                    attempt + 1,
                    MAX_RETRIES,
                    format_reqwest_error("request failed", &error)
                );
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }
            Err(error) => {
                return Err(format_reqwest_error(
                    "reset credit consume request failed",
                    &error,
                ));
            }
        };
        let status = resp.status();
        if !status.is_success() {
            if (status.is_server_error() || status.as_u16() == 429) && attempt + 1 < MAX_RETRIES {
                debug!(
                    "reset credit consume attempt {}/{} returned HTTP {status}",
                    attempt + 1,
                    MAX_RETRIES
                );
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }
            anyhow::bail!("reset credit consume request failed (HTTP {status})");
        }

        match resp.json::<Value>().await {
            Ok(body) => return parse_consumed_reset_credit(&body, credit),
            Err(error) if attempt + 1 < MAX_RETRIES => {
                debug!(
                    "reset credit consume attempt {}/{} returned invalid JSON: {error}",
                    attempt + 1,
                    MAX_RETRIES
                );
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "failed to parse reset credit consume response: {error}"
                ));
            }
        }
    }

    unreachable!("reset credit retry loop always returns on its final attempt")
}

fn parse_consumed_reset_credit(body: &Value, credit: ResetCredit) -> Result<ConsumedResetCredit> {
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("reset credit consume response missing code"))?;
    if code != "reset" {
        anyhow::bail!("reset credit was not consumed: {code}");
    }

    Ok(ConsumedResetCredit {
        credit,
        code: Some(code.to_string()),
        windows_reset: parse_optional_u64(body.get("windows_reset")),
        redeemed_at: body
            .get("credit")
            .and_then(|v| v.as_object())
            .and_then(|obj| {
                obj.get("redeemed_at")
                    .or_else(|| obj.get("redeemedAt"))
                    .and_then(|v| v.as_str())
            })
            .map(|s| s.to_string()),
    })
}

fn redeem_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let value = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    )
}

fn parse_optional_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_reset_credit(value: &Value) -> Option<ResetCredit> {
    let obj = value.as_object()?;

    let reset_type = obj
        .get("reset_type")
        .or_else(|| obj.get("resetType"))
        .and_then(|v| v.as_str())
        .map(str::trim);
    if let Some(reset_type) = reset_type
        && reset_type != "codex_rate_limits"
    {
        return None;
    }

    let status = obj.get("status").and_then(|v| v.as_str()).map(str::trim);
    if let Some(status) = status
        && status != "available"
    {
        return None;
    }

    let expires_at = obj
        .get("expires_at")
        .or_else(|| obj.get("expiresAt"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let granted_at = obj
        .get("granted_at")
        .or_else(|| obj.get("grantedAt"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Some(ResetCredit {
        id,
        granted_at,
        expires_at,
    })
}

fn parse_reset_credits_summary(body: &Value) -> (Option<u64>, Vec<ResetCredit>, bool) {
    let Some(obj) = body.as_object() else {
        return (None, vec![], false);
    };

    let available_count = parse_optional_u64(
        obj.get("available_count")
            .or_else(|| obj.get("availableCount")),
    );
    let credits = obj
        .get("credits")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(parse_reset_credit).collect())
        .unwrap_or_default();
    let valid_shape = obj.contains_key("credits")
        || obj.contains_key("available_count")
        || obj.contains_key("availableCount");

    (available_count, credits, valid_shape)
}

fn parse_window(val: &Value) -> Option<WindowUsage> {
    // Require used_percent to be present for meaningful scoring data.
    // A window with only resets_at but no used_percent would cause
    // has_5h_data=true with used_5h=0.0, incorrectly treating it as "fully available".
    let used_percent = val.get("used_percent").and_then(|v| v.as_f64());
    used_percent?;
    let resets_at = val.get("reset_at").and_then(|v| v.as_i64());

    Some(WindowUsage {
        used_percent,
        resets_at,
    })
}

/// Whether an account is currently usable (both windows have remaining quota).
pub fn is_available(u: &UsageInfo) -> bool {
    if u.account_limited || (u.primary.is_none() && u.secondary.is_none()) {
        return false;
    }
    if let Some(w) = &u.secondary
        && w.used_percent.unwrap_or(0.0) >= 100.0
    {
        return false;
    }
    if let Some(w) = &u.primary
        && w.used_percent.unwrap_or(0.0) >= 100.0
    {
        return false;
    }
    true
}

/// Eligibility check on a Candidate (reset-aware).
pub fn is_candidate_eligible(c: &Candidate, safety_margin_7d: f64) -> bool {
    if !c.has_5h_data && !c.has_7d_data {
        return false;
    }
    let used_5h = c.effective_used_5h();
    let used_7d = c.effective_used_7d();

    // Gate 1: 5h exhausted (and not past reset)
    if used_5h >= 100.0 {
        return false;
    }
    // Gate 2: 7d exhausted (and not past reset)
    if used_7d >= 100.0 {
        return false;
    }
    // Gate 3: 7d critically low and reset far away
    if c.has_7d_data {
        let remaining_7d = 100.0 - used_7d;
        let critical_pct = (safety_margin_7d * 0.25_f64).max(1.0);
        if remaining_7d < critical_pct {
            let hours_to_reset = c
                .resets_at_7d
                .map(|ts| ((ts - c.now) as f64 / 3600.0).max(0.0))
                .unwrap_or(f64::MAX);
            if hours_to_reset > 48.0 {
                return false;
            }
        }
    }
    // Gate 4: Free plan safety floor
    if c.is_free && c.has_5h_data {
        let remaining_5h = 100.0 - used_5h;
        if remaining_5h < FREE_FLOOR_PCT {
            return false;
        }
    }
    true
}

// ── adaptive scoring algorithm ─────────────────────────────

/// Adaptive scoring algorithm. Pure function, no I/O.
///
/// Automatically adjusts strategy based on pool state. No mode selection needed.
///
/// Components:
///   tier_bonus   — Team priority (0 or 500, configurable)
///   headroom     — Pace-aware effective remaining time (0..1100)
///   drain_value  — Quota that will be wasted if not used before reset (0..300)
///   sustain      — 7d budget-per-window sustainability (-800..0)
///   recency      — Spread usage across accounts (-60..0)
///
/// Pool-adaptive: drain_weight scales with pool_size and exhausted ratio.
pub fn score_unified(c: &Candidate, safety_margin_7d: f64) -> f64 {
    let used_5h = c.effective_used_5h();
    let used_7d = c.effective_used_7d();

    // ── Component A: tier_bonus (0 or 500) ──
    let tier_bonus = if c.is_team && c.team_priority {
        500.0
    } else {
        0.0
    };

    // ── Component B: headroom (0..1100) ──
    // Pace-aware: uses burn rate to project effective remaining time,
    // not just static remaining%.
    let headroom = if !c.has_5h_data {
        50.0
    } else if used_5h >= 100.0 {
        // Exhausted: score by time-to-reset (closer = higher, range 0..500).
        // The 500 ceiling (vs 1000+ for active accounts) is intentional:
        // is_candidate_eligible() marks exhausted accounts as ineligible,
        // and the caller sorts eligible-first. This branch only ranks among
        // ineligible fallback candidates when no eligible account exists.
        match c.resets_at_5h {
            None => 0.0,
            Some(reset_ts) => {
                let remaining_secs = (reset_ts - c.now).max(0) as f64;
                (500.0 - remaining_secs / 60.0).max(0.0)
            }
        }
    } else {
        // Pace-aware headroom: project remaining minutes using burn rate
        let remaining_pct = 100.0 - used_5h;
        match c.resets_at_5h {
            Some(reset_ts) => {
                let remaining_secs = (reset_ts - c.now).max(0) as f64;
                let elapsed_secs = (WINDOW_5H_SECS as f64 - remaining_secs).max(1.0);
                let burn_rate = used_5h / elapsed_secs; // %/sec

                if burn_rate > 0.001 {
                    // Project minutes until exhaustion at current rate
                    let projected_min = (remaining_pct / burn_rate) / 60.0;
                    // Cap at 300 min (5h), normalize to 0..100, add base 1000
                    1000.0 + (projected_min.min(300.0) / 300.0 * 100.0)
                } else {
                    // Near-zero burn rate → effectively full capacity
                    1000.0 + remaining_pct
                }
            }
            None => 1000.0 + remaining_pct,
        }
    };

    // ── Component C: sustain — 7d sustainability (-800..0) ──
    // Uses budget-per-window: how much 7d quota is available per remaining 5h window.
    const RELIEF_WINDOW_HOURS: f64 = 48.0;
    const MAX_RELIEF: f64 = 0.8;

    let sustain = if !c.has_7d_data {
        -50.0
    } else if used_7d >= 100.0 {
        // 7d exhausted: heavy penalty, relieved as reset approaches
        match c.resets_at_7d {
            None => -800.0, // no reset info: maximum penalty
            Some(reset_ts) => {
                let remaining_min = ((reset_ts - c.now).max(0) as f64) / 60.0;
                let relief = (1.0 - remaining_min / 10080.0).clamp(0.0, 1.0);
                -800.0 * (1.0 - relief)
            }
        }
    } else {
        let remaining_7d = 100.0 - used_7d;
        if remaining_7d >= safety_margin_7d {
            0.0
        } else {
            // Compute budget per remaining 5h window
            let budget_penalty = if let Some(reset_ts_7d) = c.resets_at_7d {
                let hours_to_7d_reset = ((reset_ts_7d - c.now) as f64 / 3600.0).max(0.0);
                let remaining_windows = (hours_to_7d_reset / 5.0).max(1.0);
                let budget_per_window = remaining_7d / remaining_windows;
                // If each window gets ≥ safety_margin worth of budget, it's fine
                if budget_per_window >= safety_margin_7d {
                    0.0
                } else {
                    // Shortfall: 0..1, higher = more pressure
                    ((safety_margin_7d - budget_per_window) / safety_margin_7d).clamp(0.0, 1.0)
                }
            } else {
                // No reset time: use simple pressure
                if safety_margin_7d > 0.0 {
                    ((safety_margin_7d - remaining_7d) / safety_margin_7d).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            };

            // Time relief: if 7d resets within 48h, reduce penalty
            let time_relief = c
                .resets_at_7d
                .map(|ts| {
                    let hours = ((ts - c.now) as f64 / 3600.0).max(0.0);
                    if hours < RELIEF_WINDOW_HOURS {
                        (1.0 - hours / RELIEF_WINDOW_HOURS).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);

            let effective = budget_penalty * (1.0 - time_relief * MAX_RELIEF);
            -800.0 * effective
        }
    };

    // ── Component D: drain_value (0..300) ──
    // Only activates when 5h reset is within 60 minutes AND there's quota to waste.
    // Pool-adaptive: larger pools with more available accounts → more aggressive drain.
    const DRAIN_WINDOW_MIN: f64 = 60.0;

    let raw_drain = if c.has_5h_data && used_5h < 100.0 {
        if let Some(reset_ts) = c.resets_at_5h {
            let remaining_min = ((reset_ts - c.now).max(0) as f64) / 60.0;
            if remaining_min <= DRAIN_WINDOW_MIN {
                let remaining_pct = 100.0 - used_5h;
                let urgency =
                    ((DRAIN_WINDOW_MIN - remaining_min) / DRAIN_WINDOW_MIN).clamp(0.0, 1.0);
                // waste = remaining quota × urgency, scaled to 0..300
                (remaining_pct * urgency * 3.0).min(300.0)
            } else {
                0.0
            }
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Pool-adaptive drain weight
    let drain_weight = if c.pool_size <= 2 {
        0.5 // Few accounts: be conservative, don't chase drain
    } else {
        let exhausted_ratio = c.pool_exhausted as f64 / c.pool_size as f64;
        if exhausted_ratio > 0.7 {
            0.3 // Most accounts exhausted: conserve what we have
        } else if c.pool_size >= 5 && exhausted_ratio < 0.3 {
            1.5 // Plenty of backup: drain aggressively
        } else {
            1.0
        }
    };

    let drain_value = raw_drain * drain_weight;

    // ── Component E: recency (-60..0) ──
    // Light spread penalty to avoid hammering the same account
    let recency = if c.last_used == 0 {
        0.0
    } else {
        let seconds_ago = (c.now - c.last_used).max(0) as f64;
        -(60.0 - (seconds_ago / 30.0)).clamp(0.0, 60.0)
    };

    tier_bonus + headroom + sustain + drain_value + recency
}

// ── Shared candidate building and selection ───────────────
//
// CLI `use` and the daemon score the same way through these helpers; only
// the final ranking/selection policy differs per caller.

/// One scored candidate. Pure data, no I/O.
pub struct ScoredCandidate {
    pub candidate: Candidate,
    pub usage: UsageInfo,
    pub score: f64,
}

/// Build and score candidates uniformly: the API `plan_type` is
/// authoritative over the JWT (handles plan downgrades), and
/// `pool_exhausted` counts 5h-exhausted accounts across the whole input.
/// Input order is preserved.
pub fn score_candidates(
    fetched: Vec<(String, UsageInfo, crate::jwt::AccountInfo, i64)>,
    now: i64,
    safety_7d: f64,
    team_priority: bool,
) -> Vec<ScoredCandidate> {
    let pool_size = fetched.len();

    let mut candidates: Vec<(Candidate, UsageInfo)> = fetched
        .into_iter()
        .map(|(alias, u, info, last_used)| {
            let api_plan = u.plan_type.as_deref();
            let is_team = api_plan
                .map(|p| p == "team")
                .unwrap_or_else(|| info.is_team());
            let is_free = api_plan
                .map(|p| p == "free")
                .unwrap_or_else(|| info.is_free());
            let mut candidate = Candidate::from_usage(alias, &u, is_team, is_free, last_used, now);
            candidate.pool_size = pool_size;
            candidate.team_priority = team_priority;
            (candidate, u)
        })
        .collect();

    let pool_exhausted = candidates
        .iter()
        .filter(|(candidate, _)| candidate.effective_used_5h() >= 100.0)
        .count();
    for (candidate, _) in &mut candidates {
        candidate.pool_exhausted = pool_exhausted;
    }

    candidates
        .into_iter()
        .map(|(candidate, usage)| {
            let score = score_unified(&candidate, safety_7d);
            ScoredCandidate {
                candidate,
                usage,
                score,
            }
        })
        .collect()
}

/// Daemon switch policy over already-scored candidates: prefer an eligible
/// candidate that beats `current_score`; fall back to the best ineligible
/// one only when no candidate is eligible at all.
pub fn pick_switch_target<'a>(
    current_score: f64,
    others: &'a [ScoredCandidate],
    safety_7d: f64,
) -> Option<(&'a str, f64)> {
    let mut best_eligible: Option<(&'a str, f64)> = None;
    let mut best_ineligible: Option<(&'a str, f64)> = None;
    let mut any_eligible = false;

    for s in others {
        let eligible = is_candidate_eligible(&s.candidate, safety_7d);
        if eligible {
            any_eligible = true;
            if s.score > current_score && best_eligible.is_none_or(|(_, bs)| s.score > bs) {
                best_eligible = Some((s.candidate.alias.as_str(), s.score));
            }
        } else if s.score > current_score && best_ineligible.is_none_or(|(_, bs)| s.score > bs) {
            best_ineligible = Some((s.candidate.alias.as_str(), s.score));
        }
    }

    best_eligible.or(if !any_eligible { best_ineligible } else { None })
}

fn known_rate_limit_reached_type(body: &Value) -> bool {
    let kind = body.get("rate_limit_reached_type").and_then(|value| {
        value
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| value.as_str())
    });
    matches!(
        kind,
        Some(
            "rate_limit_reached"
                | "workspace_owner_credits_depleted"
                | "workspace_member_credits_depleted"
                | "workspace_owner_usage_limit_reached"
                | "workspace_member_usage_limit_reached"
        )
    )
}

fn parse_usage_checked(body: &Value) -> Result<UsageInfo> {
    let usage = parse_usage(body);
    let credits_has_data = body
        .get("credits")
        .and_then(Value::as_object)
        .is_some_and(|credits| {
            credits
                .get("has_credits")
                .and_then(Value::as_bool)
                .is_some()
                || credits.get("unlimited").and_then(Value::as_bool).is_some()
                || credits.get("balance").is_some_and(|balance| {
                    balance.as_f64().is_some()
                        || balance
                            .as_str()
                            .is_some_and(|value| value.parse::<f64>().is_ok())
                })
        });
    if usage.primary.is_none()
        && usage.secondary.is_none()
        && !usage.account_limited
        && !credits_has_data
    {
        anyhow::bail!("usage response missing recognized quota fields");
    }
    Ok(usage)
}

pub fn parse_usage(body: &Value) -> UsageInfo {
    const SECS_7D: i64 = 7 * 86400; // 604800

    let primary_raw = body
        .pointer("/rate_limit/primary_window")
        .filter(|v| !v.is_null());

    let secondary_raw = body
        .pointer("/rate_limit/secondary_window")
        .filter(|v| !v.is_null());

    let primary_window_secs = primary_raw
        .and_then(|v| v.get("limit_window_seconds"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let primary_parsed = primary_raw.and_then(parse_window);
    let secondary_parsed = secondary_raw.and_then(parse_window);

    // Free accounts (new API): only one window exists, placed in primary_window slot
    // with limit_window_seconds == 604800 (7d). Remap it to secondary so scoring works.
    let (primary, secondary) = if primary_window_secs >= SECS_7D && secondary_parsed.is_none() {
        debug!("parse_usage: primary_window is 7d (free account) — remapping to secondary");
        (None, primary_parsed)
    } else {
        if secondary_raw.is_some() && secondary_parsed.is_none() {
            warn!(
                "parse_usage: secondary_window present but failed to parse (missing used_percent?): {:?}",
                secondary_raw
            );
        }
        (primary_parsed, secondary_parsed)
    };

    debug!(
        "parse_usage: primary={} secondary={}",
        primary.is_some(),
        secondary.is_some()
    );

    // has_credits=false means no pay-per-use credits (Plus/Pro included usage only).
    // Default true for old API format which lacked this field.
    let has_credits = body
        .pointer("/credits/has_credits")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // balance changed from number to string "0" in new API — handle both.
    // Skip entirely when has_credits=false to avoid showing "$0.00" for accounts
    // that simply don't use the pay-per-use credits system.
    let credits_balance = if has_credits {
        body.pointer("/credits/balance").and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    } else {
        None
    };

    let unlimited_credits = body.pointer("/credits/unlimited").and_then(|v| v.as_bool());

    let plan_type = body
        .get("plan_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let account_limited = known_rate_limit_reached_type(body)
        || body
            .pointer("/spend_control/reached")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || body
            .pointer("/rate_limit/limit_reached")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let reset_credits_raw = body
        .get("rate_limit_reset_credits")
        .or_else(|| body.get("rateLimitResetCredits"));
    let (reset_credits_available_count, reset_credits, _) = reset_credits_raw
        .map(parse_reset_credits_summary)
        .unwrap_or((None, vec![], false));

    UsageInfo {
        fetched_at: Some(auth::now_unix_secs()),
        primary,
        secondary,
        credits_balance,
        unlimited_credits,
        plan_type,
        reset_credits_available_count,
        reset_credits,
        reset_credits_error: None,
        account_limited,
    }
}

/// Max number of tokens to refresh opportunistically per CLI invocation.
const OPPORTUNISTIC_REFRESH_LIMIT: usize = 3;
/// Refresh tokens expiring within this many seconds.
const OPPORTUNISTIC_REFRESH_MARGIN: i64 = 1800; // 30 minutes
/// Total wall-clock timeout for all opportunistic refreshes (concurrent).
const OPPORTUNISTIC_TOTAL_TIMEOUT: Duration = Duration::from_secs(8);

/// Opportunistically refresh tokens that are about to expire.
/// Runs concurrently with a bounded total timeout.
/// Errors are logged, not propagated — safe to await at end of CLI commands.
pub async fn refresh_expiring_tokens() {
    let profiles = match crate::profile::list_profiles() {
        Ok(p) => p,
        Err(_) => return,
    };

    let now = auth::now_unix_secs();

    // Collect current tokens for profiles expiring soon.
    let mut candidates: Vec<(
        String,
        std::path::PathBuf,
        Option<String>,
        String,
        String,
        i64,
    )> = Vec::new();
    for alias in &profiles {
        let path = match crate::profile::profile_auth_path(alias) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let val = match auth::read_auth(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (access_token, refresh_token) = auth::extract_tokens(&val);
        let id_token = auth::extract_id_token(&val);
        let Some(at) = access_token else { continue };
        let Some(rt) = refresh_token else { continue };
        let Some(exp) = crate::jwt::token_expires_at(&at) else {
            continue;
        };
        let remaining = exp - now;
        if remaining < OPPORTUNISTIC_REFRESH_MARGIN {
            candidates.push((alias.clone(), path, id_token, at, rt, exp));
        }
    }

    if candidates.is_empty() {
        return;
    }

    // Sort by expiration: soonest first
    candidates.sort_by_key(|c| c.5);
    candidates.truncate(OPPORTUNISTIC_REFRESH_LIMIT);

    let count = candidates.len();
    debug!(
        "opportunistic refresh: {count} token(s) expiring within {}s",
        OPPORTUNISTIC_REFRESH_MARGIN
    );

    // Spawn all refreshes concurrently, bounded by total timeout
    let mut tasks = tokio::task::JoinSet::new();
    for (alias, path, id_token, access_token, rt, exp) in candidates {
        tasks.spawn(async move {
            let remaining = exp - auth::now_unix_secs();
            debug!("[{alias}] token expires in {remaining}s, refreshing");

            let client = match auth::build_http_client() {
                Ok(c) => c,
                Err(e) => {
                    debug!("[{alias}] skipping refresh: {e}");
                    return;
                }
            };

            match do_refresh_token(
                &alias,
                &client,
                id_token.as_deref(),
                Some(&access_token),
                &rt,
            )
            .await
            {
                Ok(new_tokens) => {
                    persist_refreshed_tokens(&alias, &path, &new_tokens);
                    info!("[{alias}] opportunistic token refresh succeeded");
                }
                Err(e) => {
                    debug!("[{alias}] opportunistic token refresh failed: {e}");
                }
            }
        });
    }

    // Wait for all with total timeout — don't block CLI too long
    let _ = tokio::time::timeout(OPPORTUNISTIC_TOTAL_TIMEOUT, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use chrono::DateTime;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_refresh_request_uses_json_body_like_codex() {
        let request = build_refresh_request(
            &reqwest::Client::new(),
            "https://auth.openai.com/oauth/token",
            "refresh-token-value",
        )
        .build()
        .unwrap();

        assert_eq!(
            request
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        let body: serde_json::Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "client_id": crate::auth::CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": "refresh-token-value",
            })
        );
    }

    #[test]
    fn test_account_routing_headers_include_workspace_and_fedramp() {
        let request = apply_account_routing_headers(
            reqwest::Client::new().get("https://example.invalid/usage"),
            Some("workspace-123"),
            true,
        )
        .build()
        .unwrap();

        assert_eq!(
            request
                .headers()
                .get("ChatGPT-Account-ID")
                .and_then(|value| value.to_str().ok()),
            Some("workspace-123")
        );
        assert_eq!(
            request
                .headers()
                .get("X-OpenAI-Fedramp")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
    }

    #[test]
    fn test_refresh_without_id_token_preserves_existing_id_token() {
        let refreshed = resolve_refreshed_tokens(
            RefreshResponse {
                id_token: None,
                access_token: Some("new-access".to_string()),
                refresh_token: None,
                error: None,
            },
            Some("existing-id"),
            Some("existing-access"),
            "existing-refresh",
        )
        .unwrap();

        assert_eq!(refreshed.id_token, "existing-id");
        assert_eq!(refreshed.access_token, "new-access");
        assert_eq!(refreshed.refresh_token, "existing-refresh");
    }

    fn usage_with(primary: Option<WindowUsage>, secondary: Option<WindowUsage>) -> UsageInfo {
        UsageInfo {
            fetched_at: None,
            primary,
            secondary,
            credits_balance: None,
            unlimited_credits: None,
            plan_type: None,
            reset_credits_available_count: None,
            reset_credits: vec![],
            reset_credits_error: None,
            account_limited: false,
        }
    }

    fn window(used_percent: f64, resets_at: Option<i64>) -> WindowUsage {
        WindowUsage {
            used_percent: Some(used_percent),
            resets_at,
        }
    }

    #[test]
    fn test_parse_usage_full_response() {
        let primary_reset = DateTime::parse_from_rfc3339("2026-03-26T10:00:00Z")
            .unwrap()
            .timestamp();
        let secondary_reset = DateTime::parse_from_rfc3339("2026-03-30T00:00:00Z")
            .unwrap()
            .timestamp();
        let body = json!({
            "rate_limit": {
                "primary_window": {
                    "remaining_seconds": 3600,
                    "requests_remaining": 50,
                    "requests_limit": 100,
                    "reset_time": "2026-03-26T10:00:00Z",
                    "used_percent": 50.0,
                    "reset_at": primary_reset
                },
                "secondary_window": {
                    "remaining_seconds": 86400,
                    "requests_remaining": 200,
                    "requests_limit": 500,
                    "reset_time": "2026-03-30T00:00:00Z",
                    "used_percent": 60.0,
                    "reset_at": secondary_reset
                }
            },
            "credits": {
                "balance": 15.50,
                "unlimited": false
            },
            "rate_limit_reset_credits": {
                "available_count": "2"
            }
        });

        let before = auth::now_unix_secs();
        let usage = parse_usage(&body);
        let after = auth::now_unix_secs();

        assert!(matches!(usage.fetched_at, Some(ts) if ts >= before && ts <= after));
        assert_eq!(
            usage.primary.as_ref().and_then(|w| w.used_percent),
            Some(50.0)
        );
        assert_eq!(
            usage.primary.as_ref().and_then(|w| w.resets_at),
            Some(primary_reset)
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.used_percent),
            Some(60.0)
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.resets_at),
            Some(secondary_reset)
        );
        assert_eq!(usage.credits_balance, Some(15.5));
        assert_eq!(usage.unlimited_credits, Some(false));
        assert_eq!(usage.reset_credits_available_count, Some(2));
    }

    #[test]
    fn test_parse_usage_reset_credit_details() {
        let usage = parse_usage(&json!({
            "rate_limit_reset_credits": {
                "available_count": 2,
                "credits": [
                    {
                        "id": "cred_1",
                        "reset_type": "codex_rate_limits",
                        "status": "available",
                        "granted_at": "2026-07-01T00:00:00Z",
                        "expires_at": "2026-07-08T00:00:00Z"
                    },
                    {
                        "id": "cred_2",
                        "reset_type": "codex_rate_limits",
                        "status": "consumed",
                        "expires_at": "2026-07-08T00:00:00Z"
                    }
                ]
            }
        }));

        assert_eq!(usage.reset_credits_available_count, Some(2));
        assert_eq!(usage.reset_credits.len(), 1);
        assert_eq!(usage.reset_credits[0].id, "cred_1");
        assert_eq!(
            usage.reset_credits[0].expires_at.as_deref(),
            Some("2026-07-08T00:00:00Z")
        );
    }

    #[test]
    fn test_reset_credit_without_expiry_is_preserved_and_sorted_last() {
        let expiring = parse_reset_credit(&json!({
            "id": "expiring",
            "status": "available",
            "expires_at": "2026-07-08T00:00:00Z"
        }))
        .unwrap();
        let no_expiry = parse_reset_credit(&json!({
            "id": "no-expiry",
            "status": "available",
            "expires_at": null
        }))
        .unwrap();
        let credits = vec![no_expiry, expiring];

        assert_eq!(credits[0].expires_at, None);
        assert_eq!(earliest_reset_credit(&credits).unwrap().id, "expiring");
    }

    #[test]
    fn test_consume_outcome_only_accepts_reset() {
        let credit = ResetCredit {
            id: "credit-1".to_string(),
            granted_at: None,
            expires_at: None,
        };

        let consumed = parse_consumed_reset_credit(
            &json!({"code": "reset", "windows_reset": 2}),
            credit.clone(),
        )
        .unwrap();
        assert_eq!(consumed.code.as_deref(), Some("reset"));

        for code in ["nothing_to_reset", "no_credit", "already_redeemed"] {
            let error =
                parse_consumed_reset_credit(&json!({"code": code}), credit.clone()).unwrap_err();
            assert!(error.to_string().contains(code));
        }
    }

    #[tokio::test]
    async fn test_consume_retry_reuses_redeem_request_id() {
        let request_ids = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&request_ids);
        let app = axum::Router::new().route(
            "/consume",
            post(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured);
                async move {
                    let request_id = body
                        .get("redeem_request_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let attempt = {
                        let mut ids = captured.lock().unwrap();
                        ids.push(request_id);
                        ids.len()
                    };
                    if attempt == 1 {
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    } else {
                        Json(json!({"code": "reset", "windows_reset": 2})).into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let result = consume_reset_credit_at_url(
            &reqwest::Client::new(),
            "access-token",
            Some("workspace-123"),
            ResetCredit {
                id: "credit-1".to_string(),
                granted_at: None,
                expires_at: None,
            },
            &format!("http://{address}/consume"),
        )
        .await
        .unwrap();
        server.abort();

        assert_eq!(result.code.as_deref(), Some("reset"));
        let ids = request_ids.lock().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(!ids[0].is_empty());
        assert_eq!(ids[0], ids[1]);
    }

    #[test]
    fn test_parse_usage_unlimited_credits() {
        let usage = parse_usage(&json!({
            "credits": {
                "balance": 15.50,
                "unlimited": true
            }
        }));

        assert_eq!(usage.credits_balance, Some(15.5));
        assert_eq!(usage.unlimited_credits, Some(true));
    }

    #[test]
    fn test_parse_usage_no_credits() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25.0,
                    "reset_at": 123
                }
            }
        }));

        assert_eq!(usage.credits_balance, None);
        assert_eq!(usage.unlimited_credits, None);
    }

    #[test]
    fn test_parse_usage_has_credits_false_hides_balance() {
        // New API: plus accounts return has_credits=false with balance="0" (string).
        // We must NOT show $0.00 for these accounts.
        let usage = parse_usage(&json!({
            "plan_type": "plus",
            "credits": {
                "has_credits": false,
                "unlimited": false,
                "balance": "0"
            }
        }));

        assert_eq!(
            usage.credits_balance, None,
            "has_credits=false must suppress balance"
        );
        assert_eq!(usage.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn test_parse_usage_balance_string() {
        // New API: balance is a string when has_credits=true
        let usage = parse_usage(&json!({
            "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": "5.25"
            }
        }));

        assert_eq!(usage.credits_balance, Some(5.25));
    }

    #[test]
    fn test_parse_usage_free_account_single_window() {
        // New API: free accounts have one 7d window in primary_window slot.
        // Must be remapped to secondary so scoring treats it as 7d data.
        let usage = parse_usage(&json!({
            "plan_type": "free",
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {
                    "used_percent": 100,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 437896,
                    "reset_at": 1778468889i64
                },
                "secondary_window": null
            }
        }));

        assert!(
            usage.primary.is_none(),
            "free account must have no 5h window"
        );
        assert!(
            usage.secondary.is_some(),
            "free account 7d data must be in secondary"
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.used_percent),
            Some(100.0)
        );
        assert_eq!(usage.plan_type.as_deref(), Some("free"));
    }

    #[test]
    fn test_parse_usage_null_windows() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "primary_window": null,
                "secondary_window": null
            }
        }));

        assert!(usage.primary.is_none());
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn test_parse_usage_empty_response() {
        let usage = parse_usage(&json!({}));

        assert!(usage.primary.is_none());
        assert!(usage.secondary.is_none());
        assert_eq!(usage.credits_balance, None);
        assert_eq!(usage.unlimited_credits, None);
    }

    #[test]
    fn test_checked_usage_rejects_empty_or_drifted_response() {
        assert!(parse_usage_checked(&json!({})).is_err());
        assert!(parse_usage_checked(&json!({"unexpected": true})).is_err());
        assert!(parse_usage_checked(&json!({"credits": {"balance": null}})).is_err());
    }

    #[test]
    fn test_checked_usage_empty_object_error_message_names_missing_fields() {
        let err = parse_usage_checked(&json!({})).expect_err("empty body must be rejected");
        assert!(
            err.to_string().contains("missing recognized quota fields"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_checked_usage_drifted_response_is_rejected() {
        let err = parse_usage_checked(&json!({
            "some_new_field": "unrecognized",
            "another": { "nested": 1 }
        }))
        .expect_err("structurally drifted body must be rejected");
        assert!(
            err.to_string().contains("missing recognized quota fields"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_default_usage_is_not_available() {
        assert!(!is_available(&UsageInfo::default()));
        let candidate = Candidate::from_usage(
            "empty".to_string(),
            &UsageInfo::default(),
            false,
            false,
            0,
            1,
        );
        assert!(!is_candidate_eligible(&candidate, 20.0));
    }

    #[test]
    fn test_parse_usage_marks_known_rate_limit_reached_type_as_limited() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 10.0}
            },
            "rate_limit_reached_type": {
                "type": "workspace_member_usage_limit_reached"
            }
        }));

        assert!(usage.account_limited);
        assert!(!is_available(&usage));
    }

    #[test]
    fn test_parse_usage_marks_reached_spend_control_as_limited() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 10.0}
            },
            "spend_control": {"reached": true}
        }));

        assert!(usage.account_limited);
        assert!(!is_available(&usage));
    }

    #[test]
    fn test_parse_usage_ignores_unknown_limit_reason() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 10.0}
            },
            "rate_limit_reached_type": {"type": "future_reason"},
            "spend_control": {"reached": false}
        }));

        assert!(!usage.account_limited);
        assert!(is_available(&usage));
    }

    #[test]
    fn test_is_available_both_under_100() {
        let usage = usage_with(
            Some(window(50.0, Some(1_000))),
            Some(window(30.0, Some(2_000))),
        );

        assert!(is_available(&usage));
    }

    #[test]
    fn test_is_available_primary_exhausted() {
        let usage = usage_with(
            Some(window(100.0, Some(1_000))),
            Some(window(30.0, Some(2_000))),
        );

        assert!(!is_available(&usage));
    }

    #[test]
    fn test_is_available_secondary_exhausted() {
        let usage = usage_with(
            Some(window(50.0, Some(1_000))),
            Some(window(100.0, Some(2_000))),
        );

        assert!(!is_available(&usage));
    }

    #[test]
    fn test_is_available_no_data() {
        assert!(!is_available(&UsageInfo::default()));
    }

    // ── adaptive scoring tests ──

    fn make_candidate(
        alias: &str,
        used_5h: f64,
        reset_5h: Option<i64>,
        used_7d: f64,
        reset_7d: Option<i64>,
    ) -> Candidate {
        Candidate {
            alias: alias.to_string(),
            used_5h,
            resets_at_5h: reset_5h,
            used_7d,
            resets_at_7d: reset_7d,
            has_5h_data: true,
            has_7d_data: true,
            is_team: false,
            is_free: false,
            last_used: 0,
            now: 1_000_000,
            pool_size: 5,
            pool_exhausted: 0,
            team_priority: true,
        }
    }

    fn usage_with_5h(used_percent: f64, resets_at: i64, plan_type: Option<&str>) -> UsageInfo {
        UsageInfo {
            primary: Some(WindowUsage {
                used_percent: Some(used_percent),
                resets_at: Some(resets_at),
            }),
            secondary: Some(WindowUsage {
                used_percent: Some(10.0),
                resets_at: Some(resets_at + 5 * 86400),
            }),
            plan_type: plan_type.map(|p| p.to_string()),
            ..UsageInfo::default()
        }
    }

    #[test]
    fn test_score_candidates_api_plan_overrides_jwt_and_counts_pool_exhausted() {
        let now = 1_000_000i64;
        let jwt_team = crate::jwt::AccountInfo {
            plan_type: Some("team".to_string()),
            ..Default::default()
        };
        let items = vec![
            // API says free although the JWT still claims team (plan downgrade)
            (
                "downgraded".to_string(),
                usage_with_5h(100.0, now + 3600, Some("free")),
                jwt_team,
                0,
            ),
            // No API plan — JWT (default: not team/free) applies
            (
                "healthy".to_string(),
                usage_with_5h(20.0, now + 3600, None),
                Default::default(),
                0,
            ),
        ];

        let scored = score_candidates(items, now, 20.0, true);

        assert_eq!(scored.len(), 2);
        assert_eq!(scored[0].candidate.alias, "downgraded"); // input order preserved
        assert!(scored[0].candidate.is_free);
        assert!(!scored[0].candidate.is_team);
        // One exhausted account (100% 5h), visible to every candidate
        assert_eq!(scored[0].candidate.pool_exhausted, 1);
        assert_eq!(scored[1].candidate.pool_exhausted, 1);
        assert_eq!(scored[1].candidate.pool_size, 2);
    }

    fn scored(candidate: Candidate, safety_7d: f64) -> ScoredCandidate {
        let score = score_unified(&candidate, safety_7d);
        ScoredCandidate {
            candidate,
            usage: UsageInfo::default(),
            score,
        }
    }

    #[test]
    fn test_pick_switch_target_prefers_eligible_above_current() {
        let now = 1_000_000i64;
        let current = make_candidate("current", 90.0, Some(now + 3600), 50.0, Some(now + 86400));
        let good = make_candidate("good", 10.0, Some(now + 3600), 10.0, Some(now + 5 * 86400));
        let current_score = score_unified(&current, 20.0);

        let others = vec![scored(good, 20.0)];
        let pick = pick_switch_target(current_score, &others, 20.0);
        assert_eq!(pick.map(|(a, _)| a), Some("good"));
    }

    #[test]
    fn test_pick_switch_target_ignores_ineligible_when_an_eligible_exists() {
        let now = 1_000_000i64;
        let current = make_candidate("current", 90.0, Some(now + 3600), 50.0, Some(now + 86400));
        let current_score = score_unified(&current, 20.0);

        // Eligible but worse than current; ineligible (7d over safety margin) better.
        let weak_eligible =
            make_candidate("weak", 95.0, Some(now + 3600), 40.0, Some(now + 5 * 86400));
        let strong_ineligible = make_candidate(
            "strong",
            0.0,
            Some(now + 18000),
            95.0,
            Some(now + 5 * 86400),
        );

        let others = vec![scored(weak_eligible, 20.0), scored(strong_ineligible, 20.0)];
        // An eligible candidate exists, so the ineligible one must not be picked,
        // and the eligible one does not beat current — no switch.
        assert!(pick_switch_target(current_score, &others, 20.0).is_none());
    }

    #[test]
    fn test_pick_switch_target_falls_back_when_nothing_is_eligible() {
        let now = 1_000_000i64;
        let current = make_candidate("current", 100.0, Some(now + 3600), 96.0, Some(now + 86400));
        let current_score = score_unified(&current, 20.0);

        let ineligible = make_candidate(
            "fallback",
            0.0,
            Some(now + 18000),
            95.0,
            Some(now + 5 * 86400),
        );
        let others = vec![scored(ineligible, 20.0)];

        let pick = pick_switch_target(current_score, &others, 20.0);
        assert_eq!(pick.map(|(a, _)| a), Some("fallback"));
    }

    #[test]
    fn test_adaptive_prefers_more_remaining() {
        let now = 1_000_000i64;
        let a = make_candidate("a", 30.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        let b = make_candidate("b", 60.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        assert!(score_unified(&a, 20.0) > score_unified(&b, 20.0));
    }

    #[test]
    fn test_adaptive_team_priority_dominates() {
        let now = 1_000_000i64;
        // Non-team with 0% used vs Team with 50% used → Team wins with priority
        let a = make_candidate("a", 0.0, Some(now + 18000), 10.0, Some(now + 5 * 86400));
        let mut b = make_candidate("b", 50.0, Some(now + 7200), 10.0, Some(now + 5 * 86400));
        b.is_team = true;
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(
            sb > sa,
            "team account should beat non-team even with worse 5h: {sb} > {sa}"
        );
    }

    #[test]
    fn test_adaptive_team_priority_disabled() {
        let now = 1_000_000i64;
        // With team_priority=false, Team should not get +500 bonus
        let mut a = make_candidate("a", 0.0, Some(now + 18000), 10.0, Some(now + 5 * 86400));
        a.team_priority = false;
        let mut b = make_candidate("b", 50.0, Some(now + 7200), 10.0, Some(now + 5 * 86400));
        b.is_team = true;
        b.team_priority = false;
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(
            sa > sb,
            "without team_priority, more remaining should win: {sa} > {sb}"
        );
    }

    #[test]
    fn test_adaptive_drain_near_reset() {
        let now = 1_000_000i64;
        // Account A: 40% used, resets in 30 min (within drain window)
        let a = make_candidate("a", 40.0, Some(now + 1800), 20.0, Some(now + 5 * 86400));
        // Account B: 40% used, resets in 4h (outside drain window)
        let b = make_candidate("b", 40.0, Some(now + 14400), 20.0, Some(now + 5 * 86400));
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(
            sa > sb,
            "near-reset account should score higher due to drain: {sa} > {sb}"
        );
    }

    #[test]
    fn test_adaptive_no_drain_outside_window() {
        let now = 1_000_000i64;
        // Both accounts reset in 2h+ (outside 60-min drain window)
        // A: 40% used, resets in 2h → elapsed 3h → burn=40/3h → low rate, more headroom
        // B: 40% used, resets in 4h → elapsed 1h → burn=40/1h → high rate, less headroom
        let a = make_candidate("a", 40.0, Some(now + 7200), 20.0, Some(now + 5 * 86400));
        let b = make_candidate("b", 40.0, Some(now + 14400), 20.0, Some(now + 5 * 86400));
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(
            sa > 1000.0 && sb > 1000.0,
            "both should be usable: {sa}, {sb}"
        );
        // A consumed 40% over 3h (lower burn rate) → more projected headroom
        assert!(sa > sb, "lower burn rate gives more headroom: {sa} > {sb}");
    }

    #[test]
    fn test_adaptive_7d_critical_overrides_5h() {
        let now = 1_000_000i64;
        let a = make_candidate("a", 0.0, Some(now + 18000), 95.0, Some(now + 6 * 86400));
        let b = make_candidate("b", 50.0, Some(now + 7200), 30.0, Some(now + 5 * 86400));
        assert!(
            score_unified(&b, 20.0) > score_unified(&a, 20.0),
            "7d-critical should lose"
        );
    }

    #[test]
    fn test_adaptive_7d_budget_per_window() {
        let now = 1_000_000i64;
        // Account A: 7d 15% remaining, resets in 3 windows (15h) → 5%/window (tight)
        let a = make_candidate("a", 30.0, Some(now + 3600), 85.0, Some(now + 15 * 3600));
        // Account B: 7d 15% remaining, resets in 1 window (5h) → 15%/window (ok)
        let b = make_candidate("b", 30.0, Some(now + 3600), 85.0, Some(now + 5 * 3600));
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(
            sb > sa,
            "higher budget-per-window should score better: {sb} > {sa}"
        );
    }

    #[test]
    fn test_adaptive_recency_breaks_tie() {
        let now = 1_000_000i64;
        let mut a = make_candidate("a", 40.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        a.last_used = now - 5; // used 5 seconds ago
        let mut b = make_candidate("b", 40.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        b.last_used = now - 1200; // used 20 minutes ago
        assert!(
            score_unified(&b, 20.0) > score_unified(&a, 20.0),
            "recently-used should score lower"
        );
    }

    #[test]
    fn test_adaptive_reset_aware() {
        let now = 1_000_000i64;
        let a = make_candidate("a", 80.0, Some(now - 600), 20.0, Some(now + 5 * 86400));
        let score = score_unified(&a, 20.0);
        assert!(
            score > 1000.0,
            "past-reset account should score as fully available, got {score}"
        );
    }

    #[test]
    fn test_adaptive_exhausted_scores_low() {
        let now = 1_000_000i64;
        let a = make_candidate("a", 100.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        let b = make_candidate("b", 50.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(sb > sa, "exhausted should score much lower: {sb} > {sa}");
        assert!(sa < 500.0, "exhausted score should be low: {sa}");
    }

    #[test]
    fn test_adaptive_pool_exhausted_conservative_drain() {
        let now = 1_000_000i64;
        // Most accounts exhausted → drain weight should be low
        let mut a = make_candidate("a", 40.0, Some(now + 1800), 20.0, Some(now + 5 * 86400));
        a.pool_size = 10;
        a.pool_exhausted = 8; // 80% exhausted
        let mut b = make_candidate("b", 40.0, Some(now + 1800), 20.0, Some(now + 5 * 86400));
        b.pool_size = 10;
        b.pool_exhausted = 1; // 10% exhausted
        // Both should have drain but b's pool allows more aggressive drain
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        assert!(sb > sa, "healthy pool should allow more drain: {sb} > {sa}");
    }

    #[test]
    fn test_adaptive_free_floor_ineligible() {
        let now = 1_000_000i64;
        let mut c = make_candidate("free1", 70.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        c.is_free = true;
        assert!(!is_candidate_eligible(&c, 20.0));
    }

    #[test]
    fn test_adaptive_no_data_low_score() {
        let c = Candidate {
            alias: "unknown".to_string(),
            used_5h: 0.0,
            resets_at_5h: None,
            used_7d: 0.0,
            resets_at_7d: None,
            has_5h_data: false,
            has_7d_data: false,
            is_team: false,
            is_free: false,
            last_used: 0,
            now: 1_000_000,
            pool_size: 1,
            pool_exhausted: 0,
            team_priority: true,
        };
        // headroom=50 (no 5h data) + sustain=-50 (no 7d data) = 0
        assert_eq!(
            score_unified(&c, 20.0),
            0.0,
            "no-data account should score exactly 0"
        );
    }

    #[test]
    fn test_adaptive_both_windows_exhausted() {
        let now = 1_000_000i64;
        // 5h exhausted (no reset info) + 7d exhausted (resets in 7 days)
        let mut c = make_candidate("both_dead", 100.0, None, 100.0, Some(now + 7 * 86400));
        c.has_5h_data = true;
        c.has_7d_data = true;
        let s = score_unified(&c, 20.0);
        // headroom=0 (exhausted, no reset), sustain should still be heavily negative
        assert!(
            s < -700.0,
            "doubly-exhausted account must score very low, got {s}"
        );
    }

    #[test]
    fn test_adaptive_both_windows_exhausted_no_reset_info() {
        // Worst case: both exhausted, no reset info at all
        let c = Candidate {
            alias: "dead".to_string(),
            used_5h: 100.0,
            resets_at_5h: None,
            used_7d: 100.0,
            resets_at_7d: None,
            has_5h_data: true,
            has_7d_data: true,
            is_team: false,
            is_free: false,
            last_used: 0,
            now: 1_000_000,
            pool_size: 1,
            pool_exhausted: 1,
            team_priority: false,
        };
        let s = score_unified(&c, 20.0);
        assert!(
            s < -700.0,
            "doubly-exhausted no-reset account must score very low, got {s}"
        );
    }

    #[test]
    fn test_adaptive_pace_aware_headroom() {
        let now = 1_000_000i64;
        // Account A: 30% used, resets in 4h → elapsed 1h → burn=30%/3600s (fast)
        // projected exhaustion = 70 / (30/3600) / 60 ≈ 140 min
        let a = make_candidate("a", 30.0, Some(now + 4 * 3600), 20.0, Some(now + 5 * 86400));
        // Account B: 30% used, resets in 1h → elapsed 4h → burn=30%/14400s (slow)
        // projected exhaustion = 70 / (30/14400) / 60 ≈ 560 min → capped 300 min
        let b = make_candidate("b", 30.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        let sa = score_unified(&a, 20.0);
        let sb = score_unified(&b, 20.0);
        // B has slower burn rate → higher projected exhaustion → higher headroom
        assert!(
            sb > sa,
            "slower burn rate should give higher headroom: {sb} > {sa}"
        );
    }

    #[test]
    fn test_candidate_eligible_basic() {
        let now = 1_000_000i64;
        let c = make_candidate("ok", 30.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        assert!(is_candidate_eligible(&c, 20.0));
    }

    #[test]
    fn test_candidate_ineligible_5h_exhausted() {
        let now = 1_000_000i64;
        let c = make_candidate("ex", 100.0, Some(now + 3600), 20.0, Some(now + 5 * 86400));
        assert!(!is_candidate_eligible(&c, 20.0));
    }

    #[test]
    fn test_candidate_ineligible_7d_critical_far() {
        let now = 1_000_000i64;
        // 7d at 97% (3% remaining < critical 5%), resets in 5 days
        let c = make_candidate("crit", 30.0, Some(now + 3600), 97.0, Some(now + 5 * 86400));
        assert!(!is_candidate_eligible(&c, 20.0));
    }

    #[test]
    fn test_candidate_eligible_7d_critical_near_reset() {
        let now = 1_000_000i64;
        // 7d at 97%, but resets in 12h → still eligible
        let c = make_candidate("near", 30.0, Some(now + 3600), 97.0, Some(now + 12 * 3600));
        assert!(is_candidate_eligible(&c, 20.0));
    }

    #[test]
    fn test_visible_pace_percent_hidden_when_ui_shows_zero_left() {
        let w = WindowUsage {
            used_percent: Some(99.6),
            resets_at: Some(auth::now_unix_secs() + 3600),
        };
        assert_eq!(visible_pace_percent(&w, WINDOW_5H_SECS), None);
    }

    #[test]
    fn test_visible_pace_percent_shown_when_remaining_exists() {
        let w = WindowUsage {
            used_percent: Some(99.4),
            resets_at: Some(auth::now_unix_secs() + 3600),
        };
        assert!(visible_pace_percent(&w, WINDOW_5H_SECS).is_some());
    }

    #[test]
    fn test_warmup_window_active_requires_elapsed_threshold() {
        let now = 1_000_000i64;
        let just_started = WindowUsage {
            used_percent: Some(1.0),
            resets_at: Some(now + WINDOW_5H_SECS - 60),
        };
        let past_threshold = WindowUsage {
            used_percent: Some(1.0),
            resets_at: Some(now + WINDOW_5H_SECS - MIN_WARMUP_ELAPSED_SECS),
        };

        assert!(!warmup_window_active(&just_started, WINDOW_5H_SECS, now));
        assert!(warmup_window_active(&past_threshold, WINDOW_5H_SECS, now));
    }

    #[test]
    fn test_warmup_window_active_requires_real_usage() {
        let now = 1_000_000i64;
        let no_usage = WindowUsage {
            used_percent: Some(0.0),
            resets_at: Some(now + WINDOW_5H_SECS - MIN_WARMUP_ELAPSED_SECS),
        };
        let no_reset = WindowUsage {
            used_percent: Some(1.0),
            resets_at: None,
        };

        assert!(!warmup_window_active(&no_usage, WINDOW_5H_SECS, now));
        assert!(!warmup_window_active(&no_reset, WINDOW_5H_SECS, now));
    }

    #[test]
    fn test_paid_account_with_expired_5h_but_active_7d_is_not_already_warmed() {
        // Regression: previously OR-ed primary and secondary, so a paid account
        // whose 7d window was still active (the normal case after any real use)
        // would never re-warm after its 5h window expired.
        let now = 1_000_000i64;
        let expired_5h = WindowUsage {
            used_percent: Some(99.0),
            resets_at: Some(now - 60), // already reset server-side
        };
        let active_7d = WindowUsage {
            used_percent: Some(40.0),
            resets_at: Some(now + WINDOW_7D_SECS - MIN_WARMUP_ELAPSED_SECS),
        };
        let u = UsageInfo {
            primary: Some(expired_5h),
            secondary: Some(active_7d),
            ..Default::default()
        };
        assert!(!usage_has_active_warmup_window(&u, now));
    }

    #[test]
    fn test_paid_account_with_active_5h_is_already_warmed() {
        let now = 1_000_000i64;
        let active_5h = WindowUsage {
            used_percent: Some(20.0),
            resets_at: Some(now + WINDOW_5H_SECS - MIN_WARMUP_ELAPSED_SECS),
        };
        let u = UsageInfo {
            primary: Some(active_5h),
            secondary: None,
            ..Default::default()
        };
        assert!(usage_has_active_warmup_window(&u, now));
    }

    #[test]
    fn test_free_account_falls_back_to_7d_window() {
        // Free accounts have primary=None (remapped to secondary in parse_usage).
        let now = 1_000_000i64;
        let active_7d = WindowUsage {
            used_percent: Some(10.0),
            resets_at: Some(now + WINDOW_7D_SECS - MIN_WARMUP_ELAPSED_SECS),
        };
        let u = UsageInfo {
            primary: None,
            secondary: Some(active_7d),
            ..Default::default()
        };
        assert!(usage_has_active_warmup_window(&u, now));
    }
}
