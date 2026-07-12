/// TUI menu state machines for Phase 2:
///   - Account menu (single-account actions)
///   - Add menu (OAuth flow choice for new account)
///   - OAuth flow choice (browser vs device code, used by re-login)
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::popup::{PopupState, render_popup, render_responsive_split_popup};

const C_WHITE: Color = Color::Rgb(240, 240, 240);
const DIM: Color = Color::Rgb(120, 120, 120);
const C_RED: Color = Color::Rgb(255, 90, 90);
const C_GREEN: Color = Color::Rgb(80, 220, 120);
const C_YELLOW: Color = Color::Rgb(255, 220, 80);
const C_CYAN: Color = Color::Rgb(100, 210, 255);

/// Active menu state. Only one menu is visible at a time.
pub enum MenuState {
    /// Account-scoped action menu (Enter on a single account).
    Account {
        info: Box<AccountMenuInfo>,
        popup: PopupState,
    },
    /// Add new account: choose OAuth flow.
    Add { popup: PopupState },
    /// Re-login: choose OAuth flow for an existing account.
    ReloginFlow {
        alias: String,
        email: Option<String>,
        popup: PopupState,
    },
    /// Batch menu shown when one or more accounts are marked.
    Batch { count: usize, popup: PopupState },
    /// Batch re-login flow chooser (browser vs device code).
    BatchReloginFlow { count: usize, popup: PopupState },
}

#[derive(Debug, Clone)]
pub struct AccountMenuInfo {
    pub alias: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub workspace_name: Option<String>,
    pub is_fedramp: bool,
    pub plan_label: String,
    pub plan_type: Option<String>,
    pub is_current: bool,
    pub organizations: Vec<String>,
    pub auth_expiries: Vec<String>,
    pub usage: Option<Box<crate::usage::UsageInfo>>,
    pub usage_meta: Vec<String>,
    pub models: Vec<String>,
    pub reset_cards: Option<u64>,
    pub reset_card_expiries: Vec<String>,
    pub can_consume_reset_card: bool,
}

#[derive(Debug, Clone)]
pub enum MenuAction {
    /// Keep the menu open and ignore the key.
    Noop,
    /// Close the menu, no further action.
    Close,
    /// Switch to alias.
    Use(String),
    /// Open re-login flow chooser for alias.
    ReloginRequest(String, Option<String>),
    /// Trigger re-login with chosen flow.
    Relogin { alias: String, device: bool },
    /// Trigger add-new-account with chosen flow.
    Add { device: bool },
    /// Refresh usage and model metadata for one account.
    RefreshOne(String),
    /// Open rename input for alias.
    Rename(String),
    /// Warmup just this alias.
    WarmupOne(String),
    /// Consume the earliest-expiring reset card for alias.
    ConsumeResetCard(String),
    /// Request delete confirmation for alias.
    DeleteRequest(String),

    // Batch actions ────────────────────────────
    /// Force-refresh all marked accounts.
    BatchRefresh,
    /// Warmup all marked accounts.
    BatchWarmup,
    /// Open OAuth flow chooser for batch re-login.
    BatchReloginRequest,
    /// Re-login marked accounts sequentially using `device` flow.
    BatchRelogin { device: bool },
    /// Request batch-delete confirmation.
    BatchDeleteRequest,
}

fn quota_window_lines(
    window: &crate::usage::WindowUsage,
    fallback_label: &str,
) -> Vec<Line<'static>> {
    const BAR_WIDTH: usize = 22;
    let label = match window.window_minutes {
        Some(minutes) if minutes % 1_440 == 0 => format!("{}d", minutes / 1_440),
        Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{minutes}m"),
        None => fallback_label.to_string(),
    };
    let used = window.used_percent.unwrap_or(0.0).clamp(0.0, 100.0);
    let remaining = (100.0 - used).max(0.0);
    let used_width = ((used / 100.0) * BAR_WIDTH as f64).round() as usize;
    let used_color = if used >= 90.0 {
        C_RED
    } else if used >= 70.0 {
        C_YELLOW
    } else {
        C_GREEN
    };
    let mut spans = vec![Span::styled(
        format!("{label:<3} "),
        Style::default().fg(C_WHITE),
    )];
    spans.push(Span::styled(
        "█".repeat(used_width),
        Style::default().fg(used_color),
    ));
    spans.push(Span::styled(
        "░".repeat(BAR_WIDTH.saturating_sub(used_width)),
        Style::default().fg(DIM),
    ));
    spans.push(Span::styled(
        format!("  {remaining:.0}% left"),
        Style::default().fg(if remaining <= 10.0 { C_RED } else { C_YELLOW }),
    ));
    let reset = window
        .resets_at
        .map(crate::output::format_local_timestamp)
        .unwrap_or_else(|| "--".to_string());
    let window_secs = window
        .window_minutes
        .map(|minutes| minutes.saturating_mul(60))
        .unwrap_or_else(|| {
            if fallback_label == "5h" {
                crate::usage::WINDOW_5H_SECS
            } else {
                crate::usage::WINDOW_7D_SECS
            }
        });
    let pace = crate::usage::pace_percent(window, window_secs);
    let pace_line = pace.map(|pace| {
        let delta = used - pace;
        let state = if delta > 0.0 {
            format!("{delta:.0}% over")
        } else if delta < 0.0 {
            format!("{:.0}% under", -delta)
        } else {
            "on pace".to_string()
        };
        let rest = if delta > 0.0 {
            let seconds = ((delta * window_secs as f64 / 100.0) as i64).max(1);
            format!(" · Rest {} to pace", format_duration(seconds))
        } else {
            " · Rest not needed".to_string()
        };
        format!("    Pace {pace:.0}% expected · {state}{rest}")
    });
    let reset_relative = window
        .resets_at
        .map(crate::output::format_reset_time)
        .unwrap_or_else(|| "--".to_string());
    let mut lines = vec![Line::from(spans)];
    if let Some(pace_line) = pace_line {
        lines.push(Line::from(Span::styled(
            pace_line,
            Style::default().fg(if used > pace.unwrap_or(used) {
                C_YELLOW
            } else {
                DIM
            }),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!("    Reset {reset} · {reset_relative}"),
        Style::default().fg(DIM),
    )));
    lines
}

fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{}m", minutes.max(1))
    }
}

fn quota_lines(usage: Option<&crate::usage::UsageInfo>) -> Vec<Line<'static>> {
    let Some(usage) = usage else {
        return vec![Line::from(Span::styled(
            "Usage not loaded",
            Style::default().fg(DIM),
        ))];
    };
    let mut lines = Vec::new();
    let mut add_pool = |name: &str,
                        primary: Option<&crate::usage::WindowUsage>,
                        secondary: Option<&crate::usage::WindowUsage>,
                        unavailable: bool| {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(vec![
            Span::styled(
                name.to_string(),
                Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if unavailable { "  unavailable" } else { "" },
                Style::default().fg(C_RED),
            ),
        ]));
        if let Some(window) = primary {
            lines.extend(quota_window_lines(window, "5h"));
        }
        if let Some(window) = secondary {
            lines.extend(quota_window_lines(window, "7d"));
        }
        if primary.is_none() && secondary.is_none() {
            lines.push(Line::from(Span::styled(
                "  No active window",
                Style::default().fg(DIM),
            )));
        }
    };
    add_pool(
        "Main",
        usage.primary.as_ref(),
        usage.secondary.as_ref(),
        false,
    );
    for pool in &usage.additional_limits {
        add_pool(
            pool.limit_name.as_deref().unwrap_or("Additional"),
            pool.primary.as_ref(),
            pool.secondary.as_ref(),
            pool.allowed == Some(false) || pool.limit_reached == Some(true),
        );
    }
    lines
}

impl MenuState {
    pub fn account(info: AccountMenuInfo) -> Self {
        MenuState::Account {
            info: Box::new(info),
            popup: PopupState::new(),
        }
    }

    pub fn add() -> Self {
        MenuState::Add {
            popup: PopupState::new(),
        }
    }

    pub fn relogin_flow(alias: String, email: Option<String>) -> Self {
        MenuState::ReloginFlow {
            alias,
            email,
            popup: PopupState::new(),
        }
    }

    pub fn batch(count: usize) -> Self {
        MenuState::Batch {
            count,
            popup: PopupState::new(),
        }
    }

    pub fn batch_relogin_flow(count: usize) -> Self {
        MenuState::BatchReloginFlow {
            count,
            popup: PopupState::new(),
        }
    }

    /// Translate a key press into an action. Returns `Close` to dismiss menu only.
    pub fn handle_key(&mut self, code: ratatui::crossterm::event::KeyCode) -> MenuAction {
        use ratatui::crossterm::event::KeyCode;
        match self {
            MenuState::Account { info, popup } => match code {
                KeyCode::Esc | KeyCode::Char('q') => MenuAction::Close,
                KeyCode::Down | KeyCode::Char('j') => {
                    popup.scroll_down(u16::MAX);
                    MenuAction::Noop
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    popup.scroll_up();
                    MenuAction::Noop
                }
                KeyCode::PageDown => {
                    popup.page_down(5, u16::MAX);
                    MenuAction::Noop
                }
                KeyCode::PageUp => {
                    popup.page_up(5);
                    MenuAction::Noop
                }
                KeyCode::Home => {
                    popup.reset();
                    MenuAction::Noop
                }
                KeyCode::Char('u') => MenuAction::Use(info.alias.clone()),
                KeyCode::Char('l') => {
                    MenuAction::ReloginRequest(info.alias.clone(), info.email.clone())
                }
                KeyCode::Char('n') => MenuAction::Rename(info.alias.clone()),
                KeyCode::Char('r') => MenuAction::RefreshOne(info.alias.clone()),
                KeyCode::Char('w') => MenuAction::WarmupOne(info.alias.clone()),
                KeyCode::Char('c') => MenuAction::ConsumeResetCard(info.alias.clone()),
                KeyCode::Char('d') => MenuAction::DeleteRequest(info.alias.clone()),
                _ => MenuAction::Noop,
            },
            MenuState::Add { .. } => match code {
                KeyCode::Esc | KeyCode::Char('q') => MenuAction::Close,
                KeyCode::Char('b') => MenuAction::Add { device: false },
                KeyCode::Char('d') => MenuAction::Add { device: true },
                _ => MenuAction::Noop,
            },
            MenuState::ReloginFlow { alias, .. } => match code {
                KeyCode::Esc | KeyCode::Char('q') => MenuAction::Close,
                KeyCode::Char('b') => MenuAction::Relogin {
                    alias: alias.clone(),
                    device: false,
                },
                KeyCode::Char('d') => MenuAction::Relogin {
                    alias: alias.clone(),
                    device: true,
                },
                _ => MenuAction::Noop,
            },
            MenuState::Batch { .. } => match code {
                KeyCode::Esc | KeyCode::Char('q') => MenuAction::Close,
                KeyCode::Char('r') => MenuAction::BatchRefresh,
                KeyCode::Char('w') => MenuAction::BatchWarmup,
                KeyCode::Char('l') => MenuAction::BatchReloginRequest,
                KeyCode::Char('d') => MenuAction::BatchDeleteRequest,
                _ => MenuAction::Noop,
            },
            MenuState::BatchReloginFlow { .. } => match code {
                KeyCode::Esc | KeyCode::Char('q') => MenuAction::Close,
                KeyCode::Char('b') => MenuAction::BatchRelogin { device: false },
                KeyCode::Char('d') => MenuAction::BatchRelogin { device: true },
                _ => MenuAction::Noop,
            },
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let key_style = Style::default().fg(C_YELLOW).add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(C_WHITE);
        let dim = Style::default().fg(DIM);
        let header_style = Style::default().fg(C_CYAN);

        match self {
            MenuState::Account { info, popup } => {
                let title = "Account details";
                let mut left_lines = vec![Line::from(Span::styled(
                    "Identity",
                    header_style.add_modifier(Modifier::BOLD),
                ))];
                let mut identity = vec![
                    Span::styled(
                        info.alias.clone(),
                        Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        info.plan_label.clone(),
                        Style::default().fg(C_YELLOW).add_modifier(Modifier::BOLD),
                    ),
                ];
                if info.is_current {
                    identity.push(Span::styled(
                        "  ● active",
                        Style::default().fg(C_GREEN).add_modifier(Modifier::BOLD),
                    ));
                }
                left_lines.push(Line::from(identity));
                if let Some(email) = &info.email {
                    left_lines.push(Line::from(vec![
                        Span::styled("email      ", dim),
                        Span::styled(email.clone(), Style::default().fg(C_WHITE)),
                    ]));
                }
                if info.workspace_name.is_some() || info.plan_type.is_some() {
                    left_lines.push(Line::from(vec![
                        Span::styled("workspace  ", dim),
                        Span::styled(
                            info.workspace_name
                                .clone()
                                .unwrap_or_else(|| "Personal".into()),
                            label_style,
                        ),
                        Span::styled(
                            info.plan_type
                                .as_ref()
                                .map(|value| format!("  ·  {value}"))
                                .unwrap_or_default(),
                            dim,
                        ),
                    ]));
                }
                if let Some(account_id) = &info.account_id {
                    left_lines.push(Line::from(vec![
                        Span::styled("account id ", dim),
                        Span::styled(account_id.clone(), dim),
                    ]));
                }
                if let Some(user_id) = &info.user_id {
                    left_lines.push(Line::from(vec![
                        Span::styled("user id    ", dim),
                        Span::styled(user_id.clone(), dim),
                    ]));
                }
                if info.is_fedramp {
                    left_lines.push(Line::from(vec![
                        Span::styled("route      ", dim),
                        Span::styled("FedRAMP", Style::default().fg(C_YELLOW)),
                    ]));
                }
                for organization in &info.organizations {
                    left_lines.push(Line::from(vec![
                        Span::styled("organization  ", dim),
                        Span::styled(organization.clone(), label_style),
                    ]));
                }
                for expiry in &info.auth_expiries {
                    if let Some((name, details)) = expiry.split_once(" · ") {
                        left_lines.push(Line::from(vec![
                            Span::styled(
                                name.to_string(),
                                Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!(" · {details}"), dim),
                        ]));
                    } else {
                        left_lines.push(Line::from(Span::styled(expiry.clone(), dim)));
                    }
                }
                left_lines.push(Line::from(""));
                left_lines.push(Line::from(Span::styled(
                    "Quota pools",
                    header_style.add_modifier(Modifier::BOLD),
                )));
                left_lines.extend(quota_lines(info.usage.as_deref()));
                for item in &info.usage_meta {
                    left_lines.push(Line::from(Span::styled(item.clone(), dim)));
                }
                let cards = info
                    .reset_cards
                    .map(|count| format!("{count} available"))
                    .unwrap_or_else(|| "not available".to_string());
                left_lines.push(Line::from(vec![
                    Span::styled("Reset cards  ", header_style.add_modifier(Modifier::BOLD)),
                    Span::styled(
                        cards,
                        Style::default().fg(if info.can_consume_reset_card {
                            C_GREEN
                        } else {
                            DIM
                        }),
                    ),
                ]));
                for (idx, expiry) in info.reset_card_expiries.iter().enumerate() {
                    let note = if idx == 0 { "  next to use" } else { "" };
                    left_lines.push(Line::from(vec![
                        Span::styled(format!("  #{}  ", idx + 1), dim),
                        Span::styled(expiry.clone(), label_style),
                        Span::styled(note, dim),
                    ]));
                }
                let mut right_lines = vec![Line::from(Span::styled(
                    "Models",
                    header_style.add_modifier(Modifier::BOLD),
                ))];
                let mut first_model = true;
                for model in &info.models {
                    if model.starts_with("    ") {
                        right_lines.push(Line::from(Span::styled(model.clone(), dim)));
                    } else {
                        if !first_model {
                            right_lines.push(Line::from(""));
                        }
                        first_model = false;
                        right_lines.push(Line::from(vec![
                            Span::styled("● ", Style::default().fg(C_CYAN)),
                            Span::styled(
                                model.trim().to_string(),
                                Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD),
                            ),
                        ]));
                    }
                }
                left_lines.push(Line::from(""));
                left_lines.push(Line::from(Span::styled(
                    "Actions",
                    header_style.add_modifier(Modifier::BOLD),
                )));
                let actions = [
                    ("u", "use", true),
                    ("r", "refresh", true),
                    ("w", "warmup", true),
                    ("c", "card", info.can_consume_reset_card),
                    ("l", "login", true),
                    ("n", "rename", true),
                    ("d", "delete", true),
                ];
                for row in [&actions[..4], &actions[4..]] {
                    let mut action_spans = Vec::new();
                    for (idx, (key, label, enabled)) in row.iter().enumerate() {
                        if idx > 0 {
                            action_spans.push(Span::styled("  ·  ", dim));
                        }
                        action_spans.push(Span::styled(
                            (*key).to_string(),
                            if *enabled { key_style } else { dim },
                        ));
                        action_spans.push(Span::styled(
                            format!(" {label}"),
                            if *enabled { label_style } else { dim },
                        ));
                    }
                    left_lines.push(Line::from(action_spans));
                }
                left_lines.push(Line::from(""));
                left_lines.push(Line::from(Span::styled(
                    "j k / arrows / PgUp PgDn scroll models · esc / q cancel",
                    dim,
                )));
                render_responsive_split_popup(f, title, &left_lines, &right_lines, popup, area);
            }
            MenuState::Add { popup } => {
                let title = "Add new account";
                let mut lines: Vec<Line<'static>> = Vec::new();
                lines.push(Line::from(Span::styled("Choose OAuth flow:", header_style)));
                lines.push(Line::from(""));
                lines.extend(menu_items(
                    &[
                        ("b", "Browser (PKCE, opens local callback)"),
                        ("d", "Device code (for headless / no browser)"),
                    ],
                    key_style,
                    label_style,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("esc / q to cancel", dim)));
                render_popup(f, title, &lines, popup, area);
            }
            MenuState::ReloginFlow {
                alias,
                email,
                popup,
            } => {
                let header = match email {
                    Some(e) => format!("{alias}  ({e})"),
                    None => alias.clone(),
                };
                let mut lines: Vec<Line<'static>> =
                    vec![Line::from(Span::styled(header, header_style))];
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Choose OAuth flow:", header_style)));
                lines.push(Line::from(""));
                lines.extend(menu_items(
                    &[
                        ("b", "Browser (PKCE, opens local callback)"),
                        ("d", "Device code (for headless / no browser)"),
                    ],
                    key_style,
                    label_style,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("esc / q to cancel", dim)));
                render_popup(f, "re-Login", &lines, popup, area);
            }
            MenuState::Batch { count, popup } => {
                let title = "Batch";
                let header = format!("{count} account(s) marked");
                let mut lines: Vec<Line<'static>> = Vec::new();
                lines.push(Line::from(Span::styled(header, header_style)));
                lines.push(Line::from(""));
                lines.extend(menu_items(
                    &[
                        ("r", "Refresh selected"),
                        ("w", "Warmup selected"),
                        ("l", "re-Login selected (sequential)"),
                        ("d", "Delete selected"),
                    ],
                    key_style,
                    label_style,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("esc / q to cancel", dim)));
                render_popup(f, title, &lines, popup, area);
            }
            MenuState::BatchReloginFlow { count, popup } => {
                let mut lines: Vec<Line<'static>> = Vec::new();
                lines.push(Line::from(Span::styled(
                    format!("{count} account(s) marked"),
                    header_style,
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Sequential re-login. Browser uses local port 1455 each round.",
                    Style::default().fg(DIM),
                )));
                lines.push(Line::from(""));
                lines.extend(menu_items(
                    &[("b", "Browser (PKCE)"), ("d", "Device code")],
                    key_style,
                    label_style,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("esc / q to cancel", dim)));
                render_popup(f, "Batch re-Login", &lines, popup, area);
            }
        }
    }
}

fn menu_items(items: &[(&str, &str)], key_style: Style, label_style: Style) -> Vec<Line<'static>> {
    let key_w = items
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(1);
    items
        .iter()
        .map(|(k, label)| {
            let pad = key_w.saturating_sub(k.chars().count());
            Line::from(vec![
                Span::raw("  "),
                Span::styled((*k).to_string(), key_style),
                Span::raw(" ".repeat(pad)),
                Span::raw("  "),
                Span::styled((*label).to_string(), label_style),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, crossterm::event::KeyCode};

    use super::{AccountMenuInfo, MenuAction, MenuState, quota_lines};
    use crate::usage::{AdditionalRateLimit, UsageInfo, WindowUsage};

    fn find_text(backend: &TestBackend, needle: &str) -> Option<(u16, u16)> {
        let area = backend.buffer().area;
        for y in 0..area.height {
            let row = (0..area.width)
                .map(|x| {
                    backend
                        .buffer()
                        .cell((x, y))
                        .expect("cell inside test buffer")
                        .symbol()
                })
                .collect::<String>();
            if let Some(x) = row.find(needle) {
                return Some((x as u16, y));
            }
        }
        None
    }

    #[test]
    fn unknown_key_keeps_menu_open() {
        let mut menu = MenuState::add();
        assert!(matches!(
            menu.handle_key(KeyCode::Char('x')),
            MenuAction::Noop
        ));
    }

    #[test]
    fn account_details_navigation_scrolls_popup() {
        let mut menu = MenuState::account(AccountMenuInfo {
            alias: "account".into(),
            email: None,
            account_id: None,
            user_id: None,
            workspace_name: None,
            is_fedramp: false,
            plan_label: "Unknown".into(),
            plan_type: None,
            is_current: false,
            organizations: Vec::new(),
            auth_expiries: Vec::new(),
            usage: None,
            usage_meta: Vec::new(),
            models: Vec::new(),
            reset_cards: None,
            reset_card_expiries: Vec::new(),
            can_consume_reset_card: false,
        });

        assert!(matches!(menu.handle_key(KeyCode::Down), MenuAction::Noop));
        let MenuState::Account { popup, .. } = menu else {
            unreachable!();
        };
        assert_eq!(popup.scroll, 1);
    }

    #[test]
    fn quota_visuals_include_main_and_future_model_pools() {
        let now = crate::auth::now_unix_secs();
        let window = WindowUsage {
            used_percent: Some(80.0),
            resets_at: Some(now + 2 * 60 * 60),
            window_minutes: Some(300),
        };
        let usage = UsageInfo {
            primary: Some(window.clone()),
            additional_limits: vec![AdditionalRateLimit {
                limit_name: Some("GPT-6-Codex-Burst".to_string()),
                metered_feature: Some("codex_futureburst".to_string()),
                primary: Some(window),
                ..Default::default()
            }],
            ..Default::default()
        };
        let text = quota_lines(Some(&usage))
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Main"));
        assert!(text.contains("GPT-6-Codex-Burst"));
        assert!(text.contains('█'));
        assert!(text.contains("20% left"));
        assert!(text.contains("Pace"));
        assert!(text.contains("Reset"));
        assert!(text.contains("Rest"));
        assert!(text.contains("to pace"));
    }

    #[test]
    fn realistic_account_detail_keeps_models_in_the_right_column() {
        let now = crate::auth::now_unix_secs();
        let window = WindowUsage {
            used_percent: Some(50.0),
            resets_at: Some(now + 3_600),
            window_minutes: Some(300),
        };
        let usage = UsageInfo {
            primary: Some(window.clone()),
            secondary: Some(window.clone()),
            additional_limits: vec![AdditionalRateLimit {
                limit_name: Some("GPT-5.3-Codex-Spark".into()),
                primary: Some(window.clone()),
                secondary: Some(window),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut menu = MenuState::account(AccountMenuInfo {
            alias: "account".into(),
            email: Some("account@example.com".into()),
            account_id: Some("account-id".into()),
            user_id: Some("user-id".into()),
            workspace_name: Some("Night City".into()),
            is_fedramp: false,
            plan_label: "Pro 20×".into(),
            plan_type: Some("pro".into()),
            is_current: true,
            organizations: vec!["Night City · Owner · default workspace".into()],
            auth_expiries: vec![
                "ID token · proves account identity · expires soon".into(),
                "Access token · authorizes API requests · expires soon".into(),
            ],
            usage: Some(Box::new(usage)),
            usage_meta: vec!["  updated now".into()],
            models: vec!["  Official Model".into(), "    Official description".into()],
            reset_cards: Some(0),
            reset_card_expiries: Vec::new(),
            can_consume_reset_card: false,
        });
        let backend = TestBackend::new(160, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| menu.render(frame, frame.area()))
            .unwrap();

        let models = find_text(terminal.backend(), "Models").expect("models heading");
        assert!(models.0 > 80, "models should remain in the right column");
    }
}
