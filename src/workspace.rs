use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::RequestBuilder;
use serde::Deserialize;

const ACCOUNTS_CHECK_URL: &str = "https://chatgpt.com/backend-api/wham/accounts/check";

#[derive(Debug, Deserialize)]
struct AccountEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    structure: String,
}

#[derive(Debug, Deserialize)]
struct ChatGptAccountEntry {
    account: ChatGptAccountInfo,
}

#[derive(Debug, Deserialize)]
struct ChatGptAccountInfo {
    account_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    structure: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAccounts {
    List(Vec<AccountEntry>),
    Map(HashMap<String, ChatGptAccountEntry>),
}

impl Default for RawAccounts {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

#[derive(Debug, Deserialize)]
struct AccountsCheckResponse {
    #[serde(default)]
    accounts: RawAccounts,
    #[serde(default)]
    account_ordering: Vec<String>,
}

impl AccountsCheckResponse {
    fn into_accounts(self) -> Vec<AccountEntry> {
        match self.accounts {
            RawAccounts::List(accounts) => accounts,
            RawAccounts::Map(mut accounts) => self
                .account_ordering
                .iter()
                .filter_map(|id| {
                    let account = accounts.remove(id)?.account;
                    Some(AccountEntry {
                        id: account.account_id?,
                        name: account.name,
                        structure: account.structure,
                    })
                })
                .collect(),
        }
    }

    fn workspace_name_for(self, account_id: &str) -> Option<String> {
        self.into_accounts()
            .into_iter()
            .find(|account| account.id == account_id)
            .and_then(|account| {
                let _structure = account.structure;
                account.name
            })
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
    }
}

fn accounts_check_url() -> String {
    std::env::var("CS_ACCOUNTS_CHECK_URL").unwrap_or_else(|_| ACCOUNTS_CHECK_URL.to_string())
}

fn build_accounts_check_request(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
    account_id: &str,
    is_fedramp: bool,
) -> RequestBuilder {
    let mut request = client
        .get(url)
        .timeout(Duration::from_secs(5))
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("ChatGPT-Account-ID", account_id);
    if is_fedramp {
        request = request.header("X-OpenAI-Fedramp", "true");
    }
    request
}

pub(crate) async fn fetch_workspace_name(
    client: &reqwest::Client,
    access_token: &str,
    account_id: &str,
    is_fedramp: bool,
) -> Result<Option<String>> {
    if account_id.trim().is_empty() {
        return Ok(None);
    }
    let url = accounts_check_url();
    let response = build_accounts_check_request(client, &url, access_token, account_id, is_fedramp)
        .send()
        .await
        .with_context(|| "requesting ChatGPT workspace metadata")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("workspace metadata request failed (HTTP {status})");
    }
    let body = response
        .json::<AccountsCheckResponse>()
        .await
        .with_context(|| format!("parsing workspace metadata response (HTTP {status})"))?;
    Ok(body.workspace_name_for(account_id))
}

pub(crate) async fn refresh_for_auth(auth: &serde_json::Value) -> Result<Option<String>> {
    refresh_for_auth_if_needed(auth, true).await
}

pub(crate) async fn refresh_for_auth_if_needed(
    auth: &serde_json::Value,
    force: bool,
) -> Result<Option<String>> {
    let info = crate::jwt::parse_account_info(auth);
    let Some(account_id) = info.account_id.as_deref() else {
        return Ok(None);
    };
    if !force && let Some(name) = crate::cache::get_workspace_name(account_id) {
        return Ok(Some(name));
    }
    let Some(access_token) = auth
        .pointer("/tokens/access_token")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let client = crate::auth::build_http_client()?;
    remember_workspace_name(&client, access_token, Some(account_id), info.is_fedramp).await
}

pub(crate) async fn remember_workspace_name(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    is_fedramp: bool,
) -> Result<Option<String>> {
    let Some(account_id) = account_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let name = fetch_workspace_name(client, access_token, account_id, is_fedramp).await?;
    let cache_account_id = account_id.to_string();
    let cache_name = name.clone();
    tokio::task::spawn_blocking(move || {
        crate::cache::set_workspace_name(&cache_account_id, cache_name.as_deref())
    })
    .await
    .context("joining workspace cache update")??;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_api_list_shape() {
        let response: AccountsCheckResponse = serde_json::from_value(serde_json::json!({
            "accounts": [
                {"id": "acct-personal", "name": "Personal", "structure": "personal"},
                {"id": "acct-team", "name": "Platform Team", "structure": "workspace"}
            ],
            "account_ordering": ["acct-personal", "acct-team"]
        }))
        .unwrap();
        let accounts = response.into_accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1].id, "acct-team");
        assert_eq!(accounts[1].name.as_deref(), Some("Platform Team"));
    }

    #[test]
    fn parses_chatgpt_map_shape_in_server_order() {
        let response: AccountsCheckResponse = serde_json::from_value(serde_json::json!({
            "accounts": {
                "personal": {"account": {"account_id": "acct-personal", "name": "Personal", "structure": "personal"}},
                "team": {"account": {"account_id": "acct-team", "name": "Platform Team", "structure": "workspace"}}
            },
            "account_ordering": ["team", "personal"]
        }))
        .unwrap();
        let accounts = response.into_accounts();
        assert_eq!(accounts[0].id, "acct-team");
        assert_eq!(accounts[1].id, "acct-personal");
    }

    #[test]
    fn workspace_name_matches_selected_account_and_trims_name() {
        let response: AccountsCheckResponse = serde_json::from_value(serde_json::json!({
            "accounts": [
                {"id": "acct-personal", "name": "Personal", "structure": "personal"},
                {"id": "acct-team", "name": "  Platform Team  ", "structure": "workspace"}
            ]
        }))
        .unwrap();

        assert_eq!(
            response.workspace_name_for("acct-team").as_deref(),
            Some("Platform Team")
        );
    }

    #[test]
    fn request_matches_codex_headers() {
        let request = build_accounts_check_request(
            &reqwest::Client::new(),
            "https://chatgpt.com/backend-api/wham/accounts/check",
            "secret-token",
            "acct-team",
            true,
        )
        .build()
        .unwrap();
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer secret-token"
        );
        assert_eq!(request.headers()["ChatGPT-Account-ID"], "acct-team");
        assert_eq!(request.headers()["X-OpenAI-Fedramp"], "true");
    }
}
