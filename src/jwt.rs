use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;

/// Single organization/workspace entry
#[derive(Debug, Default, Clone)]
pub struct OrgInfo {
    #[allow(dead_code)]
    pub id: String,
    pub title: String,
    #[allow(dead_code)]
    pub role: String,
    pub is_default: bool,
}

#[derive(Debug, Default, Clone)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub account_id: Option<String>,
    pub is_fedramp: bool,
    #[allow(dead_code)]
    pub user_id: Option<String>,
    pub workspace_name: Option<String>,
    pub organizations: Vec<OrgInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Free,
    Go,
    Plus,
    ProLite,
    Pro,
    Team,
    Business,
    Enterprise,
    Edu,
    Unknown,
}

impl PlanKind {
    pub fn from_wire(plan_type: Option<&str>) -> Self {
        match plan_type {
            Some("free") => Self::Free,
            Some("go") => Self::Go,
            Some("plus") => Self::Plus,
            Some("prolite") => Self::ProLite,
            Some("pro") => Self::Pro,
            Some("team") => Self::Team,
            Some("self_serve_business_usage_based" | "business") => Self::Business,
            Some("enterprise_cbp_usage_based" | "enterprise") => Self::Enterprise,
            Some("education" | "edu") => Self::Edu,
            _ => Self::Unknown,
        }
    }

    fn display_name(self, raw: Option<&str>) -> String {
        match self {
            Self::Free => "Free".to_string(),
            Self::Go => "Go".to_string(),
            Self::Plus => "Plus".to_string(),
            Self::ProLite => "Pro 5×".to_string(),
            Self::Pro => "Pro 20×".to_string(),
            Self::Team => "Team".to_string(),
            Self::Business => "Business".to_string(),
            Self::Enterprise => "Enterprise".to_string(),
            Self::Edu => "Edu".to_string(),
            Self::Unknown => raw.unwrap_or("?").to_string(),
        }
    }
}

impl AccountInfo {
    pub fn plan_label(&self) -> String {
        self.plan_label_with(self.plan_type.as_deref())
    }

    /// Same as `plan_label` but with an overridden plan type (e.g. from API response).
    pub fn plan_label_with(&self, plan_type: Option<&str>) -> String {
        let base = PlanKind::from_wire(plan_type).display_name(plan_type);
        let titled_organization_count = self
            .organizations
            .iter()
            .filter(|organization| !organization.title.is_empty())
            .count();
        let active_workspace_is_organization = self.workspace_name.as_ref().is_some_and(|name| {
            self.organizations
                .iter()
                .any(|organization| organization.title == *name)
        });
        let displayed_organization_count = if active_workspace_is_organization
            || self.workspace_name.is_none()
                && self
                    .organizations
                    .iter()
                    .any(|organization| organization.is_default && !organization.title.is_empty())
        {
            titled_organization_count.saturating_sub(1)
        } else {
            titled_organization_count
        };
        let organization_suffix = match displayed_organization_count {
            0 => String::new(),
            1 => " (+1 org)".to_string(),
            count => format!(" (+{count} orgs)"),
        };
        if let Some(name) = &self.workspace_name
            && !name.is_empty()
        {
            return format!("{base} - {name}{organization_suffix}");
        }
        if let Some(org) = self.organizations.iter().find(|o| o.is_default)
            && !org.title.is_empty()
        {
            return format!("{base} - {}{organization_suffix}", org.title);
        }
        base
    }

    pub fn is_free(&self) -> bool {
        matches!(self.plan_type.as_deref(), Some("free") | None)
    }

    pub fn is_team(&self) -> bool {
        matches!(self.plan_type.as_deref(), Some("team"))
            || !self.organizations.is_empty()
            || self.workspace_name.is_some()
    }
}

/// Parse account info from an auth.json Value
pub fn parse_account_info(auth: &Value) -> AccountInfo {
    let id_token = auth
        .pointer("/tokens/id_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let account_id_from_tokens = auth
        .pointer("/tokens/account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string());

    let claims = decode_jwt_payload(id_token).unwrap_or_default();

    // Root claim first, then the profile claim — matches Codex 0.144.1,
    // which falls back to https://api.openai.com/profile.email.
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| {
            claims
                .get("https://api.openai.com/profile")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());

    let auth_claims = claims.get("https://api.openai.com/auth");

    let plan_type = auth_claims
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let user_id = auth_claims
        .and_then(|a| a.get("chatgpt_user_id").or_else(|| a.get("user_id")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let account_id = auth_claims
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .or(account_id_from_tokens);

    let is_fedramp = auth_claims
        .and_then(|a| a.get("chatgpt_account_is_fedramp"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let workspace_name = extract_workspace_name(&claims);
    let organizations = extract_organizations(&claims);

    AccountInfo {
        email,
        plan_type,
        account_id,
        is_fedramp,
        user_id,
        workspace_name,
        organizations,
    }
}

/// Extract workspace name from JWT claims (team/org accounts)
fn extract_workspace_name(claims: &Value) -> Option<String> {
    // Top-level fields
    for key in &[
        "workspace_name",
        "organization_name",
        "org_name",
        "team_name",
    ] {
        if let Some(v) = claims.get(key).and_then(|v| v.as_str()) {
            let s = v.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    // Nested under auth claims
    let auth = claims.get("https://api.openai.com/auth")?;
    for key in &[
        "workspace_name",
        "organization_name",
        "org_name",
        "team_name",
    ] {
        if let Some(v) = auth.get(key).and_then(|v| v.as_str()) {
            let s = v.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    // Fallback: default org title from organizations array
    if let Some(orgs) = auth.get("organizations").and_then(|v| v.as_array()) {
        let default = orgs.iter().find(|o| {
            o.get("is_default")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });
        let candidate = default.or_else(|| orgs.first());
        if let Some(title) = candidate
            .and_then(|o| o.get("title"))
            .and_then(|v| v.as_str())
        {
            let s = title.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Extract organizations list from JWT claims
fn extract_organizations(claims: &Value) -> Vec<OrgInfo> {
    let auth = match claims.get("https://api.openai.com/auth") {
        Some(a) => a,
        None => return vec![],
    };
    let orgs = match auth.get("organizations").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return vec![],
    };
    orgs.iter()
        .filter_map(|o| {
            let id = o.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(OrgInfo {
                id,
                title: o
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                role: o
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                is_default: o
                    .get("is_default")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Decode the payload section of a JWT token (base64 → JSON)
fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Return the `exp` claim (unix timestamp) from a JWT, or `None` if missing.
pub fn token_expires_at(token: &str) -> Option<i64> {
    let payload = decode_jwt_payload(token)?;
    payload.get("exp")?.as_i64()
}

/// Check if a JWT token is expired or will expire within `margin_secs`.
/// Returns `true` if expired/expiring, `false` if still valid, `None` if exp claim is missing.
pub fn is_token_expiring(token: &str, margin_secs: i64) -> Option<bool> {
    let payload = decode_jwt_payload(token)?;
    let exp = payload.get("exp")?.as_i64()?;
    let now = crate::auth::now_unix_secs();
    Some(now + margin_secs >= exp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_jwt(claims: &serde_json::Value) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        format!("header.{payload}.signature")
    }

    #[test]
    fn test_parse_account_info_extracts_email_and_plan() {
        let auth = json!({
            "tokens": {
                "id_token": make_jwt(&json!({
                    "email": "user@example.com",
                    "https://api.openai.com/auth": {
                        "chatgpt_plan_type": "pro",
                        "chatgpt_account_id": "acct-from-claim",
                    }
                })),
                "account_id": "acct-from-tokens"
            }
        });

        let info = parse_account_info(&auth);

        assert_eq!(info.email.as_deref(), Some("user@example.com"));
        assert_eq!(info.plan_type.as_deref(), Some("pro"));
        assert_eq!(info.account_id.as_deref(), Some("acct-from-claim"));
    }

    #[test]
    fn test_parse_account_info_email_falls_back_to_profile_claim() {
        // Codex 0.144.1 reads email from the root claim, then falls back to
        // the https://api.openai.com/profile claim — some id_tokens only
        // carry the latter.
        let auth = json!({
            "tokens": {
                "id_token": make_jwt(&json!({
                    "https://api.openai.com/profile": {
                        "email": "workspace-user@example.com"
                    },
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "acct-1"
                    }
                }))
            }
        });

        let info = parse_account_info(&auth);

        assert_eq!(info.email.as_deref(), Some("workspace-user@example.com"));
    }

    #[test]
    fn test_parse_account_info_extracts_fedramp_routing_claim() {
        let auth = json!({
            "tokens": {
                "id_token": make_jwt(&json!({
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "acct-fedramp",
                        "chatgpt_account_is_fedramp": true,
                    }
                }))
            }
        });

        let info = parse_account_info(&auth);

        assert!(info.is_fedramp);
    }

    #[test]
    fn test_parse_account_info_empty_token() {
        let auth = json!({
            "tokens": {
                "id_token": ""
            }
        });

        let info = parse_account_info(&auth);

        assert!(info.email.is_none());
        assert!(info.plan_type.is_none());
        assert!(info.account_id.is_none());
        assert!(info.user_id.is_none());
        assert!(info.workspace_name.is_none());
        assert!(info.organizations.is_empty());
    }

    #[test]
    fn plan_label_shows_active_team_and_additional_organizations() {
        let info = AccountInfo {
            plan_type: Some("team".to_string()),
            workspace_name: Some("Platform Team".to_string()),
            organizations: vec![
                OrgInfo {
                    id: "org-platform".to_string(),
                    title: "Platform Team".to_string(),
                    role: "owner".to_string(),
                    is_default: true,
                },
                OrgInfo {
                    id: "org-research".to_string(),
                    title: "Research Team".to_string(),
                    role: "member".to_string(),
                    is_default: false,
                },
            ],
            ..Default::default()
        };

        assert_eq!(info.plan_label(), "Team - Platform Team (+1 org)");
    }

    #[test]
    fn plan_label_counts_every_org_when_workspace_is_not_an_org_title() {
        let info = AccountInfo {
            plan_type: Some("team".to_string()),
            workspace_name: Some("Personal".to_string()),
            organizations: vec![
                OrgInfo {
                    id: "org-platform".to_string(),
                    title: "Platform Team".to_string(),
                    role: "owner".to_string(),
                    is_default: true,
                },
                OrgInfo {
                    id: "org-research".to_string(),
                    title: "Research Team".to_string(),
                    role: "member".to_string(),
                    is_default: false,
                },
            ],
            ..Default::default()
        };

        assert_eq!(info.plan_label(), "Team - Personal (+2 orgs)");
    }

    #[test]
    fn plan_label_normalizes_consumer_plan_names() {
        let mut info = AccountInfo::default();

        for (wire, expected) in [
            ("free", "Free"),
            ("go", "Go"),
            ("plus", "Plus"),
            ("prolite", "Pro 5×"),
            ("pro", "Pro 20×"),
        ] {
            info.plan_type = Some(wire.to_string());
            assert_eq!(info.plan_label(), expected);
        }
    }

    #[test]
    fn plan_label_normalizes_workspace_plan_names_and_preserves_unknown_values() {
        let info = AccountInfo {
            workspace_name: Some("Example Workspace".to_string()),
            ..Default::default()
        };

        for (wire, expected) in [
            ("team", "Team - Example Workspace"),
            (
                "self_serve_business_usage_based",
                "Business - Example Workspace",
            ),
            ("business", "Business - Example Workspace"),
            (
                "enterprise_cbp_usage_based",
                "Enterprise - Example Workspace",
            ),
            ("enterprise", "Enterprise - Example Workspace"),
            ("education", "Edu - Example Workspace"),
            ("edu", "Edu - Example Workspace"),
            ("future_plan", "future_plan - Example Workspace"),
        ] {
            assert_eq!(info.plan_label_with(Some(wire)), expected);
        }
    }

    #[test]
    fn test_is_token_expiring_expired() {
        let token = make_jwt(&json!({ "exp": 0 }));

        assert_eq!(is_token_expiring(&token, 0), Some(true));
    }

    #[test]
    fn test_is_token_expiring_valid() {
        let token = make_jwt(&json!({ "exp": 9_999_999_999_i64 }));

        assert_eq!(is_token_expiring(&token, 60), Some(false));
    }

    #[test]
    fn test_is_token_expiring_within_margin() {
        let token = make_jwt(&json!({
            "exp": crate::auth::now_unix_secs() + 30
        }));

        assert_eq!(is_token_expiring(&token, 60), Some(true));
    }

    #[test]
    fn test_is_token_expiring_no_exp_claim() {
        let token = make_jwt(&json!({ "sub": "user-123" }));

        assert_eq!(is_token_expiring(&token, 60), None);
    }

    #[test]
    fn test_is_token_expiring_invalid_jwt() {
        assert_eq!(is_token_expiring("not-a-jwt", 60), None);
    }
}
