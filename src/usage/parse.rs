use anyhow::Result;
use serde_json::Value;
use tracing::{debug, warn};

use crate::auth;

use super::reset_credits::parse_reset_credits_summary;
use super::{AdditionalRateLimit, UsageInfo, WindowUsage};

pub(super) fn parse_optional_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_window(val: &Value) -> Option<WindowUsage> {
    // Require used_percent to be present for meaningful scoring data.
    // A window with only resets_at but no used_percent would cause
    // has_5h_data=true with used_5h=0.0, incorrectly treating it as "fully available".
    let used_percent = val.get("used_percent").and_then(|v| v.as_f64());
    used_percent?;
    let resets_at = val.get("reset_at").and_then(|v| v.as_i64());

    Some(WindowUsage {
        used_percent,
        resets_at,
    })
}

/// Parse `additional_rate_limits[]`. Malformed entries (missing/non-object
/// `rate_limit`) are skipped rather than failing the whole parse.
fn parse_additional_rate_limits(body: &Value) -> Vec<AdditionalRateLimit> {
    let Some(items) = body.get("additional_rate_limits").and_then(Value::as_array) else {
        return vec![];
    };

    items
        .iter()
        .filter_map(|item| {
            let rate_limit = item.get("rate_limit")?;
            if !rate_limit.is_object() {
                return None;
            }
            let primary = rate_limit
                .get("primary_window")
                .filter(|v| !v.is_null())
                .and_then(parse_window);
            let secondary = rate_limit
                .get("secondary_window")
                .filter(|v| !v.is_null())
                .and_then(parse_window);
            Some(AdditionalRateLimit {
                limit_name: item
                    .get("limit_name")
                    .and_then(Value::as_str)
                    .map(String::from),
                metered_feature: item
                    .get("metered_feature")
                    .and_then(Value::as_str)
                    .map(String::from),
                allowed: rate_limit.get("allowed").and_then(Value::as_bool),
                limit_reached: rate_limit.get("limit_reached").and_then(Value::as_bool),
                primary,
                secondary,
            })
        })
        .collect()
}

fn known_rate_limit_reached_type(body: &Value) -> bool {
    let kind = body.get("rate_limit_reached_type").and_then(|value| {
        value
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| value.as_str())
    });
    matches!(
        kind,
        Some(
            "rate_limit_reached"
                | "workspace_owner_credits_depleted"
                | "workspace_member_credits_depleted"
                | "workspace_owner_usage_limit_reached"
                | "workspace_member_usage_limit_reached"
        )
    )
}

pub(super) fn parse_usage_checked(body: &Value) -> Result<UsageInfo> {
    let usage = parse_usage(body);
    let credits_has_data = body
        .get("credits")
        .and_then(Value::as_object)
        .is_some_and(|credits| {
            credits
                .get("has_credits")
                .and_then(Value::as_bool)
                .is_some()
                || credits.get("unlimited").and_then(Value::as_bool).is_some()
                || credits.get("balance").is_some_and(|balance| {
                    balance.as_f64().is_some()
                        || balance
                            .as_str()
                            .is_some_and(|value| value.parse::<f64>().is_ok())
                })
        });
    if usage.primary.is_none()
        && usage.secondary.is_none()
        && !usage.account_limited
        && !credits_has_data
    {
        anyhow::bail!("usage response missing recognized quota fields");
    }
    Ok(usage)
}

pub fn parse_usage(body: &Value) -> UsageInfo {
    const SECS_7D: i64 = 7 * 86400; // 604800

    let primary_raw = body
        .pointer("/rate_limit/primary_window")
        .filter(|v| !v.is_null());

    let secondary_raw = body
        .pointer("/rate_limit/secondary_window")
        .filter(|v| !v.is_null());

    let primary_window_secs = primary_raw
        .and_then(|v| v.get("limit_window_seconds"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let primary_parsed = primary_raw.and_then(parse_window);
    let secondary_parsed = secondary_raw.and_then(parse_window);

    // Free accounts (new API): only one window exists, placed in primary_window slot
    // with limit_window_seconds == 604800 (7d). Remap it to secondary so scoring works.
    let (primary, secondary) = if primary_window_secs >= SECS_7D && secondary_parsed.is_none() {
        debug!("parse_usage: primary_window is 7d (free account) — remapping to secondary");
        (None, primary_parsed)
    } else {
        if secondary_raw.is_some() && secondary_parsed.is_none() {
            warn!(
                "parse_usage: secondary_window present but failed to parse (missing used_percent?): {:?}",
                secondary_raw
            );
        }
        (primary_parsed, secondary_parsed)
    };

    debug!(
        "parse_usage: primary={} secondary={}",
        primary.is_some(),
        secondary.is_some()
    );

    // has_credits=false means no pay-per-use credits (Plus/Pro included usage only).
    // Default true for old API format which lacked this field.
    let has_credits = body
        .pointer("/credits/has_credits")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // balance changed from number to string "0" in new API — handle both.
    // Skip entirely when has_credits=false to avoid showing "$0.00" for accounts
    // that simply don't use the pay-per-use credits system.
    let credits_balance = if has_credits {
        body.pointer("/credits/balance").and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    } else {
        None
    };

    let unlimited_credits = body.pointer("/credits/unlimited").and_then(|v| v.as_bool());

    let plan_type = body
        .get("plan_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let account_limited = known_rate_limit_reached_type(body)
        || body
            .pointer("/spend_control/reached")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || body
            .pointer("/rate_limit/limit_reached")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let reset_credits_raw = body
        .get("rate_limit_reset_credits")
        .or_else(|| body.get("rateLimitResetCredits"));
    let (reset_credits_available_count, reset_credits, _) = reset_credits_raw
        .map(parse_reset_credits_summary)
        .unwrap_or((None, vec![], false));

    let additional_limits = parse_additional_rate_limits(body);

    UsageInfo {
        fetched_at: Some(auth::now_unix_secs()),
        primary,
        secondary,
        credits_balance,
        unlimited_credits,
        plan_type,
        reset_credits_available_count,
        reset_credits,
        reset_credits_error: None,
        account_limited,
        additional_limits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use serde_json::json;

    #[test]
    fn test_parse_usage_full_response() {
        let primary_reset = DateTime::parse_from_rfc3339("2026-03-26T10:00:00Z")
            .unwrap()
            .timestamp();
        let secondary_reset = DateTime::parse_from_rfc3339("2026-03-30T00:00:00Z")
            .unwrap()
            .timestamp();
        let body = json!({
            "rate_limit": {
                "primary_window": {
                    "remaining_seconds": 3600,
                    "requests_remaining": 50,
                    "requests_limit": 100,
                    "reset_time": "2026-03-26T10:00:00Z",
                    "used_percent": 50.0,
                    "reset_at": primary_reset
                },
                "secondary_window": {
                    "remaining_seconds": 86400,
                    "requests_remaining": 200,
                    "requests_limit": 500,
                    "reset_time": "2026-03-30T00:00:00Z",
                    "used_percent": 60.0,
                    "reset_at": secondary_reset
                }
            },
            "credits": {
                "balance": 15.50,
                "unlimited": false
            },
            "rate_limit_reset_credits": {
                "available_count": "2"
            }
        });

        let before = auth::now_unix_secs();
        let usage = parse_usage(&body);
        let after = auth::now_unix_secs();

        assert!(matches!(usage.fetched_at, Some(ts) if ts >= before && ts <= after));
        assert_eq!(
            usage.primary.as_ref().and_then(|w| w.used_percent),
            Some(50.0)
        );
        assert_eq!(
            usage.primary.as_ref().and_then(|w| w.resets_at),
            Some(primary_reset)
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.used_percent),
            Some(60.0)
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.resets_at),
            Some(secondary_reset)
        );
        assert_eq!(usage.credits_balance, Some(15.5));
        assert_eq!(usage.unlimited_credits, Some(false));
        assert_eq!(usage.reset_credits_available_count, Some(2));
    }

    #[test]
    fn test_parse_usage_reset_credit_details() {
        let usage = parse_usage(&json!({
            "rate_limit_reset_credits": {
                "available_count": 2,
                "credits": [
                    {
                        "id": "cred_1",
                        "reset_type": "codex_rate_limits",
                        "status": "available",
                        "granted_at": "2026-07-01T00:00:00Z",
                        "expires_at": "2026-07-08T00:00:00Z"
                    },
                    {
                        "id": "cred_2",
                        "reset_type": "codex_rate_limits",
                        "status": "consumed",
                        "expires_at": "2026-07-08T00:00:00Z"
                    }
                ]
            }
        }));

        assert_eq!(usage.reset_credits_available_count, Some(2));
        assert_eq!(usage.reset_credits.len(), 1);
        assert_eq!(usage.reset_credits[0].id, "cred_1");
        assert_eq!(
            usage.reset_credits[0].expires_at.as_deref(),
            Some("2026-07-08T00:00:00Z")
        );
    }

    #[test]
    fn test_parse_usage_unlimited_credits() {
        let usage = parse_usage(&json!({
            "credits": {
                "balance": 15.50,
                "unlimited": true
            }
        }));

        assert_eq!(usage.credits_balance, Some(15.5));
        assert_eq!(usage.unlimited_credits, Some(true));
    }

    #[test]
    fn test_parse_usage_no_credits() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25.0,
                    "reset_at": 123
                }
            }
        }));

        assert_eq!(usage.credits_balance, None);
        assert_eq!(usage.unlimited_credits, None);
    }

    #[test]
    fn test_parse_usage_has_credits_false_hides_balance() {
        // New API: plus accounts return has_credits=false with balance="0" (string).
        // We must NOT show $0.00 for these accounts.
        let usage = parse_usage(&json!({
            "plan_type": "plus",
            "credits": {
                "has_credits": false,
                "unlimited": false,
                "balance": "0"
            }
        }));

        assert_eq!(
            usage.credits_balance, None,
            "has_credits=false must suppress balance"
        );
        assert_eq!(usage.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn test_parse_usage_balance_string() {
        // New API: balance is a string when has_credits=true
        let usage = parse_usage(&json!({
            "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": "5.25"
            }
        }));

        assert_eq!(usage.credits_balance, Some(5.25));
    }

    #[test]
    fn test_parse_usage_free_account_single_window() {
        // New API: free accounts have one 7d window in primary_window slot.
        // Must be remapped to secondary so scoring treats it as 7d data.
        let usage = parse_usage(&json!({
            "plan_type": "free",
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {
                    "used_percent": 100,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 437896,
                    "reset_at": 1778468889i64
                },
                "secondary_window": null
            }
        }));

        assert!(
            usage.primary.is_none(),
            "free account must have no 5h window"
        );
        assert!(
            usage.secondary.is_some(),
            "free account 7d data must be in secondary"
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.used_percent),
            Some(100.0)
        );
        assert_eq!(usage.plan_type.as_deref(), Some("free"));
    }

    #[test]
    fn test_parse_usage_null_windows() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "primary_window": null,
                "secondary_window": null
            }
        }));

        assert!(usage.primary.is_none());
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn test_parse_usage_empty_response() {
        let usage = parse_usage(&json!({}));

        assert!(usage.primary.is_none());
        assert!(usage.secondary.is_none());
        assert_eq!(usage.credits_balance, None);
        assert_eq!(usage.unlimited_credits, None);
    }

    #[test]
    fn test_checked_usage_rejects_empty_or_drifted_response() {
        assert!(parse_usage_checked(&json!({})).is_err());
        assert!(parse_usage_checked(&json!({"unexpected": true})).is_err());
        assert!(parse_usage_checked(&json!({"credits": {"balance": null}})).is_err());
    }

    #[test]
    fn test_checked_usage_empty_object_error_message_names_missing_fields() {
        let err = parse_usage_checked(&json!({})).expect_err("empty body must be rejected");
        assert!(
            err.to_string().contains("missing recognized quota fields"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_checked_usage_drifted_response_is_rejected() {
        let err = parse_usage_checked(&json!({
            "some_new_field": "unrecognized",
            "another": { "nested": 1 }
        }))
        .expect_err("structurally drifted body must be rejected");
        assert!(
            err.to_string().contains("missing recognized quota fields"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_parse_usage_marks_known_rate_limit_reached_type_as_limited() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 10.0}
            },
            "rate_limit_reached_type": {
                "type": "workspace_member_usage_limit_reached"
            }
        }));

        assert!(usage.account_limited);
        assert!(!crate::usage::is_available(&usage));
    }

    #[test]
    fn test_parse_usage_marks_reached_spend_control_as_limited() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 10.0}
            },
            "spend_control": {"reached": true}
        }));

        assert!(usage.account_limited);
        assert!(!crate::usage::is_available(&usage));
    }

    #[test]
    fn test_parse_usage_additional_rate_limits_parsed_alongside_top_level_window() {
        // Real production shape (Pro 20x account, sanitized). Top-level 42%/84%
        // plus an additional_rate_limits item with its own independent windows.
        // A sibling `code_review_rate_limit` key (observed null) must not break parsing.
        let body = json!({
            "rate_limit": {
                "primary_window": {"used_percent": 42.0, "reset_at": 1000},
                "secondary_window": {"used_percent": 84.0, "reset_at": 2000}
            },
            "code_review_rate_limit": null,
            "additional_rate_limits": [
                {
                    "limit_name": "GPT-5.3-Codex-Spark",
                    "metered_feature": "codex_bengalfox",
                    "rate_limit": {
                        "allowed": true,
                        "limit_reached": false,
                        "primary_window": {
                            "used_percent": 0,
                            "limit_window_seconds": 18000,
                            "reset_after_seconds": 18000,
                            "reset_at": 1783843614i64
                        },
                        "secondary_window": {
                            "used_percent": 0,
                            "limit_window_seconds": 604800,
                            "reset_after_seconds": 604800,
                            "reset_at": 1784430414i64
                        }
                    }
                }
            ]
        });

        let usage = parse_usage(&body);

        // Top-level primary window unaffected by additional_rate_limits presence.
        assert_eq!(
            usage.primary.as_ref().and_then(|w| w.used_percent),
            Some(42.0)
        );
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.used_percent),
            Some(84.0)
        );

        assert_eq!(usage.additional_limits.len(), 1);
        let extra = &usage.additional_limits[0];
        assert_eq!(extra.metered_feature.as_deref(), Some("codex_bengalfox"));
        assert_eq!(extra.limit_name.as_deref(), Some("GPT-5.3-Codex-Spark"));
        assert_eq!(extra.allowed, Some(true));
        assert_eq!(extra.limit_reached, Some(false));
        assert_eq!(
            extra.primary.as_ref().and_then(|w| w.used_percent),
            Some(0.0)
        );
        assert_eq!(
            extra.primary.as_ref().and_then(|w| w.resets_at),
            Some(1783843614i64)
        );
        assert_eq!(
            extra.secondary.as_ref().and_then(|w| w.used_percent),
            Some(0.0)
        );
        assert_eq!(
            extra.secondary.as_ref().and_then(|w| w.resets_at),
            Some(1784430414i64)
        );
    }

    #[test]
    fn test_parse_usage_additional_rate_limits_missing_is_empty() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "primary_window": {"used_percent": 10.0}
            }
        }));
        assert!(usage.additional_limits.is_empty());
    }

    #[test]
    fn test_parse_usage_additional_rate_limits_empty_array_is_empty() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "primary_window": {"used_percent": 10.0}
            },
            "additional_rate_limits": []
        }));
        assert!(usage.additional_limits.is_empty());
    }

    #[test]
    fn test_parse_usage_additional_rate_limits_skips_malformed_entries() {
        let usage = parse_usage(&json!({
            "additional_rate_limits": [
                {"limit_name": "missing_rate_limit", "metered_feature": "codex_other"},
                {"limit_name": "bad_shape", "rate_limit": "not-an-object"},
                {
                    "limit_name": "ok_one",
                    "metered_feature": "codex_ok",
                    "rate_limit": {
                        "primary_window": {"used_percent": 33.0}
                    }
                }
            ]
        }));

        assert_eq!(usage.additional_limits.len(), 1);
        assert_eq!(
            usage.additional_limits[0].metered_feature.as_deref(),
            Some("codex_ok")
        );
    }

    #[test]
    fn test_parse_usage_ignores_unknown_limit_reason() {
        let usage = parse_usage(&json!({
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 10.0}
            },
            "rate_limit_reached_type": {"type": "future_reason"},
            "spend_control": {"reached": false}
        }));

        assert!(!usage.account_limited);
        assert!(crate::usage::is_available(&usage));
    }
}
