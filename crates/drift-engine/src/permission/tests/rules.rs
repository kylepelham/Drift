use super::*;

#[test]
fn workspace_rules_are_checked_against_every_command_of_a_line() {
    let workspace = Policy {
        rules: vec![
            rule("bash", "git push*", Decision::Deny),
            rule("bash", "git *", Decision::Allow),
        ],
    };
    let permissions = Permissions::new(Policy::default());

    assert_eq!(
        permissions.decide("ses_1", &workspace, &shell("git status")),
        Decision::Allow
    );
    assert_eq!(
        permissions.decide("ses_1", &workspace, &shell("git status && rm -rf ~")),
        Decision::Ask,
        "the rm is not covered by git *"
    );
    assert_eq!(
        permissions.decide("ses_1", &workspace, &shell("git log | git push --force")),
        Decision::Deny,
        "a denied command denies the line"
    );
    assert_eq!(
        permissions.decide("ses_1", &workspace, &shell("git status $(rm -rf ~)")),
        Decision::Ask,
        "a hidden command is not covered by a wildcard"
    );
}

#[test]
fn deny_rules_catch_assignment_prefixes_and_powershell_aliases_but_approvals_stay_literal() {
    let permissions = Permissions::new(Policy::default());
    let deny = |pattern| Policy {
        rules: vec![rule("bash", pattern, Decision::Deny)],
    };
    assert_eq!(
        permissions.decide("ses_1", &deny("git status*"), &shell("FIXTURE=1 git status")),
        Decision::Deny
    );
    assert_eq!(
        permissions.decide(
            "ses_1",
            &deny("git push*"),
            &shell("ls && GIT_TRACE=1 git push --force")
        ),
        Decision::Deny
    );
    assert_eq!(
        permissions.decide("ses_1", &deny("Remove-Item *"), &powershell("rm build -Recurse")),
        Decision::Deny
    );

    let allow = Policy {
        rules: vec![rule("bash", "git status*", Decision::Allow)],
    };
    assert_eq!(
        permissions.decide("ses_1", &allow, &shell("EVIL=1 git status")),
        Decision::Ask,
        "an allow rule does not reach past what was written"
    );
    approve_always(&permissions, shell("LANG=C git status"));
    assert_eq!(
        permissions.decide("ses_1", &Policy::default(), &shell("LANG=C git status")),
        Decision::Allow,
        "approved as written"
    );
    assert_eq!(
        permissions.decide("ses_1", &Policy::default(), &shell("PATH=/tmp git status")),
        Decision::Ask
    );
}

#[test]
fn a_secret_read_is_allowed_only_by_name_and_denied_by_any_glob() {
    let broad = Policy {
        rules: vec![rule("read", "*", Decision::Allow)],
    };
    let permissions = Permissions::new(Policy::default());
    assert_eq!(
        permissions.decide("ses_1", &broad, &ask("read", "C:/repo/src/a.rs")),
        Decision::Allow
    );
    assert_eq!(
        permissions.decide("ses_1", &broad, &ask("read", "C:/repo/.env")),
        Decision::Ask,
        "read * does not cover secrets"
    );
    let session = new_request("ses_1", "m", "c", "read", ask("read", "C:/elsewhere/a.rs"));
    permissions.apply(
        &session,
        &ReplyBody {
            reply: Reply::Always,
            pattern: Some("**".into()),
            message: None,
        },
    );
    assert_eq!(
        permissions.decide("ses_1", &Policy::default(), &ask("read", "C:/repo/.env")),
        Decision::Ask,
        "a widened approval does not either"
    );

    let named = Policy {
        rules: vec![rule("read", "C:/repo/.env", Decision::Allow)],
    };
    assert_eq!(
        permissions.decide("ses_1", &named, &ask("read", "C:/repo/.env")),
        Decision::Allow
    );
    let deny = Policy {
        rules: vec![rule("read", "**/.env*", Decision::Deny)],
    };
    assert_eq!(
        permissions.decide("ses_1", &deny, &ask("read", "C:/repo/.env.local")),
        Decision::Deny
    );
    approve_always(&permissions, ask("read", "C:/repo/.env"));
    assert_eq!(
        permissions.decide("ses_1", &Policy::default(), &ask("read", "C:/repo/.env")),
        Decision::Allow,
        "always remembers this file"
    );
    assert_eq!(
        permissions.decide("ses_1", &Policy::default(), &ask("read", "C:/repo/.env.local")),
        Decision::Ask,
        "and only this file"
    );
}

#[test]
fn first_matching_rule_wins_and_no_match_asks() {
    let policy = Policy {
        rules: vec![
            rule("bash", "git push*", Decision::Deny),
            rule("bash", "git *", Decision::Allow),
            rule("edit", "C:/repo/**", Decision::Allow),
        ],
    };

    assert_eq!(policy.decide(&ask("bash", "git push origin")), Decision::Deny);
    assert_eq!(policy.decide(&ask("bash", "git status")), Decision::Allow);
    assert_eq!(policy.decide(&ask("bash", "rm -rf /")), Decision::Ask);
    assert_eq!(policy.decide(&ask("edit", "C:/repo/src/a.rs")), Decision::Allow);
    assert_eq!(policy.decide(&ask("edit", "C:/other/a.rs")), Decision::Ask);
}

#[test]
fn a_committed_relative_rule_matches_paths_inside_the_workspace() {
    let workspace = std::path::Path::new("C:/repo");
    let path = |relative: &str| Ask::path("edit", &workspace.join(relative), workspace, relative);
    let rules = Policy {
        rules: vec![
            rule("edit", "src/generated/**", Decision::Deny),
            rule("edit", "src/**", Decision::Allow),
        ],
    };

    assert_eq!(
        path("src/generated/a.rs").relative.as_deref(),
        Some("src/generated/a.rs")
    );
    assert_eq!(rules.decide(&path("src/generated/a.rs")), Decision::Deny);
    assert_eq!(rules.decide(&path("src/a.rs")), Decision::Allow);
    assert_eq!(rules.decide(&path("docs/a.md")), Decision::Ask);
    let permissions = Permissions::new(Policy::default());
    assert_eq!(
        permissions.decide("ses_1", &rules, &path("src/generated/b.rs")),
        Decision::Deny,
        "the session check sees it too"
    );
    let outside = Ask::path("edit", std::path::Path::new("C:/other/src/a.rs"), workspace, "outside");
    assert_eq!(
        (outside.relative.as_deref(), rules.decide(&outside)),
        (None, Decision::Ask),
        "nothing outside is relative"
    );
}
