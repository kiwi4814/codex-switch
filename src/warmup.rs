use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, bail};
use tokio::sync::{Mutex, OnceCell};
use tracing::{debug, warn};

const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const FALLBACK_MODEL: &str = "gpt-5.3-codex";

static CODEX_VERSION: OnceCell<String> = OnceCell::const_new();
// tokio Mutex held across the await in resolve_model, ensuring only one fetch per process.
// Keyed by account (alias) so one account's resolved model never leaks into another's warmup.
static MODEL_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn model_cache_get(cache: &HashMap<String, String>, key: &str) -> Option<String> {
    cache.get(key).cloned()
}

fn model_cache_set(cache: &mut HashMap<String, String>, key: &str, model: String) {
    cache.insert(key.to_string(), model);
}

fn model_cache_invalidate(cache: &mut HashMap<String, String>, key: &str) {
    cache.remove(key);
}

/// Detects the local `codex` CLI version. Runs the subprocess probe on a
/// blocking thread pool so it never stalls a tokio worker thread.
async fn detect_codex_version() -> &'static str {
    CODEX_VERSION
        .get_or_init(|| async {
            tokio::task::spawn_blocking(|| {
                std::process::Command::new("codex")
                    .arg("--version")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .and_then(|s| parse_codex_version(&s))
                    .unwrap_or_else(|| crate::auth::ALIGNED_CODEX_VERSION.to_string())
            })
            .await
            .unwrap_or_else(|_| crate::auth::ALIGNED_CODEX_VERSION.to_string())
        })
        .await
}

/// Pick the version token out of `codex --version` output. Output shapes vary
/// (`codex-cli 0.144.1`, `codex-cli 0.1.0 (build abc)`), so take the first
/// dotted token that starts with a digit rather than the last token.
fn parse_codex_version(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.'))
        .map(|v| v.to_string())
}

fn build_models_request(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
    version: &str,
) -> reqwest::RequestBuilder {
    crate::usage::apply_account_routing_headers(
        client
            .get(MODELS_URL)
            .query(&[("client_version", version)])
            .bearer_auth(access_token),
        account_id,
        is_fedramp,
    )
}

/// One entry from the `/models` endpoint's `models[]` array.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelEntry {
    pub slug: String,
    pub display_name: Option<String>,
    pub visibility: Option<String>,
    pub priority: Option<i64>,
    pub supported_in_api: Option<bool>,
    pub context_window: Option<u64>,
}

/// Parse the `/models` endpoint's JSON body into a `Vec<ModelEntry>`. Entries
/// missing a `slug` are skipped; other fields are treated as optional
/// (defensively ignoring unknown fields per the upstream contract).
fn parse_models_body(body: &serde_json::Value) -> Result<Vec<ModelEntry>> {
    let models = body["models"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no models array in response"))?;

    Ok(models
        .iter()
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_string();
            Some(ModelEntry {
                slug,
                display_name: m["display_name"].as_str().map(String::from),
                visibility: m["visibility"].as_str().map(String::from),
                priority: m["priority"].as_i64(),
                supported_in_api: m["supported_in_api"].as_bool(),
                context_window: m["context_window"].as_u64(),
            })
        })
        .collect())
}

/// Sort models for display: ascending priority (lowest number first), unknown
/// priority sorts last. Does not filter hidden models — callers decide how to
/// present `visibility == "hide"` entries (e.g. dim them rather than drop them).
pub(crate) fn sorted_models_for_display(models: &[ModelEntry]) -> Vec<&ModelEntry> {
    let mut sorted: Vec<&ModelEntry> = models.iter().collect();
    sorted.sort_by_key(|m| m.priority.unwrap_or(i64::MAX));
    sorted
}

/// True when the model's `visibility` marks it hidden from normal listings.
pub(crate) fn is_hidden_model(m: &ModelEntry) -> bool {
    m.visibility.as_deref() == Some("hide")
}

/// Fetch and parse the full model list from the `/models` endpoint.
pub(crate) async fn fetch_models(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> Result<Vec<ModelEntry>> {
    let resp = build_models_request(
        client,
        access_token,
        account_id,
        is_fedramp,
        detect_codex_version().await,
    )
    .send()
    .await
    .map_err(|e| crate::auth::format_reqwest_error("models fetch failed", &e))?;

    if !resp.status().is_success() {
        bail!("models endpoint returned {}", resp.status());
    }

    let body: serde_json::Value = resp.json().await?;
    parse_models_body(&body)
}

async fn fetch_warmup_model(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> Result<String> {
    let models = fetch_models(client, access_token, account_id, is_fedramp).await?;

    let visible: Vec<&ModelEntry> = models
        .iter()
        .filter(|m| m.visibility.as_deref() != Some("hide"))
        .collect();

    if visible.is_empty() {
        bail!("no visible models available");
    }

    // Prefer mini (lightest), fall back to highest priority (lowest number)
    let selected = visible
        .iter()
        .find(|m| m.slug.contains("mini"))
        .or_else(|| {
            visible
                .iter()
                .min_by_key(|m| m.priority.unwrap_or(i64::MAX))
        })
        .map(|m| m.slug.as_str())
        .unwrap_or(FALLBACK_MODEL);

    debug!("warmup: model selected from API: {selected}");
    Ok(selected.to_string())
}

async fn resolve_model(
    cache_key: &str,
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> String {
    let mut guard = MODEL_CACHE.lock().await;
    if let Some(model) = model_cache_get(&guard, cache_key) {
        return model;
    }
    // Hold the lock across the fetch so concurrent callers wait here instead of
    // each issuing a redundant request.
    match fetch_warmup_model(client, access_token, account_id, is_fedramp).await {
        Ok(model) => {
            model_cache_set(&mut guard, cache_key, model.clone());
            model
        }
        Err(e) => {
            warn!("failed to fetch warmup model list, using fallback: {e}");
            FALLBACK_MODEL.to_string()
        }
    }
}

fn build_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "instructions": "You are a helpful assistant.",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "ping"}]
        }],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "stream": true,
        "store": false,
        "include": []
    })
}

fn make_request(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
    body: &serde_json::Value,
) -> reqwest::RequestBuilder {
    crate::usage::apply_account_routing_headers(
        client
            .post(RESPONSES_URL)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json"),
        account_id,
        is_fedramp,
    )
    .json(body)
}

/// Send a minimal completion request to trigger the quota window countdown for a profile.
///
/// The 5-hour and 7-day windows only start after the first real API call.
/// This sends the lightest valid request ("ping") and discards the response body,
/// which is enough for the server to stamp the window start time.
pub async fn warmup_account(alias: &str, profile_path: &Path) -> Result<()> {
    let val = crate::auth::read_auth(profile_path)
        .map_err(|e| anyhow::anyhow!("{alias}: cannot read auth: {e}"))?;

    let (at, rt) = crate::auth::extract_tokens(&val);
    let mut id_token = crate::auth::extract_id_token(&val);
    let mut access_token = at
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{alias}: no access_token in profile"))?;
    let mut refresh_token = rt.filter(|s| !s.is_empty());

    let info = crate::auth::read_account_info(profile_path);
    let account_id = info.account_id;
    let is_fedramp = info.is_fedramp;

    let client = crate::auth::build_http_client()?;

    // Pre-refresh: if token is about to expire, refresh proactively
    if let Some(ref rt) = refresh_token
        && crate::jwt::is_token_expiring(&access_token, 60) == Some(true)
    {
        debug!("[{alias}] access_token expiring soon, refreshing before warmup");
        match crate::usage::do_refresh_token(
            alias,
            &client,
            id_token.as_deref(),
            Some(&access_token),
            rt,
        )
        .await
        {
            Ok(refreshed) => {
                if let Err(e) = crate::profile::update_profile_tokens_and_live_if_current(
                    alias,
                    &refreshed.id_token,
                    &refreshed.access_token,
                    &refreshed.refresh_token,
                ) {
                    warn!("[{alias}] failed to atomically persist refreshed tokens: {e}");
                }
                access_token = refreshed.access_token;
                id_token = Some(refreshed.id_token);
                refresh_token = Some(refreshed.refresh_token);
            }
            Err(e) => warn!("[{alias}] pre-warmup token refresh failed: {e}"),
        }
    }

    let model = resolve_model(
        alias,
        &client,
        &access_token,
        account_id.as_deref(),
        is_fedramp,
    )
    .await;
    let body = build_body(&model);

    debug!("[{alias}] warmup POST → {RESPONSES_URL} (model={model})");

    let mut resp = make_request(
        &client,
        &access_token,
        account_id.as_deref(),
        is_fedramp,
        &body,
    )
    .send()
    .await
    .map_err(|e| crate::auth::format_reqwest_error("warmup request failed", &e))?;

    let status = resp.status();
    debug!("[{alias}] warmup status: {status}");

    match status.as_u16() {
        200 => {
            // Quota window is triggered server-side on request receipt.
            // Read one chunk to confirm streaming started, then drop.
            let _ = resp.chunk().await;
            Ok(())
        }
        400 => {
            let text = resp.text().await.unwrap_or_default();
            if text.contains("not supported") {
                // Model deprecated — clear cache, fetch fresh model list, retry once
                debug!(
                    "[{alias}] model {model:?} not supported, refreshing model cache and retrying"
                );
                model_cache_invalidate(&mut *MODEL_CACHE.lock().await, alias);
                let new_model = resolve_model(
                    alias,
                    &client,
                    &access_token,
                    account_id.as_deref(),
                    is_fedramp,
                )
                .await;
                let retry_body = build_body(&new_model);
                let mut retry_resp = make_request(
                    &client,
                    &access_token,
                    account_id.as_deref(),
                    is_fedramp,
                    &retry_body,
                )
                .send()
                .await
                .map_err(|e| crate::auth::format_reqwest_error("warmup retry failed", &e))?;
                let retry_status = retry_resp.status();
                if retry_status.is_success() {
                    let _ = retry_resp.chunk().await;
                    return Ok(());
                }
                let retry_text = retry_resp.text().await.unwrap_or_default();
                let snippet: String = retry_text.chars().take(160).collect();
                bail!("{alias}: HTTP {retry_status} after model refresh — {snippet}")
            }
            let snippet: String = text.chars().take(160).collect();
            bail!("{alias}: HTTP 400 — {snippet}")
        }
        401 | 403 => {
            // Retry once with refreshed token
            if let Some(ref rt) = refresh_token {
                debug!("[{alias}] got {status}, attempting token refresh and retry");
                match crate::usage::do_refresh_token(
                    alias,
                    &client,
                    id_token.as_deref(),
                    Some(&access_token),
                    rt,
                )
                .await
                {
                    Ok(refreshed) => {
                        if let Err(e) = crate::profile::update_profile_tokens_and_live_if_current(
                            alias,
                            &refreshed.id_token,
                            &refreshed.access_token,
                            &refreshed.refresh_token,
                        ) {
                            warn!("[{alias}] failed to atomically persist refreshed tokens: {e}");
                        }
                        let mut retry_resp = make_request(
                            &client,
                            &refreshed.access_token,
                            account_id.as_deref(),
                            is_fedramp,
                            &body,
                        )
                        .send()
                        .await
                        .map_err(|e| {
                            crate::auth::format_reqwest_error("warmup retry failed", &e)
                        })?;
                        let retry_status = retry_resp.status();
                        if retry_status.is_success() {
                            let _ = retry_resp.chunk().await;
                            return Ok(());
                        }
                        bail!("{alias}: HTTP {retry_status} after token refresh retry")
                    }
                    Err(e) => bail!("{alias}: authentication failed and token refresh failed: {e}"),
                }
            }
            bail!(
                "{alias}: authentication failed — token may be expired (run `codex-switch list` to refresh)"
            )
        }
        429 => bail!("{alias}: rate limited"),
        code => {
            let text = resp.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(160).collect();
            bail!("{alias}: HTTP {code} — {snippet}")
        }
    }
}

/// Fetch the full model list for a profile (for display, e.g. the TUI detail
/// panel). Unlike `warmup_account`, this never sends a warmup ping — it only
/// refreshes an expiring access token before calling the `/models` endpoint.
pub(crate) async fn fetch_models_for_profile(
    alias: &str,
    profile_path: &Path,
) -> Result<Vec<ModelEntry>> {
    let val = crate::auth::read_auth(profile_path)
        .map_err(|e| anyhow::anyhow!("{alias}: cannot read auth: {e}"))?;

    let (at, rt) = crate::auth::extract_tokens(&val);
    let id_token = crate::auth::extract_id_token(&val);
    let mut access_token = at
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{alias}: no access_token in profile"))?;
    let refresh_token = rt.filter(|s| !s.is_empty());

    let info = crate::auth::read_account_info(profile_path);
    let account_id = info.account_id;
    let is_fedramp = info.is_fedramp;

    let client = crate::auth::build_http_client()?;

    if let Some(ref rt) = refresh_token
        && crate::jwt::is_token_expiring(&access_token, 60) == Some(true)
        && let Ok(refreshed) = crate::usage::do_refresh_token(
            alias,
            &client,
            id_token.as_deref(),
            Some(&access_token),
            rt,
        )
        .await
    {
        if let Err(e) = crate::profile::update_profile_tokens_and_live_if_current(
            alias,
            &refreshed.id_token,
            &refreshed.access_token,
            &refreshed.refresh_token,
        ) {
            warn!("[{alias}] failed to persist refreshed tokens: {e}");
        }
        access_token = refreshed.access_token;
    }

    fetch_models(&client, &access_token, account_id.as_deref(), is_fedramp).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_cache_keys_are_isolated_per_account() {
        let mut cache: HashMap<String, String> = HashMap::new();
        model_cache_set(&mut cache, "account-a", "model-a".to_string());

        assert_eq!(
            model_cache_get(&cache, "account-a"),
            Some("model-a".to_string())
        );
        assert_eq!(model_cache_get(&cache, "account-b"), None);
    }

    #[test]
    fn test_model_cache_invalidation_only_affects_target_key() {
        let mut cache: HashMap<String, String> = HashMap::new();
        model_cache_set(&mut cache, "account-a", "model-a".to_string());
        model_cache_set(&mut cache, "account-b", "model-b".to_string());

        model_cache_invalidate(&mut cache, "account-a");

        assert_eq!(model_cache_get(&cache, "account-a"), None);
        assert_eq!(
            model_cache_get(&cache, "account-b"),
            Some("model-b".to_string())
        );
    }

    #[test]
    fn test_parse_models_body_full_entry() {
        let body = serde_json::json!({
            "models": [{
                "slug": "gpt-5.3-codex",
                "display_name": "GPT-5.3 Codex",
                "visibility": "List",
                "priority": 1,
                "supported_in_api": true,
                "context_window": 128000
            }]
        });

        let models = parse_models_body(&body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(
            models[0],
            ModelEntry {
                slug: "gpt-5.3-codex".to_string(),
                display_name: Some("GPT-5.3 Codex".to_string()),
                visibility: Some("List".to_string()),
                priority: Some(1),
                supported_in_api: Some(true),
                context_window: Some(128000),
            }
        );
    }

    #[test]
    fn test_parse_models_body_missing_optional_fields() {
        let body = serde_json::json!({
            "models": [{"slug": "gpt-5-mini"}]
        });

        let models = parse_models_body(&body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "gpt-5-mini");
        assert_eq!(models[0].display_name, None);
        assert_eq!(models[0].visibility, None);
        assert_eq!(models[0].priority, None);
        assert_eq!(models[0].supported_in_api, None);
        assert_eq!(models[0].context_window, None);
    }

    #[test]
    fn test_parse_models_body_empty_list() {
        let body = serde_json::json!({"models": []});
        let models = parse_models_body(&body).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_parse_models_body_missing_array_errors() {
        let body = serde_json::json!({});
        assert!(parse_models_body(&body).is_err());
    }

    #[test]
    fn test_sorted_models_for_display_orders_by_priority_ascending() {
        let models = vec![
            ModelEntry {
                slug: "b".to_string(),
                display_name: None,
                visibility: None,
                priority: Some(3),
                supported_in_api: None,
                context_window: None,
            },
            ModelEntry {
                slug: "a".to_string(),
                display_name: None,
                visibility: None,
                priority: Some(1),
                supported_in_api: None,
                context_window: None,
            },
            ModelEntry {
                slug: "c-no-priority".to_string(),
                display_name: None,
                visibility: None,
                priority: None,
                supported_in_api: None,
                context_window: None,
            },
        ];

        let sorted = sorted_models_for_display(&models);
        let slugs: Vec<&str> = sorted.iter().map(|m| m.slug.as_str()).collect();
        assert_eq!(slugs, vec!["a", "b", "c-no-priority"]);
    }

    #[test]
    fn test_sorted_models_for_display_empty_list() {
        assert!(sorted_models_for_display(&[]).is_empty());
    }

    #[test]
    fn test_is_hidden_model_detects_hide_visibility() {
        let hidden = ModelEntry {
            slug: "hidden".to_string(),
            display_name: None,
            visibility: Some("hide".to_string()),
            priority: None,
            supported_in_api: None,
            context_window: None,
        };
        let visible = ModelEntry {
            slug: "visible".to_string(),
            display_name: None,
            visibility: Some("List".to_string()),
            priority: None,
            supported_in_api: None,
            context_window: None,
        };
        assert!(is_hidden_model(&hidden));
        assert!(!is_hidden_model(&visible));
    }

    #[test]
    fn test_parse_codex_version_picks_semver_token() {
        assert_eq!(
            parse_codex_version("codex-cli 0.144.1\n"),
            Some("0.144.1".to_string())
        );
        assert_eq!(
            parse_codex_version("codex-cli 0.1.0 (build abc)\n"),
            Some("0.1.0".to_string())
        );
        assert_eq!(parse_codex_version("0.5.0\n"), Some("0.5.0".to_string()));
        assert_eq!(parse_codex_version("command not found\n"), None);
    }

    #[test]
    fn test_models_request_includes_workspace_and_fedramp_headers() {
        let request = build_models_request(
            &reqwest::Client::new(),
            "access-token",
            Some("workspace-123"),
            true,
            "0.144.1",
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
    fn test_responses_request_includes_workspace_and_fedramp_headers() {
        let request = make_request(
            &reqwest::Client::new(),
            "access-token",
            Some("workspace-123"),
            true,
            &build_body("gpt-test"),
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
}
