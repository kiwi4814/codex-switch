use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::usage::{ResetCredit, UsageInfo};

static CACHE_LOCK: Mutex<()> = Mutex::new(());
const CACHE_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const CACHE_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    ts: u64,
    primary_used: Option<f64>,
    primary_reset: Option<i64>,
    secondary_used: Option<f64>,
    secondary_reset: Option<i64>,
    #[serde(default)]
    credits_balance: Option<f64>,
    #[serde(default)]
    unlimited_credits: Option<bool>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    reset_credits_available_count: Option<u64>,
    #[serde(default)]
    reset_credits: Vec<ResetCredit>,
    #[serde(default)]
    reset_credits_error: Option<String>,
    #[serde(default)]
    account_limited: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    entries: HashMap<String, CacheEntry>,
    /// Tracks the last time each profile was selected by `use` (unix seconds).
    #[serde(default)]
    last_used: HashMap<String, i64>,
    /// Workspace display names keyed by the stable ChatGPT account id.
    #[serde(default)]
    workspace_names: HashMap<String, String>,
}

fn cache_path() -> Result<PathBuf> {
    Ok(auth::app_home()?.join("cache.json"))
}

fn cache_lock_path() -> Result<PathBuf> {
    Ok(auth::app_home()?.join("cache.lock"))
}

fn open_cache_lock_file(path: &std::path::Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating cache directory {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("setting permissions on {}", parent.display()))?;
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("opening cache lock {}", path.display()))
}

fn with_cache_file_lock_at<T>(
    path: &std::path::Path,
    timeout: Duration,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let file = open_cache_lock_file(path)?;
    let deadline = Instant::now() + timeout;
    loop {
        match FileExt::try_lock(&file) {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(CACHE_LOCK_POLL_INTERVAL);
            }
            Err(TryLockError::WouldBlock) => {
                anyhow::bail!(
                    "cache lock {} remained held for {:.3}s; refusing to replace the live lock file",
                    path.display(),
                    timeout.as_secs_f64()
                );
            }
            Err(TryLockError::Error(err)) => {
                return Err(anyhow::Error::from(err))
                    .with_context(|| format!("locking cache file {}", path.display()));
            }
        }
    }
    operation()
}

fn with_cache_lock<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let _process_lock = CACHE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("cache process lock poisoned"))?;
    with_cache_file_lock_at(&cache_lock_path()?, CACHE_LOCK_WAIT_TIMEOUT, operation)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn ttl() -> u64 {
    crate::config::get().cache.ttl
}

fn load_cache() -> CacheFile {
    let path = match cache_path() {
        Ok(p) => p,
        Err(_) => return CacheFile::default(),
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &CacheFile) -> Result<()> {
    let path = cache_path()?;
    save_cache_at(&path, cache)
}

fn save_cache_at(path: &std::path::Path, cache: &CacheFile) -> Result<()> {
    let json = serde_json::to_string(cache).context("serializing cache")?;
    auth::atomic_write_private(path, json.as_bytes())
        .with_context(|| format!("writing cache file {}", path.display()))
}

fn to_entry(u: &UsageInfo) -> CacheEntry {
    CacheEntry {
        ts: now_secs(),
        primary_used: u.primary.as_ref().and_then(|w| w.used_percent),
        primary_reset: u.primary.as_ref().and_then(|w| w.resets_at),
        secondary_used: u.secondary.as_ref().and_then(|w| w.used_percent),
        secondary_reset: u.secondary.as_ref().and_then(|w| w.resets_at),
        credits_balance: u.credits_balance,
        unlimited_credits: u.unlimited_credits,
        plan_type: u.plan_type.clone(),
        reset_credits_available_count: u.reset_credits_available_count,
        reset_credits: u.reset_credits.clone(),
        reset_credits_error: u.reset_credits_error.clone(),
        account_limited: u.account_limited,
    }
}

fn from_entry(e: &CacheEntry) -> UsageInfo {
    use crate::usage::WindowUsage;
    let primary = if e.primary_used.is_some() || e.primary_reset.is_some() {
        Some(WindowUsage {
            used_percent: e.primary_used,
            resets_at: e.primary_reset,
        })
    } else {
        None
    };
    let secondary = if e.secondary_used.is_some() || e.secondary_reset.is_some() {
        Some(WindowUsage {
            used_percent: e.secondary_used,
            resets_at: e.secondary_reset,
        })
    } else {
        None
    };
    UsageInfo {
        fetched_at: Some(e.ts as i64),
        primary,
        secondary,
        credits_balance: e.credits_balance,
        unlimited_credits: e.unlimited_credits,
        plan_type: e.plan_type.clone(),
        reset_credits_available_count: e.reset_credits_available_count,
        reset_credits: e.reset_credits.clone(),
        reset_credits_error: e.reset_credits_error.clone(),
        account_limited: e.account_limited,
        additional_limits: vec![],
    }
}

/// Get cached usage for an alias if within TTL.
pub fn get(alias: &str) -> Option<UsageInfo> {
    match with_cache_lock(|| {
        let cache = load_cache();
        let Some(entry) = cache.entries.get(alias) else {
            return Ok(None);
        };
        if now_secs().saturating_sub(entry.ts) > ttl() {
            return Ok(None);
        }
        Ok(Some(from_entry(entry)))
    }) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!("Failed to read cache for {alias}: {err}");
            None
        }
    }
}

/// Store usage result in cache.
pub fn put(alias: &str, usage: &UsageInfo) {
    if let Err(err) = with_cache_lock(|| {
        let mut cache = load_cache();
        cache.entries.insert(alias.to_string(), to_entry(usage));
        save_cache(&cache)
    }) {
        tracing::warn!("Failed to write cache: {err}");
    }
}

pub fn get_workspace_name(account_id: &str) -> Option<String> {
    match with_cache_lock(|| Ok(load_cache().workspace_names.get(account_id).cloned())) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!("Failed to read cached workspace name: {err}");
            None
        }
    }
}

pub fn set_workspace_name(account_id: &str, name: Option<&str>) -> Result<()> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return Ok(());
    }
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    with_cache_lock(|| {
        let mut cache = load_cache();
        let changed = update_workspace_name(&mut cache, account_id, name);
        if changed {
            save_cache(&cache)?;
        }
        Ok(())
    })
}

fn update_workspace_name(cache: &mut CacheFile, account_id: &str, name: Option<&str>) -> bool {
    match name {
        Some(name) if cache.workspace_names.get(account_id).map(String::as_str) != Some(name) => {
            cache
                .workspace_names
                .insert(account_id.to_string(), name.to_string());
            true
        }
        None => cache.workspace_names.remove(account_id).is_some(),
        Some(_) => false,
    }
}

pub fn apply_workspace_name(info: &mut crate::jwt::AccountInfo) {
    let Some(account_id) = info.account_id.as_deref() else {
        return;
    };
    if let Some(name) = get_workspace_name(account_id) {
        info.workspace_name = Some(name);
    }
}

/// Remove cached usage for an alias while preserving last-used metadata.
pub fn invalidate(alias: &str) -> Result<()> {
    with_cache_lock(|| {
        let mut cache = load_cache();
        if cache.entries.remove(alias).is_some() {
            save_cache(&cache).context("writing usage cache invalidation")?;
        }
        Ok(())
    })
}

/// Async wrapper around [`get`]: runs the blocking lock + file read on a
/// dedicated blocking thread so it never stalls a tokio worker. Use this on
/// the high-concurrency usage-fetch path (up to `network.max_concurrent`
/// tasks) instead of calling [`get`] directly inside an async task.
pub async fn get_async(alias: &str) -> Option<UsageInfo> {
    let alias = alias.to_string();
    tokio::task::spawn_blocking(move || get(&alias))
        .await
        .ok()
        .flatten()
}

/// Async wrapper around [`put`]; see [`get_async`] for rationale.
pub async fn put_async(alias: &str, usage: &UsageInfo) {
    let alias = alias.to_string();
    let usage = usage.clone();
    let _ = tokio::task::spawn_blocking(move || put(&alias, &usage)).await;
}

/// Get the last-used timestamp for an alias (0 if never used).
pub fn get_last_used(alias: &str) -> i64 {
    match with_cache_lock(|| Ok(load_cache().last_used.get(alias).copied().unwrap_or(0))) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!("Failed to read last-used cache for {alias}: {err}");
            0
        }
    }
}

/// Record that an alias was just selected by `use`.
pub fn set_last_used(alias: &str) -> Result<()> {
    with_cache_lock(|| {
        let mut cache = load_cache();
        cache
            .last_used
            .insert(alias.to_string(), crate::auth::now_unix_secs());
        save_cache(&cache).context("writing last_used cache")
    })
}

pub fn rename(old: &str, new: &str) -> Result<()> {
    with_cache_lock(|| {
        let mut cache = load_cache();
        // Migrate entries and last_used independently — either may exist without the other.
        let mut changed = false;
        if let Some(entry) = cache.entries.remove(old) {
            cache.entries.insert(new.to_string(), entry);
            changed = true;
        }
        if let Some(ts) = cache.last_used.remove(old) {
            cache.last_used.insert(new.to_string(), ts);
            changed = true;
        }
        if changed {
            save_cache(&cache)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs4::FileExt;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn test_cache_entry_deserialize_without_credits() {
        let entry: CacheEntry = serde_json::from_value(json!({
            "ts": 123,
            "primary_used": 25.0,
            "primary_reset": 456,
            "secondary_used": 75.0,
            "secondary_reset": 789
        }))
        .unwrap();

        assert_eq!(entry.credits_balance, None);
        assert_eq!(entry.unlimited_credits, None);
        assert_eq!(entry.reset_credits_available_count, None);
        assert!(entry.reset_credits.is_empty());
        assert_eq!(entry.reset_credits_error, None);
        assert!(!entry.account_limited);

        let usage = from_entry(&entry);
        assert_eq!(usage.credits_balance, None);
        assert_eq!(usage.unlimited_credits, None);
        assert_eq!(usage.reset_credits_available_count, None);
        assert!(usage.reset_credits.is_empty());
        assert!(!usage.account_limited);
    }

    #[test]
    fn test_cache_round_trip_preserves_account_limited() {
        let usage = UsageInfo {
            account_limited: true,
            ..Default::default()
        };

        let entry = to_entry(&usage);
        assert!(entry.account_limited);
        assert!(from_entry(&entry).account_limited);
    }

    #[test]
    fn authoritative_empty_workspace_name_clears_stale_cache() {
        let mut cache = CacheFile::default();
        assert!(update_workspace_name(
            &mut cache,
            "acct-team",
            Some("Old Team")
        ));
        assert!(update_workspace_name(&mut cache, "acct-team", None));
        assert!(!cache.workspace_names.contains_key("acct-team"));
    }

    #[test]
    fn cache_mutation_waits_for_cross_process_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("cache.lock");
        let holder = open_cache_lock_file(&lock_path).unwrap();
        FileExt::lock(&holder).unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker_path = lock_path.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx
                .send(with_cache_file_lock_at(
                    &worker_path,
                    Duration::from_secs(1),
                    || Ok(()),
                ))
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "cache mutation must wait for an independently-held OS lock"
        );

        drop(holder);
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn cache_atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut first = CacheFile::default();
        first.last_used.insert("alice".into(), 1);
        save_cache_at(&path, &first).unwrap();

        let mut second = CacheFile::default();
        second.last_used.insert("bob".into(), 2);
        save_cache_at(&path, &second).unwrap();

        let saved: CacheFile = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.last_used.get("bob"), Some(&2));
        assert!(!saved.last_used.contains_key("alice"));
    }

    #[test]
    fn cache_lock_timeout_preserves_live_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("cache.lock");
        let holder = open_cache_lock_file(&lock_path).unwrap();
        std::fs::write(&lock_path, "holder-marker").unwrap();
        FileExt::lock(&holder).unwrap();

        let err =
            with_cache_file_lock_at(&lock_path, Duration::from_millis(25), || Ok(())).unwrap_err();
        assert!(err.to_string().contains("cache lock"));
        let reopened = open_cache_lock_file(&lock_path).unwrap();
        assert!(matches!(
            FileExt::try_lock(&reopened),
            Err(fs4::TryLockError::WouldBlock)
        ));
        FileExt::unlock(&holder).unwrap();
        assert_eq!(
            std::fs::read_to_string(&lock_path).unwrap(),
            "holder-marker"
        );
    }
}
