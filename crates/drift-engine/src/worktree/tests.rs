use super::*;

#[test]
fn a_name_is_one_branch_safe_segment() {
    for good in ["feature", "fix-login.2", "a_b"] {
        assert_eq!(check_name(good), Ok(()), "{good}");
    }
    for bad in [
        "",
        "../up",
        "a/b",
        "a b",
        ".hidden",
        "-flag",
        "x.lock",
        "two..dots",
        "end.",
    ] {
        assert!(check_name(bad).is_err(), "{bad}");
    }
    assert!(check_name(&"n".repeat(NAME_LIMIT + 1)).is_err());
}

#[test]
fn a_prompt_names_its_worktree_from_its_first_words() {
    let name = name_for("Fix the login bug, please, today");
    assert!(name.starts_with("fix-the-login-bug-"), "{name}");
    assert_eq!(check_name(&name), Ok(()));
    assert!(name_for("  ").starts_with("thread-"));
    assert_eq!(check_name(&name_for(&"word ".repeat(40))), Ok(()));
}

#[test]
fn owners_round_trip_and_workspaces_stay_plain() {
    assert_eq!(parse_owner(&owner("wsp_1", "feature")), Some(("wsp_1", "feature")));
    assert_eq!(parse_owner("wsp_1"), None);
}

#[test]
fn the_listing_gives_each_worktree_its_branch() {
    let listing = "worktree C:/repo\nHEAD abc\nbranch refs/heads/main\n\n\
                   worktree C:/data/worktrees/w/feature\nHEAD def\nbranch refs/heads/drift/feature\n\n\
                   worktree C:/data/worktrees/w/loose\nHEAD 123\ndetached";
    assert_eq!(
        parse_listing(listing),
        [
            ("C:/repo".to_string(), Some("main".to_string())),
            (
                "C:/data/worktrees/w/feature".to_string(),
                Some("drift/feature".to_string())
            ),
            ("C:/data/worktrees/w/loose".to_string(), None),
        ]
    );
}
