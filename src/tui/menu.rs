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

use super::popup::{PopupState, render_popup};

const C_WHITE: Color = Color::Rgb(240, 240, 240);
const DIM: Color = Color::Rgb(120, 120, 120);
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
    pub usage_meta: Vec<String>,
    pub quota_pools: Vec<String>,
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
                let mut lines: Vec<Line<'static>> = Vec::new();
                let active = if info.is_current { "  active" } else { "" };
                lines.push(Line::from(Span::styled(
                    format!("{}{}", info.alias, active),
                    header_style,
                )));
                if let Some(email) = &info.email {
                    lines.push(Line::from(vec![
                        Span::styled("email  ", dim),
                        Span::styled(email.clone(), label_style),
                    ]));
                }
                lines.push(Line::from(vec![
                    Span::styled("plan   ", dim),
                    Span::styled(info.plan_label.clone(), label_style),
                ]));
                if let Some(account_id) = &info.account_id {
                    lines.push(Line::from(vec![
                        Span::styled("id     ", dim),
                        Span::styled(account_id.clone(), label_style),
                    ]));
                }
                if let Some(user_id) = &info.user_id {
                    lines.push(Line::from(vec![
                        Span::styled("user   ", dim),
                        Span::styled(user_id.clone(), label_style),
                    ]));
                }
                if let Some(workspace) = &info.workspace_name {
                    lines.push(Line::from(vec![
                        Span::styled("space  ", dim),
                        Span::styled(workspace.clone(), label_style),
                    ]));
                }
                if let Some(plan_type) = &info.plan_type {
                    lines.push(Line::from(vec![
                        Span::styled("type   ", dim),
                        Span::styled(plan_type.clone(), label_style),
                    ]));
                }
                if info.is_fedramp {
                    lines.push(Line::from(vec![
                        Span::styled("route  ", dim),
                        Span::styled("FedRAMP", label_style),
                    ]));
                }
                for organization in &info.organizations {
                    lines.push(Line::from(vec![
                        Span::styled("org    ", dim),
                        Span::styled(organization.clone(), label_style),
                    ]));
                }
                for expiry in &info.auth_expiries {
                    lines.push(Line::from(vec![
                        Span::styled("expiry ", dim),
                        Span::styled(expiry.clone(), label_style),
                    ]));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Quota", header_style)));
                for item in &info.usage_meta {
                    lines.push(Line::from(Span::styled(item.clone(), dim)));
                }
                for pool in &info.quota_pools {
                    lines.push(Line::from(Span::styled(pool.clone(), label_style)));
                }
                let cards = info
                    .reset_cards
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "--".to_string());
                lines.push(Line::from(vec![
                    Span::styled("cards  ", dim),
                    Span::styled(cards, label_style),
                ]));
                for (idx, expiry) in info.reset_card_expiries.iter().enumerate() {
                    let note = if idx == 0 { "  next" } else { "" };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  #{}  ", idx + 1), dim),
                        Span::styled(expiry.clone(), label_style),
                        Span::styled(note, dim),
                    ]));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Models", header_style)));
                for model in &info.models {
                    lines.push(Line::from(Span::styled(model.clone(), label_style)));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Primary", header_style)));
                lines.extend(menu_items(
                    &[("u", "Use (switch to)")],
                    key_style,
                    label_style,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Quota actions", header_style)));
                lines.extend(menu_items_stateful(
                    &[
                        ("r", "Refresh account details", true),
                        (
                            "c",
                            "Confirm earliest reset card",
                            info.can_consume_reset_card,
                        ),
                        ("w", "Warmup", true),
                    ],
                    key_style,
                    label_style,
                    dim,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Auth", header_style)));
                lines.extend(menu_items(&[("l", "re-Login")], key_style, label_style));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("Manage", header_style)));
                lines.extend(menu_items(
                    &[("n", "reName"), ("d", "Delete")],
                    key_style,
                    label_style,
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "j k / arrows / PgUp PgDn to scroll · esc / q to cancel",
                    dim,
                )));
                render_popup(f, title, &lines, popup, area);
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

fn menu_items_stateful(
    items: &[(&str, &str, bool)],
    key_style: Style,
    label_style: Style,
    disabled_style: Style,
) -> Vec<Line<'static>> {
    let key_w = items
        .iter()
        .map(|(k, _, _)| k.chars().count())
        .max()
        .unwrap_or(1);
    items
        .iter()
        .map(|(k, label, enabled)| {
            let pad = key_w.saturating_sub(k.chars().count());
            let style = if *enabled {
                label_style
            } else {
                disabled_style
            };
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    (*k).to_string(),
                    if *enabled { key_style } else { disabled_style },
                ),
                Span::raw(" ".repeat(pad)),
                Span::raw("  "),
                Span::styled((*label).to_string(), style),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::{AccountMenuInfo, MenuAction, MenuState};

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
            usage_meta: Vec::new(),
            quota_pools: Vec::new(),
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
}
