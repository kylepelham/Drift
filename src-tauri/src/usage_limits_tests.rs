use super::*;
use serde_json::json;

const NOW: i64 = 1_790_614_800_000;

fn windows(source: Source, body: Value) -> Vec<(WindowKind, Option<String>, f64, Option<i64>)> {
    parse(source, &body, NOW).windows.into_iter().map(|w| (w.kind, w.label, w.used_percent, w.resets_at)).collect()
}

#[test]
fn engine_provider_ids_map_to_their_usage_sources() {
    assert_eq!(source("anthropic"), Some(Source::Anthropic));
    assert_eq!(source("openai"), Some(Source::Codex));
    assert_eq!(source("zai-coding-plan"), Some(Source::Zai("https://api.z.ai/api/monitor/usage/quota/limit")));
    assert!(matches!(source("zhipuai-coding-plan"), Some(Source::Zai(url)) if url.contains("bigmodel.cn")));
    assert_eq!(source("kimi-code-plan-global"), Some(Source::Kimi));
    assert_eq!(source("openrouter"), None);
}

#[test]
fn subscription_endpoints_require_the_matching_credential_kind() {
    let oauth = Credential::OAuth { access: "tok".into(), expires: 0, account_id: Some("acct".into()), enterprise: false };
    let api = Credential::Api { key: "key".into() };
    assert!(request(Source::Anthropic, &api).is_none(), "an Anthropic API key has no plan windows");
    assert!(request(Source::Zai("u"), &oauth).is_none());
    let codex = request(Source::Codex, &oauth).unwrap();
    assert!(codex.headers.contains(&("ChatGPT-Account-Id", "acct".into())));
    assert!(codex.headers.contains(&("Authorization", "Bearer tok".into())));
    let anthropic = request(Source::Anthropic, &oauth).unwrap();
    assert!(anthropic.headers.contains(&("anthropic-beta", "oauth-2025-04-20".into())));
    let copilot = request(Source::Copilot, &oauth).unwrap();
    assert!(copilot.headers.contains(&("Authorization", "token tok".into())));
    let enterprise = Credential::OAuth { access: "tok".into(), expires: 0, account_id: None, enterprise: true };
    assert!(request(Source::Copilot, &enterprise).is_none(), "enterprise Copilot uses a different host");
}

#[test]
fn credentials_come_from_the_engines_store() {
    use drift_engine::llm::Credential as Engine;
    let signed_in = Engine::OAuth { access: "a".into(), refresh: "r".into(), expires_at: 5, account: Some("id".into()) };
    assert_eq!(from_engine(signed_in), Some(Credential::OAuth { access: "a".into(), expires: 5, account_id: Some("id".into()), enterprise: false }));
    assert_eq!(from_engine(Engine::ApiKey { key: "k".into() }), Some(Credential::Api { key: "k".into() }));
    assert_eq!(from_engine(Engine::Ambient { source: "profile".into() }), None, "a cloud route has no plan to report");
}

#[test]
fn anthropic_prefers_the_limits_list_and_keeps_only_active_model_caps() {
    let body = json!({
        "five_hour": { "utilization": 4.0, "resets_at": "2026-09-28T21:10:00.009134+00:00" },
        "seven_day": { "utilization": 15.0, "resets_at": "2026-10-03T19:00:00.009160+00:00" },
        "limits": [
            { "kind": "session", "group": "session", "percent": 4, "resets_at": "2026-09-28T21:10:00.009134+00:00", "scope": null },
            { "kind": "weekly_all", "group": "weekly", "percent": 15, "resets_at": "2026-10-03T19:00:00.009160+00:00", "scope": null },
            { "kind": "weekly_scoped", "group": "weekly", "percent": 0, "resets_at": "2026-10-03T19:00:00+00:00",
              "scope": { "model": { "id": null, "display_name": "Fable" } } },
            { "kind": "weekly_scoped", "group": "weekly", "percent": 40, "resets_at": "2026-10-03T19:00:00+00:00",
              "scope": { "model": { "id": null, "display_name": "Opus" } } }
        ]
    });
    assert_eq!(
        windows(Source::Anthropic, body),
        vec![
            (WindowKind::Session, None, 4.0, Some(1_790_629_800_009)),
            (WindowKind::Weekly, None, 15.0, Some(1_791_054_000_009)),
            (WindowKind::Weekly, Some("Opus".into()), 40.0, Some(1_791_054_000_000)),
        ]
    );
}

#[test]
fn anthropic_falls_back_to_the_flat_windows() {
    let body = json!({
        "five_hour": { "utilization": 91.0, "resets_at": "2026-09-28T21:10:00+00:00" },
        "seven_day": { "utilization": 100.0, "resets_at": null }
    });
    assert_eq!(
        windows(Source::Anthropic, body),
        vec![(WindowKind::Session, None, 91.0, Some(1_790_629_800_000)), (WindowKind::Weekly, None, 100.0, None)]
    );
}

#[test]
fn codex_classifies_windows_by_duration_not_position() {
    let body = json!({
        "plan_type": "prolite",
        "rate_limit": {
            "primary_window": { "used_percent": 1, "limit_window_seconds": 604800, "reset_after_seconds": 602897, "reset_at": 1791217619 },
            "secondary_window": null
        }
    });
    let usage = parse(Source::Codex, &body, NOW);
    assert_eq!(usage.plan.as_deref(), Some("prolite"));
    assert_eq!(windows(Source::Codex, body), vec![(WindowKind::Weekly, None, 1.0, Some(1_791_217_619_000))]);
    let both = json!({ "rate_limit": {
        "primary_window": { "used_percent": 91, "limit_window_seconds": 18000, "reset_at": 1790617740 },
        "secondary_window": { "used_percent": 100, "limit_window_seconds": 604800, "reset_at": 1791000000 }
    } });
    let kinds: Vec<_> = windows(Source::Codex, both).into_iter().map(|w| w.0).collect();
    assert_eq!(kinds, vec![WindowKind::Session, WindowKind::Weekly]);
}

#[test]
fn zai_reads_token_windows_and_skips_the_tool_quota() {
    let body = json!({ "code": 200, "data": { "level": "max", "limits": [
        { "type": "TIME_LIMIT", "unit": 5, "number": 1, "usage": 4000, "percentage": 0, "nextResetTime": 1791122979999u64 },
        { "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 12 },
        { "type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 30, "nextResetTime": 1791036579998u64 }
    ] }, "success": true });
    assert_eq!(parse(Source::Zai("u"), &body, NOW).plan.as_deref(), Some("max"));
    assert_eq!(
        windows(Source::Zai("u"), body),
        vec![(WindowKind::Session, None, 12.0, None), (WindowKind::Weekly, None, 30.0, Some(1_791_036_579_998))]
    );
}

#[test]
fn opencode_go_resolves_relative_resets_against_now() {
    let body = json!({ "usage": {
        "rolling": { "usagePercent": 17, "resetInSec": 5944 },
        "weekly": { "usagePercent": 75, "resetAt": "2026-10-01T00:00:00Z" },
        "monthly": { "usagePercent": 91, "resetInSec": 60 }
    } });
    assert_eq!(
        windows(Source::OpenCodeGo, body),
        vec![
            (WindowKind::Session, None, 17.0, Some(NOW + 5_944_000)),
            (WindowKind::Weekly, None, 75.0, Some(1_790_812_800_000)),
            (WindowKind::Monthly, None, 91.0, Some(NOW + 60_000)),
        ]
    );
}

#[test]
fn grok_prefers_the_reported_percent_and_the_current_period() {
    let body = json!({ "config": {
        "creditUsagePercent": 12.5,
        "subscriptionTier": "SuperGrok",
        "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY", "start": "2026-08-06T00:00:00Z", "end": "2026-08-13T00:00:00Z" },
        "billingPeriodEnd": "2026-09-01T00:00:00Z",
        "onDemandCap": { "val": 1000 },
        "onDemandUsed": { "val": 250 }
    } });
    assert_eq!(parse(Source::Grok, &body, NOW).plan.as_deref(), Some("SuperGrok"));
    assert_eq!(windows(Source::Grok, body), vec![(WindowKind::Weekly, None, 12.5, Some(1_786_579_200_000))]);
    let derived = json!({ "config": { "onDemandCap": { "val": 1000 }, "onDemandUsed": { "val": 250 } } });
    assert_eq!(windows(Source::Grok, derived), vec![(WindowKind::Period, None, 25.0, None)]);
}

#[test]
fn kimi_trusts_counters_over_zeroed_ratio_pools() {
    let body = json!({
        "usage": { "limit": "100", "used": "19", "remaining": "81", "resetTime": "2026-09-19T16:45:59Z" },
        "limits": [{ "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                     "detail": { "limit": "100", "remaining": "99", "resetTime": "2026-09-19T14:45:59Z" } }],
        "usages": {
            "limit_5h": { "used_ratio": 0, "reset_time": "2026-09-19T14:45:58Z" },
            "limit_7d": { "used_ratio": 0, "reset_time": "2026-09-19T16:45:58Z" },
            "limit_month_total": { "used_ratio": 0.5, "reset_time": "2026-10-01T00:00:00Z" }
        }
    });
    assert_eq!(
        windows(Source::Kimi, body),
        vec![
            (WindowKind::Session, None, 1.0, Some(1_789_829_159_000)),
            (WindowKind::Weekly, None, 19.0, Some(1_789_836_359_000)),
            (WindowKind::Monthly, None, 50.0, Some(1_790_812_800_000)),
        ]
    );
}

#[test]
fn copilot_reports_used_quota_and_skips_unlimited_lanes() {
    let body = json!({
        "copilot_plan": "individual",
        "quota_reset_date": "2026-07-01",
        "quota_snapshots": {
            "premium_interactions": { "entitlement": 500, "remaining": 125, "percent_remaining": 25 },
            "chat": { "entitlement": 0, "remaining": 0, "unlimited": true }
        }
    });
    assert_eq!(
        windows(Source::Copilot, body),
        vec![(WindowKind::Monthly, Some("premium".into()), 75.0, Some(1_782_864_000_000))]
    );
}

#[test]
fn percentages_are_clamped_and_http_failures_map_to_sign_in_states() {
    let body = json!({ "rate_limit": { "primary_window": { "used_percent": 140, "limit_window_seconds": 18000 } } });
    assert_eq!(windows(Source::Codex, body)[0].2, 100.0);
    assert_eq!(failure_status(401, ""), Some(UsageStatus::Expired));
    assert_eq!(
        failure_status(403, r#"{"error":{"type":"EntitlementError","message":"OpenCode Go subscription required."}}"#),
        Some(UsageStatus::Unsubscribed)
    );
    assert_eq!(failure_status(403, "forbidden"), Some(UsageStatus::Expired));
    assert_eq!(failure_status(429, ""), None);
}

#[test]
fn timestamps_accept_every_shape_providers_send() {
    assert_eq!(timestamp(&json!(1791036579998u64)), Some(1_791_036_579_998));
    assert_eq!(timestamp(&json!(1791217619)), Some(1_791_217_619_000));
    assert_eq!(timestamp(&json!("2026-07-01")), Some(1_782_864_000_000));
    assert_eq!(timestamp(&json!("2026-09-28T21:10:00.009134+00:00")), Some(1_790_629_800_009));
    assert_eq!(timestamp(&json!("soon")), None);
    assert_eq!(timestamp(&Value::Null), None);
}
