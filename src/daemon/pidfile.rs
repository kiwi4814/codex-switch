use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

static PIDFILE_HANDLE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

#[derive(Debug, Deserialize, Serialize)]
struct PidIdentity {
    version: u8,
    pid: u32,
    executable: PathBuf,
}

pub fn pidfile_path() -> Result<PathBuf> {
    Ok(crate::auth::app_home()?.join("daemon.pid"))
}

/// Atomically create a PID file using O_CREAT|O_EXCL semantics.
/// Fails if the file already exists (prevents TOCTOU race).
pub fn write_pidfile_exclusive() -> Result<()> {
    let path = pidfile_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // create_new(true) → O_CREAT | O_EXCL: atomic, fails if file exists.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!(
                    "PID file already exists at {}; another daemon may be running",
                    path.display()
                )
            } else {
                anyhow::anyhow!("Failed to create PID file {}: {e}", path.display())
            }
        })?;
    FileExt::lock(&file)
        .map_err(|e| anyhow::anyhow!("Failed to lock PID file {}: {e}", path.display()))?;
    let identity = PidIdentity {
        version: 1,
        pid: std::process::id(),
        executable: std::env::current_exe()?,
    };
    use std::io::Write;
    let encoded = serde_json::to_vec(&identity)?;
    file.write_all(&encoded)
        .map_err(|e| anyhow::anyhow!("Failed to write PID to {}: {e}", path.display()))?;
    file.sync_data()
        .map_err(|e| anyhow::anyhow!("Failed to sync PID file {}: {e}", path.display()))?;

    let handle = PIDFILE_HANDLE.get_or_init(|| Mutex::new(None));
    let mut guard = handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(file);
    Ok(())
}

pub fn read_pidfile() -> Option<u32> {
    let path = pidfile_path().ok()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    read_pid_from_raw(&raw)
}

fn read_pid_from_raw(raw: &str) -> Option<u32> {
    parse_pid_identity(raw)
        .map(|identity| identity.pid)
        .or_else(|| raw.trim().parse::<u32>().ok().filter(|pid| *pid > 0))
}

fn parse_pid_identity(raw: &str) -> Option<PidIdentity> {
    let identity: PidIdentity = serde_json::from_str(raw).ok()?;
    (identity.version == 1 && identity.pid > 0 && !identity.executable.as_os_str().is_empty())
        .then_some(identity)
}

fn release_pidfile_handle() {
    let Some(handle) = PIDFILE_HANDLE.get() else {
        return;
    };
    let mut guard = handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(file) = guard.take() {
        let _ = FileExt::unlock(&file);
    }
}

pub fn cleanup_pidfile() -> Result<()> {
    release_pidfile_handle();
    let path = pidfile_path()?;
    cleanup_pidfile_at(&path)
}

fn cleanup_pidfile_at(path: &Path) -> Result<()> {
    if pidfile_lock_is_held(path) {
        anyhow::bail!(
            "Refusing to remove PID file {}: locked by a running daemon",
            path.display()
        );
    }
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    match FileExt::try_lock(&file) {
        Err(TryLockError::WouldBlock) => anyhow::bail!(
            "Refusing to remove PID file {}: locked by a running daemon",
            path.display()
        ),
        Err(TryLockError::Error(e)) => return Err(e.into()),
        Ok(()) => {}
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// RAII guard that cleans up the PID file on drop (including panics).
pub struct PidGuard;

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = cleanup_pidfile();
    }
}

fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) only checks if the process exists; no signal is sent.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        ret == 0
    }
    #[cfg(target_os = "windows")]
    {
        let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        tasklist_contains_pid(&stdout, pid)
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = pid;
        false
    }
}

#[cfg(any(target_os = "windows", test))]
fn tasklist_contains_pid(stdout: &str, pid: u32) -> bool {
    stdout.lines().any(|line| {
        let Some(csv) = line
            .trim()
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
        else {
            return false;
        };
        csv.split("\",\"")
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
            == Some(pid)
    })
}

fn pidfile_lock_is_held(path: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return false;
    };
    match FileExt::try_lock(&file) {
        Err(TryLockError::WouldBlock) => true,
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(TryLockError::Error(_)) => false,
    }
}

/// A daemon is trusted only while the process still exists and owns the
/// exclusive lock created for this exact daemon startup.
pub fn process_alive(pid: u32) -> bool {
    let Ok(path) = pidfile_path() else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Some(identity) = parse_pid_identity(&raw) else {
        return false;
    };
    identity.pid == pid && process_exists(pid) && pidfile_lock_is_held(&path)
}

/// Send SIGTERM to a process.
pub fn send_sigterm(pid: u32) -> Result<()> {
    if !process_alive(pid) {
        anyhow::bail!("Refusing to stop PID {pid}: daemon process identity is stale");
    }
    #[cfg(unix)]
    {
        // SAFETY: sending SIGTERM to a known PID.
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            anyhow::bail!("Failed to send SIGTERM to PID {pid}: {err}");
        }
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if stderr.is_empty() { stdout } else { stderr };
            anyhow::bail!("Failed to stop PID {pid}: {detail}");
        }
        Ok(())
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = pid;
        anyhow::bail!("Stopping daemon is not supported on this platform");
    }
}

pub fn is_daemon_running() -> bool {
    read_pidfile().is_some_and(process_alive)
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_pidfile_at, parse_pid_identity, pidfile_lock_is_held, read_pid_from_raw,
        tasklist_contains_pid,
    };
    use fs4::FileExt;

    #[test]
    fn tasklist_reads_pid_from_second_csv_column() {
        let output = r#""codex-switch.exe","4242","Console","1","12,345 K""#;
        assert!(tasklist_contains_pid(output, 4242));
        assert!(!tasklist_contains_pid(output, 1234));
    }

    #[test]
    fn tasklist_rejects_info_and_empty_output() {
        assert!(!tasklist_contains_pid(
            "INFO: No tasks are running which match the specified criteria.",
            4242,
        ));
        assert!(!tasklist_contains_pid("", 4242));
    }

    #[test]
    fn legacy_pidfile_is_not_trusted() {
        assert!(parse_pid_identity("4242").is_none());
        assert_eq!(read_pid_from_raw("4242"), Some(4242));
    }

    #[test]
    fn pidfile_lock_identifies_only_active_daemon_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.pid");
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        FileExt::lock(&file).unwrap();
        assert!(pidfile_lock_is_held(&path));

        FileExt::unlock(&file).unwrap();
        assert!(!pidfile_lock_is_held(&path));
    }

    #[test]
    fn concurrent_start_cannot_replace_a_locked_pidfile_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.pid");
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        FileExt::lock(&file).unwrap();

        let err = cleanup_pidfile_at(&path).unwrap_err();
        let contender = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path);

        assert!(path.exists());
        assert!(err.to_string().contains("locked by a running daemon"));
        assert_eq!(
            contender.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert!(pidfile_lock_is_held(&path));
        FileExt::unlock(&file).unwrap();
    }
}
