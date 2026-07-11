use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::error::CsError;

const MAX_BACKUPS: usize = 3;

pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Upstream Codex version this release is contract-aligned with.
pub(crate) const ALIGNED_CODEX_VERSION: &str = "0.144.1";

/// User-Agent in the upstream shape: `codex_cli_rs/<version> (<os>; <arch>)`.
pub(crate) fn codex_user_agent() -> String {
    format!(
        "codex_cli_rs/{ALIGNED_CODEX_VERSION} ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
pub(crate) const ISSUER: &str = "https://auth.openai.com";
const DEFAULT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

pub(crate) fn token_url() -> String {
    std::env::var("CS_TOKEN_URL").unwrap_or_else(|_| DEFAULT_TOKEN_URL.to_string())
}

/// ~/.codex/auth.json (or $CODEX_HOME/auth.json)
pub fn codex_auth_path() -> Result<PathBuf> {
    let codex_home = codex_home_from_values(std::env::var_os("CODEX_HOME"), dirs::home_dir())?;
    validate_cli_auth_credentials_store(&codex_home)?;
    Ok(codex_home.join("auth.json"))
}

pub(crate) fn ensure_file_credentials_store() -> Result<()> {
    let codex_home = codex_home_from_values(std::env::var_os("CODEX_HOME"), dirs::home_dir())?;
    validate_cli_auth_credentials_store(&codex_home)
}

fn codex_home_from_values(
    configured_home: Option<OsString>,
    user_home: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(home) = configured_home.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(&home);
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            anyhow::bail!(
                "CODEX_HOME contains '..' component which is not allowed: {}",
                path.display()
            );
        }
        return Ok(path);
    }

    let home = user_home.ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    Ok(home.join(".codex"))
}

fn validate_cli_auth_credentials_store(codex_home: &Path) -> Result<()> {
    let Some((config_path, config)) = load_codex_config(codex_home)? else {
        return Ok(());
    };

    match config.get("cli_auth_credentials_store") {
        None => {}
        Some(toml::Value::String(mode)) if mode == "file" => {}
        Some(_) => anyhow::bail!(
            "codex-switch requires file-based Codex credentials; set \
             cli_auth_credentials_store = \"file\" in {}",
            config_path.display()
        ),
    }

    if config.get("forced_login_method").and_then(|v| v.as_str()) == Some("api") {
        anyhow::bail!(
            "Codex managed policy requires API key login, but codex-switch requires ChatGPT OAuth"
        );
    }
    Ok(())
}

fn load_codex_config(codex_home: &Path) -> Result<Option<(PathBuf, toml::Value)>> {
    let config_path = codex_home.join("config.toml");
    if !config_path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let config =
        toml::from_str(&raw).with_context(|| format!("parsing {}", config_path.display()))?;
    Ok(Some((config_path, config)))
}

fn validate_managed_auth_config(config: &toml::Value, account_id: Option<&str>) -> Result<()> {
    if config.get("forced_login_method").and_then(|v| v.as_str()) == Some("api") {
        anyhow::bail!(
            "Codex managed policy requires API key login, but codex-switch requires ChatGPT OAuth"
        );
    }

    let workspace_ids = forced_chatgpt_workspace_ids(config)?;
    if workspace_ids.is_empty() {
        return Ok(());
    }

    let account_id = account_id.ok_or_else(|| {
        anyhow::anyhow!("login token has no workspace id required by Codex managed policy")
    })?;
    if !workspace_ids.iter().any(|id| id == account_id) {
        anyhow::bail!(
            "workspace {account_id} is not allowed by Codex forced_chatgpt_workspace_id policy"
        );
    }
    Ok(())
}

fn forced_chatgpt_workspace_ids(config: &toml::Value) -> Result<Vec<String>> {
    let workspace_ids: Vec<&str> = match config.get("forced_chatgpt_workspace_id") {
        None => Vec::new(),
        Some(toml::Value::String(id)) => vec![id.trim()],
        Some(toml::Value::Array(ids)) => ids
            .iter()
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    anyhow::anyhow!("forced_chatgpt_workspace_id must contain only strings")
                })
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .map(str::trim)
            .collect(),
        Some(_) => {
            anyhow::bail!("forced_chatgpt_workspace_id must be a string or a list of strings")
        }
    };
    Ok(workspace_ids
        .into_iter()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect())
}

/// Workspace ids forced by Codex managed config — best-effort, empty when
/// unset or unreadable. Used to pre-restrict the OAuth authorize page the
/// same way Codex does via `allowed_workspace_id`.
pub(crate) fn configured_forced_workspace_ids() -> Vec<String> {
    let Ok(codex_home) = codex_home_from_values(std::env::var_os("CODEX_HOME"), dirs::home_dir())
    else {
        return Vec::new();
    };
    let Ok(Some((_path, config))) = load_codex_config(&codex_home) else {
        return Vec::new();
    };
    forced_chatgpt_workspace_ids(&config).unwrap_or_default()
}

pub(crate) fn validate_managed_chatgpt_account(id_token: &str) -> Result<()> {
    let codex_home = codex_home_from_values(std::env::var_os("CODEX_HOME"), dirs::home_dir())?;
    let Some((_config_path, config)) = load_codex_config(&codex_home)? else {
        return Ok(());
    };
    let auth = serde_json::json!({"tokens": {"id_token": id_token}});
    let account_id = crate::jwt::parse_account_info(&auth).account_id;
    validate_managed_auth_config(&config, account_id.as_deref())
}

/// ~/.codex-switch/
pub fn app_home() -> Result<PathBuf> {
    // Keep application state relocatable without changing Codex's own home.
    if let Some(path) = std::env::var_os("CODEX_SWITCH_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    Ok(home.join(".codex-switch"))
}

/// ~/.codex-switch/profiles/
pub fn profiles_dir() -> Result<PathBuf> {
    Ok(app_home()?.join("profiles"))
}

/// ~/.codex-switch/current
pub fn current_file() -> Result<PathBuf> {
    Ok(app_home()?.join("current"))
}

pub fn read_auth(path: &Path) -> Result<serde_json::Value> {
    if !path.exists() {
        return Err(CsError::NoAuthFile(path.display().to_string()).into());
    }
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let val: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    Ok(val)
}

pub(crate) fn atomic_write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating directory {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting permissions on {}", parent.display()))?;
    }

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    tmp.write_all(contents)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("syncing temporary file for {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("setting permissions on {}", tmp.path().display()))?;
    }
    tmp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("atomically replacing {}", path.display()))?;
    Ok(())
}

pub fn write_auth(path: &Path, val: &serde_json::Value) -> Result<()> {
    let raw = serde_json::to_string_pretty(val)?;
    atomic_write_private(path, raw.as_bytes())
}

/// Mask sensitive token/credential fields in a JSON body before logging.
/// Used by debug-level logs that may otherwise leak access/refresh/id tokens
/// when users share `--debug` output (e.g. in a bug report).
pub(crate) fn redact_sensitive_log_body(body: &serde_json::Value) -> String {
    const SENSITIVE_KEYS: &[&str] = &[
        "authorization_code",
        "code_verifier",
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
    ];

    fn redact(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(obj) => {
                for key in SENSITIVE_KEYS {
                    if obj.contains_key(*key) {
                        obj.insert((*key).to_string(), serde_json::json!("***"));
                    }
                }
                for (key, v) in obj.iter_mut() {
                    if !SENSITIVE_KEYS.contains(&key.as_str()) {
                        redact(v);
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    redact(v);
                }
            }
            _ => {}
        }
    }

    let mut value = body.clone();
    redact(&mut value);
    serde_json::to_string(&value).unwrap_or_default()
}

pub fn sha256_file(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let digest = Sha256::digest(&data);
    Some(hex::encode(digest))
}

pub fn backup_auth(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let ts = now_unix_secs();
    let bak = path.with_extension(format!("json.bak.{ts}"));
    std::fs::copy(path, &bak)
        .with_context(|| format!("backing up {} -> {}", path.display(), bak.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&bak, std::fs::Permissions::from_mode(0o600));
    }
    cleanup_old_backups(path);
    Ok(())
}

pub fn update_tokens(
    path: &Path,
    id_token: &str,
    access_token: &str,
    refresh_token: &str,
) -> Result<()> {
    let mut val = read_auth(path)?;
    apply_tokens(&mut val, id_token, access_token, refresh_token)
        .with_context(|| format!("updating tokens in {}", path.display()))?;
    write_auth(path, &val)
}

pub fn apply_tokens(
    val: &mut serde_json::Value,
    id_token: &str,
    access_token: &str,
    refresh_token: &str,
) -> Result<()> {
    let tokens = val
        .get_mut("tokens")
        .and_then(|t| t.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("auth.json missing tokens object"))?;

    tokens.insert("id_token".into(), serde_json::json!(id_token));
    tokens.insert("access_token".into(), serde_json::json!(access_token));
    tokens.insert("refresh_token".into(), serde_json::json!(refresh_token));
    // Codex refreshes proactively when last_refresh is older than 8 days;
    // stamping it here keeps our refreshes recognized (matches upstream).
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "last_refresh".into(),
            serde_json::json!(crate::output::format_iso8601(now_unix_secs())),
        );
    }
    Ok(())
}

/// Extract (access_token, refresh_token) from an auth.json Value.
pub fn extract_tokens(val: &serde_json::Value) -> (Option<String>, Option<String>) {
    let at = val
        .pointer("/tokens/access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let rt = val
        .pointer("/tokens/refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (at, rt)
}

pub fn extract_id_token(val: &serde_json::Value) -> Option<String> {
    val.pointer("/tokens/id_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Current unix timestamp in seconds.
pub fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read auth.json and parse AccountInfo in one step (returns default on error).
pub fn read_account_info(path: &Path) -> crate::jwt::AccountInfo {
    read_auth(path)
        .map(|v| {
            let mut info = crate::jwt::parse_account_info(&v);
            crate::cache::apply_workspace_name(&mut info);
            info
        })
        .unwrap_or_default()
}

pub fn validate_auth_value(val: &serde_json::Value) -> Result<crate::jwt::AccountInfo> {
    let tokens = val
        .get("tokens")
        .and_then(|t| t.as_object())
        .ok_or_else(|| anyhow::anyhow!("auth.json missing tokens object"))?;

    let id_token = tokens
        .get("id_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("tokens.id_token is required"))?;

    let has_access = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    let has_refresh = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty());

    if !has_access && !has_refresh {
        return Err(anyhow::anyhow!(
            "tokens.access_token or tokens.refresh_token is required"
        ));
    }

    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("tokens.id_token is not a valid JWT"))?;
    let decoded = {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| anyhow::anyhow!("tokens.id_token payload is not valid base64url"))?
    };
    let _: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|_| anyhow::anyhow!("tokens.id_token payload is not valid JSON"))?;

    let info = crate::jwt::parse_account_info(val);
    if info.email.is_none() && info.account_id.is_none() {
        return Err(anyhow::anyhow!(
            "id_token does not contain a usable email or account_id"
        ));
    }

    Ok(info)
}

/// Build a shared reqwest client with standard user-agent and proxy support.
pub fn build_http_client() -> Result<reqwest::Client> {
    let proxy_url = crate::config::resolve_proxy();
    build_http_client_with_proxy(proxy_url.as_deref())
}

pub fn build_http_client_with_proxy(proxy_url: Option<&str>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(codex_user_agent())
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(60));

    if let Some(url) = proxy_url {
        let sanitized_url = sanitize_proxy_url(url);
        tracing::debug!("Using proxy: {sanitized_url}");
        let mut proxy = reqwest::Proxy::all(url)
            .map_err(|e| anyhow::anyhow!("invalid proxy URL '{sanitized_url}': {e}"))?;
        if let Some(no_proxy) = crate::config::resolve_no_proxy() {
            tracing::debug!("No-proxy list: {no_proxy}");
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&no_proxy));
        }
        builder = builder.proxy(proxy);
    }

    if let Some(path) = custom_ca_path_from_values(
        std::env::var_os("CODEX_CA_CERTIFICATE"),
        std::env::var_os("SSL_CERT_FILE"),
    ) {
        let pem = std::fs::read(&path)
            .with_context(|| format!("reading custom CA bundle {}", path.display()))?;
        let certificates = reqwest::Certificate::from_pem_bundle(&pem)
            .with_context(|| format!("parsing custom CA bundle {}", path.display()))?;
        if certificates.is_empty() {
            anyhow::bail!(
                "custom CA bundle {} contains no certificates",
                path.display()
            );
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }

    Ok(builder.build()?)
}

fn custom_ca_path_from_values(
    codex_ca: Option<OsString>,
    ssl_cert_file: Option<OsString>,
) -> Option<PathBuf> {
    codex_ca
        .filter(|value| !value.is_empty())
        .or_else(|| ssl_cert_file.filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn sanitize_proxy_url(url: &str) -> String {
    let Some(scheme_sep) = url.find("://") else {
        return url.to_string();
    };
    let authority_start = scheme_sep + 3;
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map(|idx| authority_start + idx)
        .unwrap_or(url.len());
    let authority = &url[authority_start..authority_end];
    let Some(userinfo_end) = authority.rfind('@') else {
        return url.to_string();
    };
    let at_pos = authority_start + userinfo_end;

    let mut sanitized = String::with_capacity(url.len());
    sanitized.push_str(&url[..authority_start]);
    sanitized.push_str("***:***");
    sanitized.push_str(&url[at_pos..]);
    sanitized
}

/// Format a reqwest error with the full source chain for diagnostics.
pub fn format_reqwest_error(context: &str, err: &reqwest::Error) -> anyhow::Error {
    let mut msg = format!("{context}: {err}");
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        msg.push_str(&format!("\n  caused by: {cause}"));
        source = std::error::Error::source(cause);
    }
    anyhow::anyhow!("{msg}")
}

fn cleanup_old_backups(path: &Path) {
    let parent = match path.parent() {
        Some(p) => p,
        None => return,
    };
    let stem = match path.file_name().and_then(|f| f.to_str()) {
        Some(s) => s,
        None => return,
    };
    let prefix = format!("{stem}.bak.");

    let mut backups: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|name| name.starts_with(&prefix))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();

    if backups.len() <= MAX_BACKUPS {
        return;
    }

    backups.sort();
    let to_remove = backups.len() - MAX_BACKUPS;
    for old in &backups[..to_remove] {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_recent_rfc3339(value: &serde_json::Value) {
        let text = value.as_str().expect("last_refresh should be a string");
        let parsed = chrono::DateTime::parse_from_rfc3339(text).expect("RFC3339 last_refresh");
        let age = chrono::Utc::now().signed_duration_since(parsed);
        assert!(
            age.num_seconds().abs() < 60,
            "last_refresh not recent: {text}"
        );
    }

    #[test]
    fn test_apply_tokens_updates_last_refresh() {
        let mut val = json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "old-id",
                "access_token": "old-access",
                "refresh_token": "old-refresh",
                "account_id": "acct"
            },
            "last_refresh": "2020-01-01T00:00:00Z"
        });

        apply_tokens(&mut val, "new-id", "new-access", "new-refresh").unwrap();

        assert_eq!(val["tokens"]["access_token"], "new-access");
        assert_recent_rfc3339(&val["last_refresh"]);
    }

    #[test]
    fn test_update_tokens_updates_last_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_auth(
            &path,
            &json!({
                "tokens": { "id_token": "a", "access_token": "b", "refresh_token": "c" },
                "last_refresh": "2020-01-01T00:00:00Z"
            }),
        )
        .unwrap();

        update_tokens(&path, "new-id", "new-access", "new-refresh").unwrap();

        let val = read_auth(&path).unwrap();
        assert_eq!(val["tokens"]["refresh_token"], "new-refresh");
        assert_recent_rfc3339(&val["last_refresh"]);
    }

    #[test]
    fn test_user_agent_matches_upstream_shape() {
        let ua = codex_user_agent();
        assert!(
            ua.starts_with("codex_cli_rs/0.144.1 ("),
            "unexpected UA: {ua}"
        );
        assert!(ua.ends_with(')'));
    }

    #[test]
    fn test_sanitize_proxy_url_masks_userinfo() {
        let url = "http://user:pass@example.com:8080/path?q=1";

        assert_eq!(
            sanitize_proxy_url(url),
            "http://***:***@example.com:8080/path?q=1"
        );
    }

    #[test]
    fn test_sanitize_proxy_url_keeps_url_without_userinfo() {
        let url = "socks5://example.com:1080";

        assert_eq!(sanitize_proxy_url(url), url);
    }

    #[cfg(unix)]
    #[test]
    fn test_write_auth_sets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");

        write_auth(&path, &json!({ "tokens": {} })).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_backup_auth_sets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");

        write_auth(&path, &json!({ "tokens": {} })).unwrap();
        backup_auth(&path).unwrap();

        let backup = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .find(|candidate| candidate != &path)
            .expect("backup file should exist");

        let mode = std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn test_explicit_non_file_credentials_stores_are_rejected() {
        for mode in ["keyring", "auto", "ephemeral"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("config.toml"),
                format!("cli_auth_credentials_store = \"{mode}\"\n"),
            )
            .unwrap();

            let err = validate_cli_auth_credentials_store(dir.path()).unwrap_err();

            assert!(
                err.to_string()
                    .contains("cli_auth_credentials_store = \"file\"")
            );
        }
    }

    #[test]
    fn test_missing_credentials_store_defaults_to_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "model = \"gpt-5\"\n").unwrap();

        validate_cli_auth_credentials_store(dir.path()).unwrap();
    }

    #[test]
    fn test_explicit_file_credentials_store_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "cli_auth_credentials_store = \"file\"\n",
        )
        .unwrap();

        validate_cli_auth_credentials_store(dir.path()).unwrap();
    }

    #[test]
    fn test_empty_codex_home_falls_back_to_default_home() {
        let user_home = PathBuf::from("/test-user-home");

        let codex_home =
            codex_home_from_values(Some(std::ffi::OsString::from("")), Some(user_home.clone()))
                .unwrap();

        assert_eq!(codex_home, user_home.join(".codex"));
    }

    #[test]
    fn test_managed_auth_rejects_api_only_policy() {
        let config: toml::Value = toml::from_str("forced_login_method = \"api\"\n").unwrap();

        let err = validate_managed_auth_config(&config, Some("workspace-a")).unwrap_err();

        assert!(err.to_string().contains("requires API key login"));
    }

    #[test]
    fn test_managed_auth_enforces_workspace_list() {
        let config: toml::Value = toml::from_str(
            "forced_login_method = \"chatgpt\"\nforced_chatgpt_workspace_id = [\"workspace-a\", \"workspace-b\"]\n",
        )
        .unwrap();

        validate_managed_auth_config(&config, Some("workspace-b")).unwrap();
        let err = validate_managed_auth_config(&config, Some("workspace-c")).unwrap_err();

        assert!(err.to_string().contains("workspace-c"));
    }

    #[test]
    fn test_custom_ca_prefers_codex_ca_and_ignores_empty_values() {
        let selected = custom_ca_path_from_values(
            Some(OsString::from("/certs/codex.pem")),
            Some(OsString::from("/certs/ssl.pem")),
        );
        assert_eq!(selected, Some(PathBuf::from("/certs/codex.pem")));

        let fallback = custom_ca_path_from_values(
            Some(OsString::from("")),
            Some(OsString::from("/certs/ssl.pem")),
        );
        assert_eq!(fallback, Some(PathBuf::from("/certs/ssl.pem")));
    }

    #[test]
    fn test_redact_sensitive_log_body_masks_nested_keys() {
        let body = json!({
            "data": {
                "access_token": "secret",
                "items": [
                    { "refresh_token": "r" },
                    { "keep": "value" }
                ]
            },
            "access_token": "top",
            "keep_top": "value"
        });

        let redacted: serde_json::Value =
            serde_json::from_str(&redact_sensitive_log_body(&body)).unwrap();

        assert_eq!(redacted["access_token"], "***");
        assert_eq!(redacted["data"]["access_token"], "***");
        assert_eq!(redacted["data"]["items"][0]["refresh_token"], "***");
        assert_eq!(redacted["data"]["items"][1]["keep"], "value");
        assert_eq!(redacted["keep_top"], "value");
    }
}
