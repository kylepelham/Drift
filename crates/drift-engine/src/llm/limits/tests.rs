use serde_json::json;

use super::*;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(
            ::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }

    map
}

#[test]
fn codex_headers_name_the_weekly_and_five_hour_windows() {
    let limits = Limits::from_headers(&headers(&[
        ("x-codex-primary-used-percent", "39"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-primary-reset-at", "1791996130"),
        ("x-codex-secondary-used-percent", "12.5"),
        ("x-codex-secondary-window-minutes", "300"),
        ("x-codex-secondary-reset-at", "1791700000"),
    ]))
    .unwrap();

    assert_eq!(
        limits.weekly,
        Some(Window {
            used_percent: 39.0,
            resets_at: Some(1_791_996_130_000)
        })
    );
    assert_eq!(limits.five_hour.unwrap().used_percent, 12.5);
    assert_eq!(limits.spent_until, None);
}

#[test]
fn a_window_of_zero_minutes_is_not_reported() {
    let limits = Limits::from_headers(&headers(&[
        ("x-codex-primary-used-percent", "39"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-secondary-used-percent", "0"),
        ("x-codex-secondary-window-minutes", "0"),
    ]))
    .unwrap();

    assert!(limits.five_hour.is_none());
    assert!(limits.weekly.is_some());
    assert_eq!(Limits::from_headers(&HeaderMap::new()), None);
}

#[test]
fn a_full_window_spends_the_account_until_it_resets() {
    let limits = Limits::from_headers(&headers(&[
        ("x-codex-primary-used-percent", "100"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-primary-reset-at", "1791996130"),
    ]))
    .unwrap();

    assert_eq!(limits.spent_until, Some(1_791_996_130_000));
    assert_eq!(limits.spent_at(1_791_996_129_000), Some(1_791_996_130_000));
    assert_eq!(
        limits.spent_at(1_791_996_131_000),
        None,
        "usable again once the window resets"
    );
}

#[test]
fn the_codex_socket_event_reports_the_same_windows_and_a_reached_limit() {
    let event = json!({
        "type": "codex.rate_limits",
        "rate_limits": {
            "allowed": true,
            "limit_reached": true,
            "primary": { "used_percent": 98, "window_minutes": 10080, "reset_at": 1791996130 },
            "secondary": null
        },
        "credits": { "has_credits": true }
    });

    let limits = Limits::from_codex_event(&event).unwrap();

    assert_eq!(limits.weekly.unwrap().used_percent, 98.0);
    assert_eq!(
        limits.spent_until,
        Some(1_791_996_130_000),
        "a reached limit that credits would pay for still counts as spent"
    );
}

#[test]
fn anthropic_headers_report_fractions_and_a_rejection() {
    let limits = Limits::from_headers(&headers(&[
        ("anthropic-ratelimit-unified-status", "rejected"),
        ("anthropic-ratelimit-unified-reset", "1791700000"),
        ("anthropic-ratelimit-unified-5h-utilization", "1.0"),
        ("anthropic-ratelimit-unified-5h-reset", "1791700000"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.42"),
        ("anthropic-ratelimit-unified-7d-reset", "1792000000"),
    ]))
    .unwrap();

    assert_eq!(limits.five_hour.unwrap().used_percent, 100.0);
    assert!((limits.weekly.unwrap().used_percent - 42.0).abs() < 1e-9);
    assert_eq!(limits.spent_until, Some(1_791_700_000_000));
}

#[test]
fn the_ledger_marks_a_refusal_and_a_later_response_clears_it() {
    let ledger = Ledger::default();
    let weekly = Limits {
        weekly: Some(Window {
            used_percent: 40.0,
            resets_at: Some(10),
        }),
        ..Limits::default()
    };

    assert!(ledger.record("openai", weekly.clone()));
    assert!(!ledger.record("openai", weekly.clone()), "the same report is no change");

    let refused = ledger.refuse("openai", 5_000);
    assert_eq!(refused.weekly, weekly.weekly, "a refusal keeps the windows");
    assert_eq!(ledger.spent("openai", 1_000), Some(5_000));
    assert_eq!(ledger.spent("openai", 6_000), None);

    ledger.record("openai", weekly);
    assert_eq!(ledger.spent("openai", 1_000), None);
    assert_eq!(ledger.spent("unknown", 1_000), None);
}
