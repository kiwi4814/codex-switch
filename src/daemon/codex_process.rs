//! Detection of a running interactive Codex CLI session.
//!
//! The daemon must not swap `auth.json` under an active Codex conversation,
//! so before switching it scans process command lines. Long-lived Codex
//! infrastructure (MCP servers, `app-server` for desktop/IDE hosts, helper
//! binaries) shares the same binary but never holds a conversation — those
//! must not block switching, or machines running Codex-backed tooling would
//! defer forever. Detection is best-effort: if the process listing fails we
//! assume no session rather than blocking switches.

/// True when an interactive Codex CLI session appears to be running.
pub fn codex_process_running() -> bool {
    list_process_command_lines().is_some_and(|out| out.lines().any(is_codex_session_command))
}

/// One full process command line per line.
#[cfg(unix)]
fn list_process_command_lines() -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-eo", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(windows)]
fn list_process_command_lines() -> Option<String> {
    // tasklist has no command lines; CIM does. Filter server-side to codex*.
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name LIKE 'codex%' OR Name = 'node.exe'\" | ForEach-Object { $_.CommandLine }",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// True when a command line looks like an interactive Codex session
/// (bare `codex`, `codex resume`, `codex exec`, …) rather than Codex
/// infrastructure (`mcp-server`, `app-server`, helper binaries).
fn is_codex_session_command(cmdline: &str) -> bool {
    let Some((first, mut rest)) = split_first_token(cmdline) else {
        return false;
    };
    let mut bin = basename(first);

    // npm shim: `node /path/to/bin/codex <args…>`
    if bin.eq_ignore_ascii_case("node") || bin.eq_ignore_ascii_case("node.exe") {
        let Some((shim_target, shim_rest)) = split_first_token(rest) else {
            return false;
        };
        bin = basename(shim_target);
        rest = shim_rest;
    }

    if !is_codex_binary_name(bin) {
        return false;
    }

    // Long-lived infrastructure subcommands never hold a conversation.
    const INFRA_TOKENS: &[&str] = &[
        "mcp",
        "mcp-server",
        "app-server",
        "login",
        "logout",
        "completion",
        "--version",
        "--help",
    ];
    !rest.split_whitespace().any(|t| INFRA_TOKENS.contains(&t))
}

/// Split off the first command-line token, honoring a double-quoted leading
/// path (Windows CommandLine quotes executables whose path contains spaces).
fn split_first_token(cmdline: &str) -> Option<(&str, &str)> {
    let s = cmdline.trim_start();
    if s.is_empty() {
        return None;
    }
    if let Some(stripped) = s.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some((&stripped[..end], &stripped[end + 1..]))
    } else {
        let mut parts = s.splitn(2, char::is_whitespace);
        let first = parts.next()?;
        Some((first, parts.next().unwrap_or("")))
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Match the Codex CLI by binary name. Accepts plain `codex`, platform
/// binaries like `codex-aarch64-apple-darwin` (npm vendor binary; Linux comm
/// truncation is irrelevant here since we read full command lines), and
/// Windows `codex.exe` — but never our own `codex-switch` or Codex helper
/// binaries like `codex-code-mode-host`.
fn is_codex_binary_name(base: &str) -> bool {
    let base = base.trim_matches('"');
    let base = base.strip_suffix(".exe").unwrap_or(base);
    base == "codex"
        || (base.starts_with("codex-")
            && !base.starts_with("codex-switch")
            && !base.starts_with("codex-code-mode"))
}

#[cfg(test)]
mod tests {
    use super::is_codex_session_command;

    #[test]
    fn interactive_sessions_are_detected() {
        // Real-world command lines observed on macOS (npm vendor binary + shim).
        assert!(is_codex_session_command(
            "/Users/u/.nvm/versions/node/v24.14.0/lib/node_modules/@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex"
        ));
        assert!(is_codex_session_command(
            "/usr/local/bin/codex resume 019f400c-35d3-77c3-96b0-85d3ff6b75fe"
        ));
        assert!(is_codex_session_command("codex exec \"fix the tests\""));
        assert!(is_codex_session_command(
            "node /Users/u/.nvm/versions/node/v24.14.0/bin/codex"
        ));
        assert!(is_codex_session_command(
            "\"C:\\Program Files\\codex\\codex.exe\" resume abc"
        ));
    }

    #[test]
    fn codex_infrastructure_does_not_block_switching() {
        assert!(!is_codex_session_command(
            "/Users/u/.nvm/.../bin/codex -m gpt-5.4 -c model_reasoning_effort=high mcp-server"
        ));
        assert!(!is_codex_session_command(
            "node /Users/u/.nvm/versions/node/v24.14.0/bin/codex -m gpt-5.4 mcp-server"
        ));
        assert!(!is_codex_session_command(
            "/Applications/ChatGPT.app/Contents/Resources/codex -c features.code_mode_host=true app-server --analytics-default-enabled"
        ));
        assert!(!is_codex_session_command("/usr/local/bin/codex app-server"));
        assert!(!is_codex_session_command("codex login --device"));
        assert!(!is_codex_session_command("/path/bin/codex-code-mode-host"));
        assert!(!is_codex_session_command(
            "/Users/u/Developer/Repos/codex-switch/target/debug/codex-switch daemon start --foreground"
        ));
        assert!(!is_codex_session_command(
            "/Applications/Pencil.app/Contents/Resources/app.asar.unpacked/out/mcp-server-darwin-arm64 --app desktop --agent codexCLI"
        ));
        assert!(!is_codex_session_command("bash"));
        assert!(!is_codex_session_command(""));
    }
}
