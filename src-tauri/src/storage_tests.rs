use super::{quote_list, rules_for, PruneRules};

#[test]
fn rules_are_selected_independently() {
    let none = rules_for(PruneRules::default(), &[]);
    assert!(none.is_empty(), "no rules selected should produce no work");

    let only_orphans = rules_for(
        PruneRules {
            orphan_events: true,
            ..PruneRules::default()
        },
        &[],
    );
    assert_eq!(only_orphans.len(), 1);
    assert_eq!(only_orphans[0].0, "orphan-events");

    // Superseded snapshots cover two event types, so the single flag yields two selectors.
    let superseded = rules_for(
        PruneRules {
            superseded_snapshots: true,
            ..PruneRules::default()
        },
        &[],
    );
    assert_eq!(superseded.len(), 2);
    assert!(superseded[0].1.contains("$.part.id"));
    assert!(superseded[1].1.contains("$.info.id"));
    // Only the newest snapshot per id survives.
    assert!(superseded.iter().all(|(_, sql)| sql.contains("rank > 1")));
}

#[test]
fn drift_archived_ids_widen_the_archive_rule() {
    let base = PruneRules {
        archived_events: true,
        ..PruneRules::default()
    };
    let engine_only = rules_for(base, &[]);
    assert!(engine_only[0].1.contains("time_archived IS NOT NULL"));
    assert!(!engine_only[0].1.contains(" OR id IN"));

    let with_drift = rules_for(base, &["ses_abc".into(), "ses_def".into()]);
    assert!(with_drift[0].1.contains("'ses_abc','ses_def'"));
}

#[test]
fn quote_list_drops_ids_that_are_not_plain_identifiers() {
    let quoted = quote_list(&[
        "ses_ok-1".into(),
        "bad'; DROP TABLE session; --".into(),
        "also_ok".into(),
    ]);
    assert_eq!(quoted, "'ses_ok-1','also_ok'");
    assert!(!quoted.contains("DROP"));
}
