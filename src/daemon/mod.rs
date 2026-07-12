pub mod codex_process;
pub mod loop_runner;
pub mod notify;
pub mod pidfile;
pub mod service;
pub mod state;

use crate::cli::DaemonCommand;
use crate::output::{print_json, user_println};
use anyhow::Result;

pub async fn dispatch(cmd: DaemonCommand, json: bool) -> Result<()> {
    match cmd {
        DaemonCommand::Start { foreground } => start(foreground).await,
        DaemonCommand::Stop => stop(),
        DaemonCommand::Status => status(json),
        DaemonCommand::Install => service::install(),
        DaemonCommand::Uninstall => service::uninstall(),
    }
}

async fn start(foreground: bool) -> Result<()> {
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = foreground;
        anyhow::bail!("The background daemon is not supported on this platform.");
    }
    #[cfg(any(unix, target_os = "windows"))]
    {
        if pidfile::is_daemon_running() {
            anyhow::bail!(
                "Daemon is already running (PID {})",
                pidfile::read_pidfile().unwrap_or(0)
            );
        }
        // Clean up stale PID file before starting
        pidfile::cleanup_pidfile()?;
        if foreground {
            return run_foreground().await;
        }
        if service::is_installed() {
            return service::start_installed();
        }
        start_detached()
    }
}

async fn run_foreground() -> Result<()> {
    pidfile::write_pidfile_exclusive()?;
    // RAII guard ensures PID file is cleaned up even on panic
    let _guard = pidfile::PidGuard;
    tracing::info!("codex-switch daemon started (PID {})", std::process::id());
    loop_runner::run_daemon_loop().await
}

fn start_detached() -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut child = std::process::Command::new(exe)
        .args(["daemon", "start", "--foreground"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let pid = child.id();
    // Wait for the daemon to write its PID file, which signals it reached the
    // event loop. Polling the actual readiness signal is more reliable than a
    // fixed sleep on slow disks / CI / containers.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        // Did the child exit before initializing?
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!(
                "Daemon process (PID {pid}) exited immediately ({status}); check logs for details"
            );
        }
        if pidfile::read_pidfile() == Some(pid) {
            user_println(&format!("Daemon started (PID {pid})"));
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "Daemon (PID {pid}) did not initialize within 2s (no PID file written); check logs"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn stop() -> Result<()> {
    if service::is_installed() {
        match service::stop_installed() {
            Ok(()) => {
                wait_until_stopped_or_kill(pidfile::read_pidfile())?;
                let _ = pidfile::cleanup_pidfile();
                return Ok(());
            }
            Err(err) => {
                tracing::warn!("Failed to stop installed daemon service: {err}");
            }
        }
    }

    stop_detached()
}

fn stop_detached() -> Result<()> {
    let pid = pidfile::read_pidfile()
        .ok_or_else(|| anyhow::anyhow!("No daemon PID file found; daemon may not be running"))?;
    if !pidfile::process_alive(pid) {
        pidfile::cleanup_pidfile()?;
        user_println("Daemon was not running (stale PID file cleaned up)");
        return Ok(());
    }
    pidfile::send_sigterm(pid)?;
    #[cfg(target_os = "windows")]
    {
        let _ = pidfile::cleanup_pidfile();
    }
    user_println(&format!("Sent stop signal to daemon (PID {pid})"));
    Ok(())
}

fn wait_until_stopped_or_kill(pid: Option<u32>) -> Result<()> {
    match wait_until_stopped(pid) {
        Ok(()) => Ok(()),
        Err(err) => {
            tracing::warn!(
                "Daemon still running after service stop, falling back to PID stop: {err}"
            );
            stop_detached()?;
            wait_until_stopped(pid)
        }
    }
}

fn wait_until_stopped(pid: Option<u32>) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let running = match pid.or_else(pidfile::read_pidfile) {
            Some(pid) => pidfile::process_alive(pid),
            None => false,
        };
        if !running {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("Daemon did not stop within 10s");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub struct SelfUpdateDaemonRestart {
    pid: Option<u32>,
    service_installed: bool,
    stopped: bool,
}

impl SelfUpdateDaemonRestart {
    pub fn capture() -> Self {
        let pid = pidfile::read_pidfile().filter(|pid| pidfile::process_alive(*pid));
        Self {
            pid,
            service_installed: service::is_installed(),
            stopped: false,
        }
    }

    pub fn is_needed(&self) -> bool {
        self.pid.is_some()
    }

    pub fn stop_before_update(&mut self) -> Result<()> {
        if !self.is_needed() || self.stopped {
            return Ok(());
        }

        user_println("Stopping daemon before self-update...");
        if self.service_installed {
            service::stop_installed()?;
            wait_until_stopped_or_kill(self.pid)?;
            let _ = pidfile::cleanup_pidfile();
        } else {
            stop_detached()?;
            wait_until_stopped(self.pid)?;
        }
        self.stopped = true;
        Ok(())
    }

    pub fn restart_after_update(&mut self) -> Result<()> {
        if !self.stopped {
            return Ok(());
        }

        user_println("Restarting daemon after self-update...");
        if self.service_installed {
            service::start_installed()?;
        } else {
            start_detached()?;
        }
        self.stopped = false;
        Ok(())
    }
}

fn status(json: bool) -> Result<()> {
    let pidfile = pidfile::pidfile_path()?;
    let pid = pidfile::read_pidfile();
    let running = pid.is_some_and(pidfile::process_alive);
    let state = match (pid, running) {
        (Some(_), true) => "running",
        (Some(_), false) => "stale",
        (None, _) => "stopped",
    };

    // Loop-written snapshot; only meaningful while the daemon is running.
    let snapshot = if running { state::read() } else { None };

    if json {
        let cfg = crate::config::get();
        print_json(&serde_json::json!({
            "running": running,
            "state": state,
            "pid": pid,
            "pidfile": pidfile,
            "stale_pid_cleaned": state == "stale",
            "snapshot": snapshot,
            "platform": {
                "os": std::env::consts::OS,
                "daemon_start_supported": cfg!(any(unix, target_os = "windows")),
                "service_install_supported": cfg!(any(target_os = "macos", target_os = "linux", target_os = "windows")),
                "service_manager": service_manager_name(),
                "service_installed": service::is_installed(),
            },
            "config": {
                "poll_interval_secs": cfg.daemon.poll_interval_secs,
                "cache_refresh_interval_secs": cfg.daemon.cache_refresh_interval_secs,
                "auto_warmup": cfg.daemon.auto_warmup,
                "token_check_interval_secs": cfg.daemon.token_check_interval_secs,
                "switch_threshold": cfg.daemon.switch_threshold,
                "notify": cfg.daemon.notify,
                "log_level": cfg.daemon.log_level,
            }
        }));
        if state == "stale" {
            pidfile::cleanup_pidfile()?;
        }
        return Ok(());
    }

    #[cfg(any(unix, target_os = "windows"))]
    {
        match (pid, running) {
            (Some(pid), true) => {
                user_println(&format!("Daemon is running (PID {pid})"));
                if let Some(snap) = &snapshot {
                    if let Some(at) = snap.last_poll_at {
                        user_println(&format!("  Last poll: {}", format_unix(at)));
                    }
                    if let Some(sw) = &snap.last_switch {
                        user_println(&format!(
                            "  Last switch: '{}' -> '{}' at {} (score {:.0})",
                            sw.from,
                            sw.to,
                            format_unix(sw.at),
                            sw.score
                        ));
                    }
                    if let Some(p) = &snap.pending_switch {
                        user_println(&format!(
                            "  Pending switch to '{}' since {} (waiting for Codex session to end)",
                            p.to,
                            format_unix(p.since)
                        ));
                    }
                    if let Some(err) = &snap.last_error {
                        user_println(&format!(
                            "  Last error ({} consecutive): {err}",
                            snap.consecutive_failures
                        ));
                    }
                }
            }
            (Some(pid), false) => {
                user_println(&format!("Daemon is not running (stale PID {pid})"));
                pidfile::cleanup_pidfile()?;
            }
            (None, _) => {
                user_println("Daemon is not running");
            }
        }
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        user_println(&format!(
            "Daemon is not supported on this platform ({})",
            std::env::consts::OS
        ));
    }
    Ok(())
}

#[cfg(any(unix, target_os = "windows"))]
fn format_unix(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn service_manager_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "launchd"
    }
    #[cfg(target_os = "linux")]
    {
        "systemd-user"
    }
    #[cfg(target_os = "windows")]
    {
        "task-scheduler"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unsupported"
    }
}
