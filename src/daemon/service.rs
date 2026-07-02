use crate::output::user_println;
use anyhow::Result;
#[cfg(any(target_os = "windows", test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
const WINDOWS_TASK_NAME: &str = r"\codex-switch-daemon";

pub fn install() -> Result<()> {
    #[cfg(target_os = "macos")]
    return install_launchd();
    #[cfg(target_os = "linux")]
    return install_systemd();
    #[cfg(target_os = "windows")]
    return install_task_scheduler();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    anyhow::bail!("Service install is not supported on this platform")
}

pub fn uninstall() -> Result<()> {
    #[cfg(target_os = "macos")]
    return uninstall_launchd();
    #[cfg(target_os = "linux")]
    return uninstall_systemd();
    #[cfg(target_os = "windows")]
    return uninstall_task_scheduler();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    anyhow::bail!("Service uninstall is not supported on this platform")
}

pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        plist_path().is_ok_and(|path| path.exists())
    }
    #[cfg(target_os = "linux")]
    {
        unit_path().is_ok_and(|path| path.exists())
    }
    #[cfg(target_os = "windows")]
    {
        schtasks_status(&["/Query", "/TN", WINDOWS_TASK_NAME]).is_ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "windows")]
pub fn start_installed() -> Result<()> {
    schtasks(&["/Run", "/TN", WINDOWS_TASK_NAME], "start scheduled task")?;
    user_println("Started Windows scheduled task");
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn stop_installed() -> Result<()> {
    schtasks(&["/End", "/TN", WINDOWS_TASK_NAME], "stop scheduled task")?;
    user_println("Stopped Windows scheduled task");
    Ok(())
}

// -- macOS LaunchAgent --

#[cfg(target_os = "macos")]
fn plist_path() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join("Library/LaunchAgents/com.codex-switch.daemon.plist"))
}

#[cfg(target_os = "macos")]
fn install_launchd() -> Result<()> {
    let exe = std::env::current_exe()?.display().to_string();
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .display()
        .to_string();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.codex-switch.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
    </dict>
</dict>
</plist>"#,
        exe = exe,
        home = home,
    );

    let path = plist_path()?;
    if path.exists() {
        user_println(&format!(
            "Warning: overwriting existing LaunchAgent at {}",
            path.display()
        ));
        // Unload the old service first to avoid launchctl conflicts
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &path.display().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, plist)?;

    let status = std::process::Command::new("launchctl")
        .args(["load", &path.display().to_string()])
        .status()?;
    if !status.success() {
        anyhow::bail!("launchctl load failed");
    }
    user_println(&format!("Installed LaunchAgent at {}", path.display()));
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<()> {
    let path = plist_path()?;
    if !path.exists() {
        user_println("LaunchAgent not installed");
        return Ok(());
    }
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path.display().to_string()])
        .status();
    std::fs::remove_file(&path)?;
    user_println("Uninstalled LaunchAgent");
    Ok(())
}

// -- Linux systemd --

#[cfg(target_os = "linux")]
fn unit_path() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    Ok(home.join(".config/systemd/user/codex-switch-daemon.service"))
}

#[cfg(target_os = "linux")]
fn install_systemd() -> Result<()> {
    let exe = std::env::current_exe()?.display().to_string();
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .display()
        .to_string();

    let unit = format!(
        r#"[Unit]
Description=codex-switch auto-switching daemon
After=network-online.target

[Service]
Type=simple
ExecStart={exe} daemon start --foreground
Restart=on-failure
RestartSec=10
Environment=HOME={home}

[Install]
WantedBy=default.target
"#,
        exe = exe,
        home = home,
    );

    let path = unit_path()?;
    if path.exists() {
        user_println(&format!(
            "Warning: overwriting existing systemd service at {}",
            path.display()
        ));
        // Stop the old service first to avoid conflicts
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", "codex-switch-daemon"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, unit)?;

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let status = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "codex-switch-daemon"])
        .status()?;
    if !status.success() {
        anyhow::bail!("systemctl enable failed");
    }
    user_println(&format!(
        "Installed systemd user service at {}",
        path.display()
    ));
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> Result<()> {
    let path = unit_path()?;
    if !path.exists() {
        user_println("systemd service not installed");
        return Ok(());
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "codex-switch-daemon"])
        .status();
    std::fs::remove_file(&path)?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    user_println("Uninstalled systemd user service");
    Ok(())
}

// -- Windows Task Scheduler --

#[cfg(any(target_os = "windows", test))]
fn task_scheduler_command(exe: &Path) -> String {
    format!(
        "\"{}\" daemon start --foreground",
        exe.display().to_string().replace('"', "")
    )
}

#[cfg(target_os = "windows")]
fn schtasks_status(args: &[&str]) -> Result<std::process::ExitStatus> {
    Ok(std::process::Command::new("schtasks")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?)
}

#[cfg(target_os = "windows")]
fn schtasks(args: &[&str], action: &str) -> Result<std::process::Output> {
    let output = std::process::Command::new("schtasks").args(args).output()?;
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    anyhow::bail!("failed to {action}: {detail}");
}

#[cfg(target_os = "windows")]
fn install_task_scheduler() -> Result<()> {
    let exe = std::env::current_exe()?;
    let task_run = task_scheduler_command(&exe);

    schtasks(
        &[
            "/Create",
            "/TN",
            WINDOWS_TASK_NAME,
            "/TR",
            &task_run,
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/IT",
            "/F",
        ],
        "create scheduled task",
    )?;
    schtasks(&["/Run", "/TN", WINDOWS_TASK_NAME], "start scheduled task")?;
    user_println(&format!(
        "Installed Windows scheduled task {}",
        WINDOWS_TASK_NAME
    ));
    Ok(())
}

#[cfg(target_os = "windows")]
fn uninstall_task_scheduler() -> Result<()> {
    let _ = schtasks(&["/End", "/TN", WINDOWS_TASK_NAME], "stop scheduled task");
    if !is_installed() {
        user_println("Windows scheduled task not installed");
        return Ok(());
    }
    schtasks(
        &["/Delete", "/TN", WINDOWS_TASK_NAME, "/F"],
        "delete scheduled task",
    )?;
    user_println("Uninstalled Windows scheduled task");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::task_scheduler_command;
    use std::path::Path;

    #[test]
    fn windows_task_scheduler_command_quotes_exe_path() {
        let cmd = task_scheduler_command(Path::new(r"C:\Program Files\codex-switch.exe"));
        assert_eq!(
            cmd,
            r#""C:\Program Files\codex-switch.exe" daemon start --foreground"#
        );
    }
}
