use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::auth::{self, CLIENT_ID, format_reqwest_error};

use super::parse::parse_usage_checked;
use super::reset_credits::enrich_reset_credits;
use super::{
    MAX_RETRIES, RETRY_DELAY, RefreshedTokens, TerminalAuthError, UsageError, UsageFetchOutcome,
    UsageInfo,
};

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

/// The auth server reports failures in two shapes: the OAuth 2.0 standard
/// `{"error": "invalid_grant", "error_description": "..."}` and OpenAI's
/// `{"error": {"code": ..., "message": ..., "type": ...}}`. Accept both, or the
/// whole response fails to deserialize and the actionable server message is
/// replaced by a serde type error.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RefreshError {
    Code(String),
    Detail {
        code: Option<String>,
        message: Option<String>,
        #[serde(rename = "type")]
        kind: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<RefreshError>,
    error_description: Option<String>,
}

impl RefreshResponse {
    /// Normalize both wire shapes to `(code, message)`.
    fn error_parts(&self) -> Option<(String, Option<String>)> {
        match self.error.as_ref()? {
            RefreshError::Code(code) => Some((code.clone(), self.error_description.clone())),
            RefreshError::Detail {
                code,
                message,
                kind,
            } => Some((
                code.clone()
                    .or_else(|| kind.clone())
                    .unwrap_or_else(|| "unknown_error".to_string()),
                message.clone().or_else(|| self.error_description.clone()),
            )),
        }
    }
}

/// Auth-server verdicts no retry can change, independent of HTTP status.
const TERMINAL_AUTH_CODES: &[&str] = &[
    "refresh_token_reused",
    "invalid_grant",
    "invalid_client",
    "unauthorized_client",
    "access_denied",
];

/// A 4xx from the token endpoint means the credential itself was rejected, so
/// replaying it only re-triggers reuse detection. 429/408 are load/timing
/// signals and stay retryable.
fn is_terminal_auth_failure(code: &str, status: reqwest::StatusCode) -> bool {
    if matches!(
        status,
        reqwest::StatusCode::TOO_MANY_REQUESTS | reqwest::StatusCode::REQUEST_TIMEOUT
    ) {
        return false;
    }
    TERMINAL_AUTH_CODES.contains(&code) || status.is_client_error()
}

fn format_refresh_error(code: &str, message: Option<&str>) -> String {
    match message {
        Some(message) => format!("{code}: {message}"),
        None => code.to_string(),
    }
}

fn usage_url() -> String {
    std::env::var("CS_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string())
}

fn token_needs_refresh(access_token: &str, id_token: Option<&str>, margin_secs: i64) -> bool {
    crate::jwt::is_token_expiring(access_token, margin_secs).unwrap_or(false)
        || id_token
            .is_some_and(|token| crate::jwt::is_token_expiring(token, margin_secs).unwrap_or(false))
}

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Extract a short summary from an error message for user-facing display.
/// Looks for "HTTP <status>" patterns; falls back to first line truncated.
pub(super) fn extract_error_summary(err: &str) -> String {
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
    status: reqwest::StatusCode,
    current_id_token: Option<&str>,
    current_access_token: Option<&str>,
    current_refresh_token: &str,
) -> Result<RefreshedTokens> {
    if let Some((code, message)) = response.error_parts() {
        if is_terminal_auth_failure(&code, status) {
            return Err(TerminalAuthError { code, message }.into());
        }
        anyhow::bail!(
            "token refresh failed: {}",
            format_refresh_error(&code, message.as_deref())
        );
    }

    // A non-2xx without a recognizable error body still means no tokens were
    // issued; falling through would "succeed" by echoing the current tokens.
    if !status.is_success() {
        let code = format!("http_{}", status.as_u16());
        if is_terminal_auth_failure(&code, status) {
            return Err(TerminalAuthError {
                code,
                message: None,
            }
            .into());
        }
        anyhow::bail!("token refresh failed: HTTP {status}");
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
    let mut id_token = auth::extract_id_token(&val);
    let (access_token, refresh_token) = auth::extract_tokens(&val);
    let mut refresh_token = refresh_token;

    let mut at = match access_token {
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
        let outcome = fetch_usage_with_refresh(
            alias,
            &at,
            id_token.as_deref(),
            refresh_token.as_deref(),
            account_id.as_deref(),
            is_fedramp,
        )
        .await;

        // The auth server rotates `refresh_token` on every use and rejects the
        // previous one as reused. Persist and adopt the new credentials before
        // looking at the result, or the next attempt would replay a dead token
        // and turn a transient failure into a permanent lockout.
        if let Some(new_tokens) = &outcome.refreshed {
            persist_refreshed_tokens(alias, profile_path, new_tokens);
            at = new_tokens.access_token.clone();
            id_token = Some(new_tokens.id_token.clone());
            refresh_token = Some(new_tokens.refresh_token.clone());
        }

        match outcome.result {
            Ok(usage) => {
                crate::cache::put_async(alias, &usage).await;
                return Ok(usage);
            }
            Err(e) => {
                let msg = format!("{e:#}");
                warn!(
                    "[{alias}] attempt {}/{MAX_RETRIES} failed: {msg}",
                    attempt + 1
                );
                if let Some(terminal) = e.downcast_ref::<TerminalAuthError>() {
                    return Err(UsageError {
                        summary: terminal.summary(),
                        detail: msg,
                    });
                }
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
///
/// Returns tokens and result separately: a rotated `refresh_token` is the only
/// credential the auth server will still accept, so it is reported even when
/// the usage call afterwards failed.
pub async fn fetch_usage_with_refresh(
    alias: &str,
    access_token: &str,
    id_token: Option<&str>,
    refresh_token: Option<&str>,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> UsageFetchOutcome {
    let mut refreshed = None;
    let result = fetch_usage_capturing_refresh(
        alias,
        access_token,
        id_token,
        refresh_token,
        account_id,
        is_fedramp,
        &mut refreshed,
    )
    .await;
    UsageFetchOutcome { refreshed, result }
}

/// Inner body of [`fetch_usage_with_refresh`]. Every successful refresh is
/// written into `refreshed` *before* any further fallible step, so `?`/`bail!`
/// can never discard a rotated token.
#[allow(clippy::too_many_arguments)]
async fn fetch_usage_capturing_refresh(
    alias: &str,
    access_token: &str,
    id_token: Option<&str>,
    refresh_token: Option<&str>,
    account_id: Option<&str>,
    is_fedramp: bool,
    refreshed: &mut Option<RefreshedTokens>,
) -> Result<UsageInfo> {
    let client = auth::build_http_client()?;
    let usage_url = usage_url();
    let mut rejected_refresh: Option<anyhow::Error> = None;

    // Refresh when either JWT is near expiry so account identity metadata does
    // not remain stale while the access token is still usable.
    if let Some(rt) = refresh_token
        && token_needs_refresh(access_token, id_token, 60)
    {
        info!("[{alias}] token expiring soon, proactively refreshing");

        match do_refresh_token(alias, &client, id_token, Some(access_token), rt).await {
            Ok(new_tokens) => {
                let bearer = new_tokens.access_token.clone();
                *refreshed = Some(new_tokens);

                let resp = apply_account_routing_headers(
                    client
                        .get(&usage_url)
                        .header("Authorization", format!("Bearer {bearer}")),
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
                    enrich_reset_credits(alias, &client, &bearer, account_id, &mut usage).await;
                    return Ok(usage);
                }
                anyhow::bail!("Usage API failed (HTTP {status}) after proactive token refresh");
            }
            Err(e) => {
                if e.downcast_ref::<TerminalAuthError>().is_some() {
                    info!("[{alias}] proactive token refresh rejected permanently: {e:#}");
                    rejected_refresh = Some(e);
                } else {
                    info!(
                        "[{alias}] proactive token refresh failed, trying with existing token: {e:#}"
                    );
                }
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
        return Ok(usage);
    }

    // The auth server already rejected this refresh token moments ago; asking
    // again can only re-trigger reuse detection and add a round trip.
    if let Some(e) = rejected_refresh {
        return Err(e.context(format!("Usage API failed (HTTP {status})")));
    }

    // If 401/403 and we have a refresh_token, try to refresh
    if let Some(rt) = refresh_token
        && (status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN)
    {
        info!("[{alias}] got HTTP {status}, attempting token refresh");

        match do_refresh_token(alias, &client, id_token, Some(access_token), rt).await {
            Ok(new_tokens) => {
                let bearer = new_tokens.access_token.clone();
                *refreshed = Some(new_tokens);

                let resp2 = apply_account_routing_headers(
                    client
                        .get(&usage_url)
                        .header("Authorization", format!("Bearer {bearer}")),
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
                    enrich_reset_credits(alias, &client, &bearer, account_id, &mut usage).await;
                    return Ok(usage);
                }
                anyhow::bail!("Usage API still failed (HTTP {status2}) after token refresh");
            }
            Err(e) => {
                info!("[{alias}] token refresh failed: {e:#}");
                // `.context` (not `bail!`) so the typed terminal-auth error
                // stays downcastable by the retry loop.
                return Err(e.context(format!(
                    "Usage API failed (HTTP {status}), token refresh also failed"
                )));
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
            let outcome = fetch_usage_with_refresh(
                alias,
                &at,
                id_token.as_deref(),
                rt.as_deref(),
                account_id.as_deref(),
                is_fedramp,
            )
            .await;
            let refreshed = outcome.refreshed;
            // Apply before propagating: a rotated refresh_token is single-use,
            // so dropping it on the error path would brick the imported auth.
            if let Some(tokens) = &refreshed {
                auth::apply_tokens(
                    val,
                    &tokens.id_token,
                    &tokens.access_token,
                    &tokens.refresh_token,
                )?;
            }
            let usage = outcome.result?;
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
            let outcome = fetch_usage_with_refresh(
                alias,
                &refreshed.access_token,
                Some(&refreshed.id_token),
                Some(&refreshed.refresh_token),
                account_id.as_deref(),
                is_fedramp,
            )
            .await;
            let refreshed_again = outcome.refreshed;
            if let Some(tokens) = &refreshed_again {
                auth::apply_tokens(
                    val,
                    &tokens.id_token,
                    &tokens.access_token,
                    &tokens.refresh_token,
                )?;
            }
            let usage = outcome.result?;
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

    let refreshed = resolve_refreshed_tokens(
        r,
        status,
        current_id_token,
        current_access_token,
        refresh_token,
    )
    .with_context(|| format!("[{alias}] token refresh HTTP {status}"))?;
    info!("[{alias}] token refresh succeeded");
    Ok(refreshed)
}

/// Max number of tokens to refresh opportunistically per CLI invocation.
const OPPORTUNISTIC_REFRESH_LIMIT: usize = 3;
/// Refresh tokens expiring within this many seconds.
const OPPORTUNISTIC_REFRESH_MARGIN: i64 = 1800; // 30 minutes
/// Total wall-clock timeout for all opportunistic refreshes (concurrent).
const OPPORTUNISTIC_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

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
        let expiry = [
            crate::jwt::token_expires_at(&at),
            id_token.as_deref().and_then(crate::jwt::token_expires_at),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(exp) = expiry else {
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
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;

    fn jwt_with_exp(exp: i64) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::json!({"exp": exp}).to_string());
        format!("header.{payload}.signature")
    }

    #[test]
    fn expired_id_token_triggers_refresh_before_access_token_expires() {
        let now = crate::auth::now_unix_secs();
        let access = jwt_with_exp(now + 86_400);
        let id = jwt_with_exp(now - 60);

        assert!(token_needs_refresh(&access, Some(&id), 60));
    }

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
                error_description: None,
            },
            reqwest::StatusCode::OK,
            Some("existing-id"),
            Some("existing-access"),
            "existing-refresh",
        )
        .unwrap();

        assert_eq!(refreshed.id_token, "existing-id");
        assert_eq!(refreshed.access_token, "new-access");
        assert_eq!(refreshed.refresh_token, "existing-refresh");
    }
}
