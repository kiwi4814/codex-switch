use std::fs::{File, OpenOptions};
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs4::{FileExt, TryLockError};

use crate::auth::{
    app_home, atomic_write_private, backup_auth, codex_auth_path, current_file, profiles_dir,
    read_auth, write_auth,
};
use crate::error::CsError;
use crate::jwt::parse_account_info;
use crate::output::{user_print, user_println};

const MAX_ALIAS_LEN: usize = 64;

pub fn profile_auth_path(alias: &str) -> Result<PathBuf> {
    Ok(profiles_dir()?.join(alias).join("auth.json"))
}

pub fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty() {
        anyhow::bail!("alias cannot be empty");
    }
    if alias == "." || alias == ".." {
        anyhow::bail!("alias cannot be '.' or '..'");
    }
    if alias.len() > MAX_ALIAS_LEN {
        anyhow::bail!("alias must be at most {MAX_ALIAS_LEN} characters");
    }
    if !alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        anyhow::bail!("alias may only contain ASCII letters, digits, '_', '-', '.'");
    }
    Ok(())
}

pub fn list_profiles() -> Result<Vec<String>> {
    let dir = profiles_dir()?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading profiles directory {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

pub fn read_current() -> String {
    current_file()
        .and_then(|p| std::fs::read_to_string(p).map_err(Into::into))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("creating directory {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting permissions on {}", path.display()))?;
    }
    Ok(())
}

fn ensure_profile_parent(path: &Path) -> Result<()> {
    ensure_private_dir(&app_home()?)?;
    ensure_private_dir(&profiles_dir()?)?;
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    Ok(())
}

fn deleted_profiles_dir() -> Result<PathBuf> {
    Ok(app_home()?.join("deleted-profiles"))
}

fn auth_lock_path() -> Result<PathBuf> {
    Ok(app_home()?.join("auth.lock"))
}

fn launch_lock_path() -> Result<PathBuf> {
    Ok(app_home()?.join("launch.lock"))
}

/// Maximum time to wait for an auth-related lock. A timeout is reported rather
/// than replacing the inode because an OS lock is the only reliable liveness signal.
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn lock_live_auth() -> Result<File> {
    let path = auth_lock_path()?;
    acquire_file_lock(&path, LOCK_WAIT_TIMEOUT, "auth")
}

/// Serialize the short launch staging window without holding the auth write
/// lock while Codex starts and reads the staged credentials.
pub fn lock_launch_session() -> Result<File> {
    let path = launch_lock_path()?;
    acquire_file_lock(&path, LOCK_WAIT_TIMEOUT, "launch session")
}

struct AuthTransaction {
    _launch: File,
    _auth: File,
}

fn lock_auth_transaction() -> Result<AuthTransaction> {
    lock_auth_transaction_after_launch(|| {})
}

fn lock_auth_transaction_after_launch(after_launch: impl FnOnce()) -> Result<AuthTransaction> {
    // Every writer uses this order. Launch holds the first lock across its
    // stage/start/restore window and only takes the auth lock for each write.
    let launch = lock_launch_session()?;
    after_launch();
    let auth = lock_live_auth()?;
    Ok(AuthTransaction {
        _launch: launch,
        _auth: auth,
    })
}

fn acquire_file_lock(path: &Path, timeout: Duration, label: &str) -> Result<File> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    let file = open_lock_file(path)?;

    let deadline = Instant::now() + timeout;
    loop {
        match FileExt::try_lock(&file) {
            Ok(()) => {
                write_lock_holder(&file);
                return Ok(file);
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    let holder =
                        read_lock_holder(path).unwrap_or_else(|| "unknown holder".to_string());
                    anyhow::bail!(
                        "{label} lock {} remained held for {:.3}s by {holder}; refusing to replace the live lock file",
                        path.display(),
                        timeout.as_secs_f64(),
                    );
                }
                std::thread::sleep(LOCK_POLL_INTERVAL);
            }
            Err(TryLockError::Error(e)) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("locking {}", path.display()));
            }
        }
    }
}

/// Open a stable lock inode. Permission/ownership errors are reported rather
/// than recovered by unlinking because another process may still hold it.
fn open_lock_file(path: &Path) -> Result<File> {
    try_open_lock_file(path).with_context(|| {
        format!(
            "opening auth lock {}; check the file and parent directory ownership",
            path.display()
        )
    })
}

fn try_open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
}

/// Best-effort: write `pid epoch_secs` to the lock file for diagnostics.
/// Failure is non-fatal — the OS-level flock is the source of truth.
fn write_lock_holder(file: &File) {
    use std::io::Seek;
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{pid} {ts}\n");
    let _ = file.set_len(0);
    let mut f = file;
    let _ = f.seek(std::io::SeekFrom::Start(0));
    let _ = f.write_all(line.as_bytes());
}

fn read_lock_holder(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_current(alias: &str) -> Result<()> {
    let path = current_file()?;
    atomic_write_private(&path, alias.as_bytes())
        .with_context(|| format!("writing current profile marker {}", path.display()))?;
    Ok(())
}

fn switch_live_auth(alias: &str) -> Result<()> {
    validate_alias(alias)?;
    let src = profile_auth_path(alias)?;
    if !src.exists() {
        return Err(CsError::NotFound(alias.to_string()).into());
    }

    let _transaction = lock_auth_transaction()?;
    let val = read_auth(&src)?;
    let dst = codex_auth_path()?;
    backup_auth(&dst)?;
    write_auth(&dst, &val)?;
    write_current(alias)?;
    Ok(())
}

/// Persist refreshed tokens and, if this alias is current, update the live
/// Codex credentials under the same cross-process transaction.
pub fn update_profile_tokens_and_live_if_current(
    alias: &str,
    id_token: &str,
    access_token: &str,
    refresh_token: &str,
) -> Result<()> {
    update_profile_tokens_and_live_if_current_after_launch(
        alias,
        id_token,
        access_token,
        refresh_token,
        || {},
    )
}

fn update_profile_tokens_and_live_if_current_after_launch(
    alias: &str,
    id_token: &str,
    access_token: &str,
    refresh_token: &str,
    after_launch: impl FnOnce(),
) -> Result<()> {
    validate_alias(alias)?;
    let profile_path = profile_auth_path(alias)?;
    let _transaction = lock_auth_transaction_after_launch(after_launch)?;
    crate::auth::update_tokens(&profile_path, id_token, access_token, refresh_token)
        .with_context(|| format!("updating refreshed tokens for profile {alias}"))?;
    if read_current() == alias {
        let live = codex_auth_path()?;
        crate::auth::update_tokens(&live, id_token, access_token, refresh_token)
            .with_context(|| format!("updating live auth for current profile {alias}"))?;
    }
    Ok(())
}

/// Replace a saved profile and its live copy, when current, as one serialized
/// transaction. Used by CLI/TUI re-login paths.
pub fn replace_profile_auth_and_live_if_current(
    alias: &str,
    val: &serde_json::Value,
) -> Result<()> {
    validate_alias(alias)?;
    let profile_path = profile_auth_path(alias)?;
    let _transaction = lock_auth_transaction()?;
    ensure_same_account_identity(alias, &read_auth(&profile_path)?, val)?;
    write_auth(&profile_path, val)?;
    if read_current() == alias {
        let live = codex_auth_path()?;
        backup_auth(&live)?;
        write_auth(&live, val)?;
    }
    Ok(())
}

pub fn find_matching_profile(auth_path: &Path) -> Option<String> {
    let hash = crate::auth::sha256_file(auth_path)?;
    let profiles = list_profiles().ok()?;
    profiles.into_iter().find(|alias| {
        profile_auth_path(alias)
            .ok()
            .and_then(|p| crate::auth::sha256_file(&p))
            .map(|h| h == hash)
            .unwrap_or(false)
    })
}

pub fn active_profile_from_live() -> Option<String> {
    let src = codex_auth_path().ok()?;
    if !src.exists() {
        return None;
    }

    if let Some(alias) = find_matching_profile(&src) {
        return Some(alias);
    }

    let val = read_auth(&src).ok()?;
    let identity = extract_identity(&val);
    find_profile_by_identity_exact(&identity)
}

pub fn sync_current_from_live() -> Option<String> {
    let _transaction = lock_auth_transaction().ok()?;
    let alias = active_profile_from_live()?;
    if read_current() != alias
        && let Err(e) = write_current(&alias)
    {
        tracing::debug!("sync_current_from_live: could not sync current pointer: {e}");
    }
    Some(alias)
}

// ── Deduplication ─────────────────────────────────────────

#[derive(Debug)]
pub struct AccountIdentity {
    pub account_id: Option<String>,
    pub email: Option<String>,
}

pub fn extract_identity(auth: &serde_json::Value) -> AccountIdentity {
    let info = parse_account_info(auth);
    AccountIdentity {
        account_id: info.account_id,
        email: info.email.map(|e| e.to_lowercase()),
    }
}

fn ensure_same_account_identity(
    alias: &str,
    existing: &serde_json::Value,
    incoming: &serde_json::Value,
) -> Result<()> {
    let existing = extract_identity(existing);
    let incoming = extract_identity(incoming);
    let email_matches = matches!(
        (&existing.email, &incoming.email),
        (Some(existing), Some(incoming)) if existing == incoming
    );
    let account_matches = match (&existing.account_id, &incoming.account_id) {
        (Some(existing), Some(incoming)) => existing == incoming,
        _ => true,
    };
    if email_matches && account_matches {
        return Ok(());
    }
    anyhow::bail!("authenticated account does not match profile '{alias}'")
}

/// Find a profile with a strict match: both account_id AND email must be present and equal.
/// Used by `auto_track_current` to avoid silently syncing on ambiguous email-only matches.
pub fn find_profile_by_identity_exact(identity: &AccountIdentity) -> Option<String> {
    let (Some(target_id), Some(target_email)) = (&identity.account_id, &identity.email) else {
        return None; // identity itself is incomplete — no exact match possible
    };
    let profiles = list_profiles().ok()?;
    for alias in profiles {
        let path = match profile_auth_path(&alias) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let val = match read_auth(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let existing = extract_identity(&val);
        if let (Some(eid), Some(eemail)) = (&existing.account_id, &existing.email)
            && eid == target_id
            && eemail == target_email
        {
            return Some(alias);
        }
    }
    None
}

/// Find an existing profile matching the given identity (account_id+email > email-only).
pub fn find_profile_by_identity(identity: &AccountIdentity) -> Option<String> {
    let profiles = list_profiles().ok()?;
    let mut email_match: Option<String> = None;

    for alias in profiles {
        let path = match profile_auth_path(&alias) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let val = match read_auth(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let existing = extract_identity(&val);

        // Match: account_id AND email both equal (same person, same workspace)
        if let (Some(a1), Some(a2)) = (&identity.account_id, &existing.account_id)
            && a1 == a2
            && let (Some(e1), Some(e2)) = (&identity.email, &existing.email)
            && e1 == e2
        {
            return Some(alias);
        }

        // Fallback: email-only match (when account_id is missing on either side)
        if email_match.is_none()
            && let (Some(a), Some(b)) = (&identity.email, &existing.email)
            && a == b
            && (identity.account_id.is_none() || existing.account_id.is_none())
        {
            email_match = Some(alias);
        }
    }

    email_match
}

pub fn alias_from_email(email: &str) -> String {
    let base = email.split('@').next().unwrap_or(email);
    let alias = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(MAX_ALIAS_LEN)
        .collect::<String>();
    if alias.is_empty() {
        "account".to_string()
    } else {
        alias
    }
}

// ── Return types ──────────────────────────────────────────

pub enum SaveAction {
    Created(String),
    Updated(String),
}

impl SaveAction {
    pub fn alias(&self) -> &str {
        match self {
            SaveAction::Created(alias) | SaveAction::Updated(alias) => alias,
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            SaveAction::Created(_) => "created",
            SaveAction::Updated(_) => "updated",
        }
    }
}

#[derive(Debug)]
pub struct ImportSuccess {
    pub source: PathBuf,
    pub alias: String,
    pub action: &'static str,
    pub account: crate::jwt::AccountInfo,
    pub usage: crate::usage::UsageInfo,
}

#[derive(Debug)]
pub struct ImportFailure {
    pub source: PathBuf,
    pub stage: &'static str,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub imported: Vec<ImportSuccess>,
    pub skipped: Vec<ImportFailure>,
}

// ── Startup auth change detection ─────────────────────────

#[derive(Debug)]
pub enum AuthChange {
    /// Live auth.json belongs to a completely new account.
    NewAccount,
    /// Live auth.json matches an existing profile's identity but tokens differ.
    TokensUpdated { alias: String },
    /// No actionable change.
    NoChange,
}

/// Compare live auth.json against all saved profiles.
/// - Exact SHA256 match → NoChange
/// - Identity match (email + account_id) but different content → TokensUpdated
/// - No identity match → NewAccount
pub fn detect_auth_change() -> AuthChange {
    let auth_path = match codex_auth_path() {
        Ok(p) => p,
        Err(_) => return AuthChange::NoChange,
    };
    if !auth_path.exists() {
        return AuthChange::NoChange;
    }
    let val = match read_auth(&auth_path) {
        Ok(v) => v,
        Err(_) => return AuthChange::NoChange,
    };

    // Exact file match — nothing changed
    if find_matching_profile(&auth_path).is_some() {
        return AuthChange::NoChange;
    }

    let identity = extract_identity(&val);
    if identity.email.is_none() && identity.account_id.is_none() {
        return AuthChange::NoChange;
    }

    match find_profile_by_identity(&identity) {
        Some(alias) => AuthChange::TokensUpdated { alias },
        None => AuthChange::NewAccount,
    }
}

/// Copy the live auth.json into an existing profile's directory and mark it current.
/// The profile is written in canonical format. The live file is also normalized
/// (best-effort) to ensure SHA256 consistency; failure to normalize live is non-fatal.
pub fn update_profile_from_live(alias: &str) -> Result<()> {
    validate_alias(alias)?;
    let _transaction = lock_auth_transaction()?;
    let src = codex_auth_path()?;
    let val = read_auth(&src)?;
    let dst = profile_auth_path(alias)?;
    ensure_same_account_identity(alias, &read_auth(&dst)?, &val)?;
    ensure_profile_parent(&dst)?;
    write_auth(&dst, &val)?;
    // Best-effort: normalize live file to match profile (same key ordering)
    if let Err(e) = write_auth(&src, &val) {
        tracing::debug!("Could not normalize live auth.json: {e}");
    }
    write_current(alias)?;
    Ok(())
}

// ── Auto-track ────────────────────────────────────────────

/// If the live auth.json belongs to an untracked account, auto-save it.
/// Returns true if a new profile was created.
pub fn auto_track_current() -> bool {
    let src = match codex_auth_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if !src.exists() {
        return false;
    }
    let val = match read_auth(&src) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let identity = extract_identity(&val);

    if find_profile_by_identity_exact(&identity).is_some() {
        // Exact match (account_id + email) — safe to sync the current pointer.
        let _ = sync_current_from_live();
        return false;
    }
    // Email-only matches are ambiguous (same email, different workspace) —
    // fall through to cmd_save which will prompt the user if interactive.
    if find_profile_by_identity(&identity).is_some() {
        return false;
    }

    if let Ok(SaveAction::Created(a)) = cmd_save(None) {
        user_println(&format!("Auto-saved current account as profile: {a}"));
        return true;
    }
    false
}

// ── Command implementations ───────────────────────────────

pub fn cmd_save(alias: Option<&str>) -> Result<SaveAction> {
    let src = codex_auth_path()?;
    if !src.exists() {
        return Err(CsError::NoAuthFile(src.display().to_string()).into());
    }

    let _transaction = lock_auth_transaction()?;
    let val = read_auth(&src)?;
    // Best-effort: normalize live file to canonical formatting for SHA256 consistency
    if let Err(e) = write_auth(&src, &val) {
        tracing::debug!("Could not normalize live auth.json: {e}");
    }
    let identity = extract_identity(&val);

    let existing = find_profile_by_identity(&identity);

    let resolved_alias = match alias {
        Some(a) => a.to_string(),
        None => {
            if let Some(ref existing_alias) = existing {
                let dst = profile_auth_path(existing_alias)?;
                ensure_profile_parent(&dst)?;
                write_auth(&dst, &val)?;
                write_current(existing_alias)?;
                user_println(&format!("Updated profile: {existing_alias}"));
                return Ok(SaveAction::Updated(existing_alias.clone()));
            }
            identity
                .email
                .as_deref()
                .map(alias_from_email)
                .unwrap_or_else(|| "account".to_string())
        }
    };

    if alias.is_some()
        && let Some(existing_alias) = existing
    {
        let dst = profile_auth_path(&existing_alias)?;
        ensure_profile_parent(&dst)?;
        write_auth(&dst, &val)?;
        write_current(&existing_alias)?;
        if existing_alias != resolved_alias {
            user_println(&format!(
                "Duplicate account detected -- updated existing profile: {existing_alias} (not creating {resolved_alias})"
            ));
        } else {
            user_println(&format!("Updated profile: {existing_alias}"));
        }
        return Ok(SaveAction::Updated(existing_alias));
    }

    // New profile
    validate_alias(&resolved_alias)?;
    let dst = profile_auth_path(&resolved_alias)?;
    if dst.exists() {
        let unique = make_unique_alias(&resolved_alias)?;
        validate_alias(&unique)?;
        let unique_path = profile_auth_path(&unique)?;
        ensure_profile_parent(&unique_path)?;
        write_auth(&unique_path, &val)?;
        write_current(&unique)?;
        user_println(&format!(
            "Saved profile: {unique} (alias '{resolved_alias}' already taken)"
        ));
        return Ok(SaveAction::Created(unique));
    }

    ensure_profile_parent(&dst)?;
    write_auth(&dst, &val)?;
    write_current(&resolved_alias)?;
    user_println(&format!("Saved profile: {resolved_alias}"));
    Ok(SaveAction::Created(resolved_alias))
}

fn make_unique_alias(base: &str) -> Result<String> {
    const MAX_RETRIES: u32 = 1000;
    let mut n: u32 = 2;
    loop {
        let suffix = format!("_{n}");
        let prefix_len = MAX_ALIAS_LEN.saturating_sub(suffix.len());
        let prefix = base.chars().take(prefix_len).collect::<String>();
        let candidate = format!("{prefix}{suffix}");
        if !profile_auth_path(&candidate)?.exists() {
            return Ok(candidate);
        }
        n += 1;
        if n > MAX_RETRIES {
            anyhow::bail!(
                "could not generate a unique alias for '{base}' after {MAX_RETRIES} attempts"
            );
        }
    }
}

pub fn cmd_use(alias: &str, allow_prompt: bool) -> Result<()> {
    validate_alias(alias)?;
    let src = profile_auth_path(alias)?;
    if !src.exists() {
        return Err(CsError::NotFound(alias.to_string()).into());
    }

    let dst = codex_auth_path()?;

    if dst.exists() && find_matching_profile(&dst).is_none() {
        if !allow_prompt {
            anyhow::bail!(
                "current auth.json is not tracked; interactive confirmation is required before overwriting it"
            );
        }
        user_print(
            "Current auth.json does not belong to any saved profile -- switching will overwrite it. Continue? [y/N] ",
        );
        io::stdout().flush()?;
        io::stderr().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            return Err(CsError::Aborted.into());
        }
    }

    switch_live_auth(alias)?;
    user_println(&format!("Switched to profile: {alias}"));
    Ok(())
}

pub fn switch_profile(alias: &str) -> Result<()> {
    switch_live_auth(alias)
}

/// Write a profile's auth.json to the live codex auth path WITHOUT updating
/// the current-profile marker.  Used by `launch` for temporary switching.
/// Caller MUST hold the lock from `lock_live_auth()`.
pub fn stage_profile_auth(alias: &str) -> Result<()> {
    validate_alias(alias)?;
    let src = profile_auth_path(alias)?;
    if !src.exists() {
        return Err(CsError::NotFound(alias.to_string()).into());
    }
    let val = read_auth(&src)?;
    let dst = codex_auth_path()?;
    write_auth(&dst, &val)?;
    Ok(())
}

pub fn cmd_delete(alias: &str) -> Result<()> {
    validate_alias(alias)?;
    let _transaction = lock_auth_transaction()?;
    let dir = profiles_dir()?.join(alias);
    if !dir.exists() {
        return Err(CsError::NotFound(alias.to_string()).into());
    }
    if read_current() == alias {
        return Err(CsError::ActiveProfileDelete(alias.to_string()).into());
    }
    let deleted_dir = deleted_profiles_dir()?;
    ensure_private_dir(&deleted_dir)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    let archived = deleted_dir.join(format!("{alias}.backup-{timestamp}"));
    std::fs::rename(&dir, &archived).with_context(|| {
        format!(
            "archiving profile directory {} to {}",
            dir.display(),
            archived.display()
        )
    })?;
    user_println(&format!(
        "Deleted profile: {alias} (recoverable from {})",
        archived.display()
    ));
    Ok(())
}

pub fn collect_import_files(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        return Err(CsError::NoAuthFile(path.display().to_string()).into());
    }

    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut files = vec![];
    collect_import_files_recursive(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_import_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if entry
            .file_type()
            .with_context(|| format!("reading file type of {}", path.display()))?
            .is_dir()
        {
            collect_import_files_recursive(&path, files)?;
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            files.push(path);
        }
    }
    Ok(())
}

pub fn save_imported_auth_value(
    val: serde_json::Value,
    hint_alias: Option<&str>,
) -> Result<SaveAction> {
    let _transaction = lock_auth_transaction()?;
    let identity = extract_identity(&val);

    if let Some(existing) = find_profile_by_identity(&identity) {
        let dst = profile_auth_path(&existing)?;
        ensure_profile_parent(&dst)?;
        write_auth(&dst, &val)?;
        return Ok(SaveAction::Updated(existing));
    }

    let alias = hint_alias
        .map(|s| s.to_string())
        .or_else(|| identity.email.as_deref().map(alias_from_email))
        .unwrap_or_else(|| "account".to_string());
    validate_alias(&alias)?;
    let alias = if profile_auth_path(&alias)?.exists() {
        make_unique_alias(&alias)?
    } else {
        alias
    };
    validate_alias(&alias)?;

    let dst = profile_auth_path(&alias)?;
    ensure_profile_parent(&dst)?;
    write_auth(&dst, &val)?;
    Ok(SaveAction::Created(alias))
}

pub fn rename_profile(old_alias: &str, new_alias: &str) -> Result<()> {
    validate_alias(old_alias)?;
    validate_alias(new_alias)?;
    let old_dir = profiles_dir()?.join(old_alias);
    if !old_dir.exists() {
        return Err(CsError::NotFound(old_alias.to_string()).into());
    }
    let new_dir = profiles_dir()?.join(new_alias);
    if new_dir.exists() {
        anyhow::bail!("profile '{new_alias}' already exists");
    }
    let _transaction = lock_auth_transaction()?;
    std::fs::rename(&old_dir, &new_dir).with_context(|| {
        format!(
            "renaming profile {} -> {}",
            old_dir.display(),
            new_dir.display()
        )
    })?;
    if let Err(err) = crate::cache::rename(old_alias, new_alias) {
        tracing::warn!("Failed to rename cache entry {old_alias} -> {new_alias}: {err}");
    }
    if read_current() == old_alias {
        write_current(new_alias)?;
    }
    user_println(&format!("Renamed profile: {old_alias} -> {new_alias}"));
    Ok(())
}

pub fn save_auth_value(val: serde_json::Value, hint_alias: Option<&str>) -> Result<SaveAction> {
    let _transaction = lock_auth_transaction()?;
    let identity = extract_identity(&val);

    if let Some(existing) = find_profile_by_identity(&identity) {
        let dst = profile_auth_path(&existing)?;
        ensure_profile_parent(&dst)?;
        write_auth(&dst, &val)?;
        write_current(&existing)?;
        return Ok(SaveAction::Updated(existing));
    }

    let alias = hint_alias
        .map(|s| s.to_string())
        .or_else(|| identity.email.as_deref().map(alias_from_email))
        .unwrap_or_else(|| "account".to_string());
    validate_alias(&alias)?;

    let alias = if profile_auth_path(&alias)?.exists() {
        make_unique_alias(&alias)?
    } else {
        alias
    };
    validate_alias(&alias)?;

    let auth_dst = codex_auth_path()?;
    write_auth(&auth_dst, &val)?;

    let profile_dst = profile_auth_path(&alias)?;
    ensure_profile_parent(&profile_dst)?;
    write_auth(&profile_dst, &val)?;
    write_current(&alias)?;
    Ok(SaveAction::Created(alias))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::MutexGuard;
    use std::time::Duration;

    use anyhow::Result;
    use fs4::FileExt;

    use super::{cmd_delete, cmd_use, rename_profile, switch_profile, validate_alias};

    struct TestEnv {
        _lock: MutexGuard<'static, ()>,
        _home: tempfile::TempDir,
        old_home: Option<OsString>,
        old_codex_home: Option<OsString>,
        old_app_home: Option<OsString>,
    }

    impl TestEnv {
        fn new() -> Self {
            let lock = super::TEST_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let home = tempfile::tempdir().unwrap();
            let codex_home = home.path().join(".codex");
            let app_home = home.path().join(".codex-switch");
            let old_home = std::env::var_os("HOME");
            let old_codex_home = std::env::var_os("CODEX_HOME");
            let old_app_home = std::env::var_os("CODEX_SWITCH_HOME");

            unsafe {
                std::env::set_var("HOME", home.path());
                std::env::set_var("CODEX_HOME", &codex_home);
                std::env::set_var("CODEX_SWITCH_HOME", &app_home);
            }

            Self {
                _lock: lock,
                _home: home,
                old_home,
                old_codex_home,
                old_app_home,
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                match &self.old_home {
                    Some(value) => std::env::set_var("HOME", value),
                    None => std::env::remove_var("HOME"),
                }
                match &self.old_codex_home {
                    Some(value) => std::env::set_var("CODEX_HOME", value),
                    None => std::env::remove_var("CODEX_HOME"),
                }
                match &self.old_app_home {
                    Some(value) => std::env::set_var("CODEX_SWITCH_HOME", value),
                    None => std::env::remove_var("CODEX_SWITCH_HOME"),
                }
            }
        }
    }

    fn assert_invalid_alias(result: Result<()>, expected_message: &str) {
        let err = result.unwrap_err();
        assert_eq!(err.to_string(), expected_message);
    }

    #[test]
    fn validate_alias_accepts_expected_values() {
        assert!(validate_alias("alpha-123_.beta").is_ok());
        assert!(validate_alias(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn validate_alias_rejects_reserved_or_empty_values() {
        assert!(validate_alias("").is_err());
        assert!(validate_alias(".").is_err());
        assert!(validate_alias("..").is_err());
    }

    #[test]
    fn validate_alias_rejects_separators_and_non_ascii() {
        assert!(validate_alias("../escape").is_err());
        assert!(validate_alias("with/slash").is_err());
        assert!(validate_alias("\u{4E2D}\u{6587}").is_err());
        assert!(validate_alias(&"a".repeat(65)).is_err());
    }

    #[test]
    fn profile_commands_reject_invalid_alias_inputs() {
        let _env = TestEnv::new();

        for alias in ["../escape", "with/slash"] {
            assert_invalid_alias(
                cmd_use(alias, true),
                "alias may only contain ASCII letters, digits, '_', '-', '.'",
            );
            assert_invalid_alias(
                switch_profile(alias),
                "alias may only contain ASCII letters, digits, '_', '-', '.'",
            );
            assert_invalid_alias(
                cmd_delete(alias),
                "alias may only contain ASCII letters, digits, '_', '-', '.'",
            );
            assert_invalid_alias(
                rename_profile(alias, "valid-alias"),
                "alias may only contain ASCII letters, digits, '_', '-', '.'",
            );
        }

        assert_invalid_alias(cmd_use("", true), "alias cannot be empty");
        assert_invalid_alias(switch_profile(""), "alias cannot be empty");
        assert_invalid_alias(cmd_delete(""), "alias cannot be empty");
        assert_invalid_alias(rename_profile("", "valid-alias"), "alias cannot be empty");
    }

    #[test]
    fn rename_profile_rejects_invalid_new_alias() {
        let _env = TestEnv::new();
        let old_dir = super::profiles_dir().unwrap().join("valid-alias");
        std::fs::create_dir_all(&old_dir).unwrap();

        for alias in ["../escape", "with/slash"] {
            assert_invalid_alias(
                rename_profile("valid-alias", alias),
                "alias may only contain ASCII letters, digits, '_', '-', '.'",
            );
        }

        assert_invalid_alias(rename_profile("valid-alias", ""), "alias cannot be empty");
    }

    #[test]
    fn switch_profile_waits_for_auth_lock() {
        let _env = TestEnv::new();

        let live = crate::auth::codex_auth_path().unwrap();
        let current =
            realistic_auth_json("current@example.com", "acct_current", "acc_old", "ref_old");
        crate::auth::write_auth(&live, &current).unwrap();

        let next = realistic_auth_json("next@example.com", "acct_next", "acc_new", "ref_new");
        let profile_path = super::profile_auth_path("next-profile").unwrap();
        super::ensure_profile_parent(&profile_path).unwrap();
        crate::auth::write_auth(&profile_path, &next).unwrap();

        let lock_path = super::auth_lock_path().unwrap();
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        FileExt::lock(&lock_file).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let ok = super::switch_profile("next-profile").is_ok();
            tx.send(ok).unwrap();
        });

        std::thread::sleep(Duration::from_millis(100));
        assert!(
            rx.try_recv().is_err(),
            "switch should block while auth lock is held"
        );
        assert_eq!(
            crate::auth::read_auth(&live)
                .unwrap()
                .pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("acc_old")
        );

        drop(lock_file);

        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        handle.join().unwrap();
        assert_eq!(
            crate::auth::read_auth(&live)
                .unwrap()
                .pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("acc_new")
        );
        assert_eq!(super::read_current(), "next-profile");
    }

    #[test]
    fn auth_lock_timeout_preserves_live_lock_inode() {
        let _env = TestEnv::new();
        let lock_path = super::auth_lock_path().unwrap();
        super::ensure_private_dir(lock_path.parent().unwrap()).unwrap();
        let holder = super::open_lock_file(&lock_path).unwrap();
        FileExt::lock(&holder).unwrap();
        super::write_lock_holder(&holder);

        let err =
            super::acquire_file_lock(&lock_path, Duration::from_millis(25), "auth").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("auth lock"), "{message}");
        assert!(
            message.contains(&lock_path.display().to_string()),
            "{message}"
        );

        let reopened = super::open_lock_file(&lock_path).unwrap();
        assert!(matches!(
            FileExt::try_lock(&reopened),
            Err(fs4::TryLockError::WouldBlock)
        ));
        FileExt::unlock(&holder).unwrap();
    }

    #[test]
    fn switch_profile_waits_for_launch_session_lease() {
        let _env = TestEnv::new();
        let next = realistic_auth_json("next@example.com", "acct_next", "acc_new", "ref_new");
        let profile_path = super::profile_auth_path("next-profile").unwrap();
        super::ensure_profile_parent(&profile_path).unwrap();
        crate::auth::write_auth(&profile_path, &next).unwrap();

        let lease = super::lock_launch_session().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx
                .send(super::switch_profile("next-profile").is_ok())
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "switch must wait while the launch session lease is held"
        );

        drop(lease);
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        handle.join().unwrap();
    }

    #[test]
    fn refreshed_profile_and_live_auth_update_are_one_transaction() {
        let _env = TestEnv::new();
        let alice = realistic_auth_json("alice@example.com", "acct_a", "a-old", "a-ref");
        let bob = realistic_auth_json("bob@example.com", "acct_b", "b-old", "b-ref");
        let alice_path = super::profile_auth_path("alice").unwrap();
        let bob_path = super::profile_auth_path("bob").unwrap();
        super::ensure_profile_parent(&alice_path).unwrap();
        super::ensure_profile_parent(&bob_path).unwrap();
        crate::auth::write_auth(&alice_path, &alice).unwrap();
        crate::auth::write_auth(&bob_path, &bob).unwrap();
        super::switch_profile("alice").unwrap();

        let auth_gate = super::lock_live_auth().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let updater = std::thread::spawn(move || {
            done_tx
                .send(
                    super::update_profile_tokens_and_live_if_current_after_launch(
                        "alice",
                        "a-id-new",
                        "a-new",
                        "a-ref-new",
                        || started_tx.send(()).unwrap(),
                    ),
                )
                .unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let switcher = std::thread::spawn(|| super::switch_profile("bob"));
        drop(auth_gate);
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        updater.join().unwrap();
        switcher.join().unwrap().unwrap();

        assert_eq!(super::read_current(), "bob");
        let live = crate::auth::read_auth(&crate::auth::codex_auth_path().unwrap()).unwrap();
        assert_eq!(
            live.pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("b-old")
        );
        let alice_updated = crate::auth::read_auth(&alice_path).unwrap();
        assert_eq!(
            alice_updated
                .pointer("/tokens/access_token")
                .and_then(|v| v.as_str()),
            Some("a-new")
        );
    }

    #[test]
    fn sync_current_from_live_matches_live_identity() {
        let _env = TestEnv::new();

        let alpha = realistic_auth_json("alpha@example.com", "acct_alpha", "acc_a", "ref_a");
        let alpha_path = super::profile_auth_path("alpha").unwrap();
        super::ensure_profile_parent(&alpha_path).unwrap();
        crate::auth::write_auth(&alpha_path, &alpha).unwrap();

        let beta = realistic_auth_json("beta@example.com", "acct_beta", "acc_b_old", "ref_b_old");
        let beta_path = super::profile_auth_path("beta").unwrap();
        super::ensure_profile_parent(&beta_path).unwrap();
        crate::auth::write_auth(&beta_path, &beta).unwrap();

        super::write_current("alpha").unwrap();
        let live = realistic_auth_json("beta@example.com", "acct_beta", "acc_b_new", "ref_b_new");
        crate::auth::write_auth(&crate::auth::codex_auth_path().unwrap(), &live).unwrap();

        assert_eq!(super::sync_current_from_live().as_deref(), Some("beta"));
        assert_eq!(super::read_current(), "beta");
    }

    // ── detect_auth_change tests ─────────────────────────────

    fn make_jwt(email: &str, account_id: &str) -> String {
        let claims = serde_json::json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_account_id": account_id,
                "chatgpt_user_id": format!("user_{account_id}"),
                "organizations": [],
            }
        });
        let json = serde_json::to_vec(&claims).unwrap();
        let encoded = {
            use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
            URL_SAFE_NO_PAD.encode(json)
        };
        format!("x.{encoded}.y")
    }

    /// Build a realistic auth.json matching the format produced by `login::build_auth_json`.
    fn realistic_auth_json(
        email: &str,
        account_id: &str,
        access_token: &str,
        refresh_token: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": make_jwt(email, account_id),
                "access_token": access_token,
                "refresh_token": refresh_token,
                "account_id": account_id,
            },
            "last_refresh": "2026-04-07T00:00:00Z"
        })
    }

    // ── Basic branch coverage ────────────────────────────────

    #[test]
    fn detect_no_auth_file_returns_no_change() {
        let _env = TestEnv::new();
        assert!(matches!(
            super::detect_auth_change(),
            super::AuthChange::NoChange
        ));
    }

    #[test]
    fn detect_corrupt_auth_file_returns_no_change() {
        let env = TestEnv::new();
        let live = crate::auth::codex_auth_path().unwrap();
        let parent = live.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::write(&live, "{invalid json!!!").unwrap();
        assert!(matches!(
            super::detect_auth_change(),
            super::AuthChange::NoChange
        ));
        drop(env);
    }

    #[test]
    fn detect_exact_match_returns_no_change() {
        let _env = TestEnv::new();
        let val = realistic_auth_json("test@example.com", "acct_1", "acc_a", "ref_a");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        super::cmd_save(Some("test-profile")).unwrap();
        assert!(matches!(
            super::detect_auth_change(),
            super::AuthChange::NoChange
        ));
    }

    #[test]
    fn detect_new_account_when_no_profiles_exist() {
        let _env = TestEnv::new();
        let val = realistic_auth_json("new@example.com", "acct_new", "acc_x", "ref_x");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        assert!(matches!(
            super::detect_auth_change(),
            super::AuthChange::NewAccount
        ));
    }

    #[test]
    fn detect_new_account_when_different_identity() {
        let _env = TestEnv::new();
        let alice = realistic_auth_json("alice@example.com", "acct_alice", "acc_1", "ref_1");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &alice).unwrap();
        super::cmd_save(Some("alice")).unwrap();
        // Different person
        let bob = realistic_auth_json("bob@example.com", "acct_bob", "acc_2", "ref_2");
        crate::auth::write_auth(&live, &bob).unwrap();
        assert!(matches!(
            super::detect_auth_change(),
            super::AuthChange::NewAccount
        ));
    }

    // ── Token update scenarios (real refresh patterns) ───────

    #[test]
    fn detect_tokens_updated_refresh_token_changed() {
        let _env = TestEnv::new();
        let val = realistic_auth_json("user@example.com", "acct_u", "acc_old", "ref_old");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        super::cmd_save(Some("user-profile")).unwrap();
        // Re-login: new refresh_token
        let updated = realistic_auth_json("user@example.com", "acct_u", "acc_old", "ref_new");
        crate::auth::write_auth(&live, &updated).unwrap();
        match super::detect_auth_change() {
            super::AuthChange::TokensUpdated { alias } => assert_eq!(alias, "user-profile"),
            other => panic!("expected TokensUpdated, got {other:?}"),
        }
    }

    #[test]
    fn detect_tokens_updated_only_access_token_changed() {
        let _env = TestEnv::new();
        // Simulates token refresh where only access_token rotates (refresh_token reused)
        let val = realistic_auth_json("user@example.com", "acct_u", "acc_old", "ref_same");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        super::cmd_save(Some("user-profile")).unwrap();
        let updated = realistic_auth_json("user@example.com", "acct_u", "acc_new", "ref_same");
        crate::auth::write_auth(&live, &updated).unwrap();
        match super::detect_auth_change() {
            super::AuthChange::TokensUpdated { alias } => assert_eq!(alias, "user-profile"),
            other => panic!("expected TokensUpdated, got {other:?}"),
        }
    }

    #[test]
    fn detect_tokens_updated_only_last_refresh_timestamp_changed() {
        let _env = TestEnv::new();
        // Simulates codex CLI updating only the last_refresh timestamp
        let val = realistic_auth_json("user@example.com", "acct_u", "acc_1", "ref_1");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        super::cmd_save(Some("ts-profile")).unwrap();
        // Same tokens, different timestamp
        let mut updated = realistic_auth_json("user@example.com", "acct_u", "acc_1", "ref_1");
        updated["last_refresh"] = serde_json::json!("2026-04-08T12:00:00Z");
        crate::auth::write_auth(&live, &updated).unwrap();
        match super::detect_auth_change() {
            super::AuthChange::TokensUpdated { alias } => assert_eq!(alias, "ts-profile"),
            other => panic!("expected TokensUpdated, got {other:?}"),
        }
    }

    // ── Identity matching edge cases ─────────────────────────

    #[test]
    fn detect_tokens_updated_email_case_insensitive() {
        let _env = TestEnv::new();
        let val = realistic_auth_json("User@Example.COM", "acct_u", "acc_1", "ref_1");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        super::cmd_save(Some("case-profile")).unwrap();
        // Same email different case, new token
        let updated = realistic_auth_json("user@example.com", "acct_u", "acc_2", "ref_2");
        crate::auth::write_auth(&live, &updated).unwrap();
        match super::detect_auth_change() {
            super::AuthChange::TokensUpdated { alias } => assert_eq!(alias, "case-profile"),
            other => panic!("expected TokensUpdated, got {other:?}"),
        }
    }

    #[test]
    fn detect_tokens_updated_email_only_fallback_when_account_id_missing() {
        let _env = TestEnv::new();
        // Profile saved with account_id
        let val = realistic_auth_json("fallback@example.com", "acct_fb", "acc_1", "ref_1");
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        super::cmd_save(Some("fb-profile")).unwrap();
        // Live auth.json has no account_id in JWT claims (email-only match)
        let claims_no_id = serde_json::json!({
            "email": "fallback@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "organizations": [],
            }
        });
        let json_bytes = serde_json::to_vec(&claims_no_id).unwrap();
        let encoded = {
            use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
            URL_SAFE_NO_PAD.encode(json_bytes)
        };
        let jwt_no_id = format!("x.{encoded}.y");
        // account_id is empty string — should be treated as None after fix
        let updated = serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt_no_id,
                "access_token": "acc_new",
                "refresh_token": "ref_new",
                "account_id": "",
            },
            "last_refresh": "2026-04-08T00:00:00Z"
        });
        crate::auth::write_auth(&live, &updated).unwrap();
        match super::detect_auth_change() {
            super::AuthChange::TokensUpdated { alias } => assert_eq!(alias, "fb-profile"),
            other => panic!("expected TokensUpdated via email fallback, got {other:?}"),
        }
    }

    // ── update_profile_from_live ─────────────────────────────

    #[test]
    fn update_profile_from_live_syncs_content_and_preserves_others() {
        let _env = TestEnv::new();
        let live = crate::auth::codex_auth_path().unwrap();

        // Create two profiles
        let alice = realistic_auth_json("alice@example.com", "acct_a", "acc_a1", "ref_a1");
        crate::auth::write_auth(&live, &alice).unwrap();
        super::cmd_save(Some("alice")).unwrap();
        let bob = realistic_auth_json("bob@example.com", "acct_b", "acc_b1", "ref_b1");
        crate::auth::write_auth(&live, &bob).unwrap();
        super::cmd_save(Some("bob")).unwrap();

        // Update live with new alice tokens
        let alice_updated = realistic_auth_json("alice@example.com", "acct_a", "acc_a2", "ref_a2");
        crate::auth::write_auth(&live, &alice_updated).unwrap();
        super::update_profile_from_live("alice").unwrap();

        // Verify: alice's profile file content matches updated live
        let profile_val =
            crate::auth::read_auth(&super::profile_auth_path("alice").unwrap()).unwrap();
        assert_eq!(profile_val["tokens"]["access_token"], "acc_a2");
        assert_eq!(profile_val["tokens"]["refresh_token"], "ref_a2");
        assert_eq!(profile_val["OPENAI_API_KEY"], serde_json::Value::Null);

        // Verify: bob's profile was NOT modified
        let bob_val = crate::auth::read_auth(&super::profile_auth_path("bob").unwrap()).unwrap();
        assert_eq!(bob_val["tokens"]["access_token"], "acc_b1");

        // Verify: current marker updated
        assert_eq!(super::read_current(), "alice");
    }

    #[test]
    fn update_profile_from_live_rejects_different_account_identity() {
        let _env = TestEnv::new();
        let live = crate::auth::codex_auth_path().unwrap();
        let alice = realistic_auth_json("alice@example.com", "acct_a", "acc_a1", "ref_a1");
        crate::auth::write_auth(&live, &alice).unwrap();
        super::cmd_save(Some("alice")).unwrap();

        let bob = realistic_auth_json("bob@example.com", "acct_b", "acc_b1", "ref_b1");
        crate::auth::write_auth(&live, &bob).unwrap();

        let result = super::update_profile_from_live("alice");
        assert!(result.is_err());
        let saved = crate::auth::read_auth(&super::profile_auth_path("alice").unwrap()).unwrap();
        assert_eq!(saved["tokens"]["access_token"], "acc_a1");
    }

    #[test]
    fn relogin_rejects_different_account_identity() {
        let _env = TestEnv::new();
        let live = crate::auth::codex_auth_path().unwrap();
        let alice = realistic_auth_json("alice@example.com", "acct_a", "acc_a1", "ref_a1");
        crate::auth::write_auth(&live, &alice).unwrap();
        super::cmd_save(Some("alice")).unwrap();

        let bob = realistic_auth_json("bob@example.com", "acct_b", "acc_b1", "ref_b1");
        let result = super::replace_profile_auth_and_live_if_current("alice", &bob);
        assert!(result.is_err());
        let saved = crate::auth::read_auth(&super::profile_auth_path("alice").unwrap()).unwrap();
        assert_eq!(saved["tokens"]["access_token"], "acc_a1");
    }

    #[test]
    fn relogin_allows_matching_legacy_email_without_account_id() {
        let _env = TestEnv::new();
        let live = crate::auth::codex_auth_path().unwrap();
        let old = realistic_auth_json("alice@example.com", "", "acc_a1", "ref_a1");
        crate::auth::write_auth(&live, &old).unwrap();
        super::cmd_save(Some("alice")).unwrap();

        let refreshed = realistic_auth_json("Alice@example.com", "", "acc_a2", "ref_a2");
        super::replace_profile_auth_and_live_if_current("alice", &refreshed).unwrap();
        let saved = crate::auth::read_auth(&super::profile_auth_path("alice").unwrap()).unwrap();
        assert_eq!(saved["tokens"]["access_token"], "acc_a2");
    }

    // ── Failure paths ────────────────────────────────────────

    #[test]
    fn update_profile_from_live_fails_when_no_auth_file() {
        let _env = TestEnv::new();
        // No live auth.json exists
        let result = super::update_profile_from_live("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn detect_no_identity_in_jwt_returns_no_change() {
        let _env = TestEnv::new();
        // auth.json with no email in JWT, no account_id in claims,
        // and empty account_id in tokens (should be filtered to None)
        let empty_claims = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "organizations": [],
            }
        });
        let json_bytes = serde_json::to_vec(&empty_claims).unwrap();
        let encoded = {
            use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
            URL_SAFE_NO_PAD.encode(json_bytes)
        };
        let jwt_empty = format!("x.{encoded}.y");
        let val = serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt_empty,
                "access_token": "acc_x",
                "refresh_token": "ref_x",
                "account_id": "",
            },
            "last_refresh": "2026-04-07T00:00:00Z"
        });
        let live = crate::auth::codex_auth_path().unwrap();
        crate::auth::write_auth(&live, &val).unwrap();
        assert!(matches!(
            super::detect_auth_change(),
            super::AuthChange::NoChange
        ));
    }
}
