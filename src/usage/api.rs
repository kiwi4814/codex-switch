use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::auth::{self, CLIENT_ID, format_reqwest_error};

use super::parse::parse_usage_checked;
use super::reset_credits::enrich_reset_credits;
use super::{MAX_RETRIES, RETRY_DELAY, RefreshedTokens, UsageError, UsageInfo};

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

fn usage_url() -> String {
    std::env::var("CS_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string())
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
    use serde_json::json;

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
}
