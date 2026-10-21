use super::*;

/// Every request's messages as text, so a test can look for what the model was told.
fn sent(h: &Harness, index: usize) -> String {
    format!("{:?}", h.provider.requests.lock().unwrap()[index].messages)
}

fn system(h: &Harness, index: usize) -> String {
    h.provider.requests.lock().unwrap()[index].system.clone()
}

async fn ask(h: &Harness, text: &str) {
    h.engine.submit(&h.session.id, prompt(text)).await.await_ok();
    until_idle(h).await;
}

fn updates(text: &str) -> usize {
    text.matches("These instructions changed").count()
}

#[tokio::test]
async fn a_new_date_is_one_update_replayed_as_saved_and_the_system_prompt_never_changes() {
    let h = harness().await;
    h.provider.push(text("one")).push(text("two")).push(text("three"));
    ask(&h, "first").await;
    let stale = crate::store::InstructionValue {
        key: "date",
        text: "Date: 2000-01-01",
    };
    h.engine
        .store
        .update_instructions(&h.session.id, &[stale], &[])
        .unwrap();

    ask(&h, "second").await;
    ask(&h, "third").await;

    assert_eq!(system(&h, 1), system(&h, 0));
    assert_eq!(system(&h, 2), system(&h, 0));
    assert_eq!(updates(&sent(&h, 1)), 1, "{}", sent(&h, 1));
    assert!(sent(&h, 1).contains(&crate::platform::clock::local_date()));
    assert_eq!(
        updates(&sent(&h, 2)),
        1,
        "the same update is replayed, and nothing new is added"
    );
    assert!(
        sent(&h, 2).starts_with(&sent(&h, 1)[..sent(&h, 1).len() - 2]),
        "the second request is a prefix of the third"
    );
}

#[tokio::test]
async fn a_skill_switched_off_replaces_the_skills_list() {
    let h = harness().await;
    let skill = h._dir.join("ws/.drift/skills/tidy");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\ndescription: Tidies\n---\nTidy up.").unwrap();
    h.provider.push(text("one")).push(text("two"));
    ask(&h, "first").await;
    std::fs::remove_dir_all(&skill).unwrap();
    ask(&h, "second").await;

    assert!(system(&h, 0).contains("- tidy: Tidies"));
    assert_eq!(system(&h, 1), system(&h, 0));
    let update = sent(&h, 1);
    assert!(
        update.contains("These instructions changed") && update.contains("- review: ") && !update.contains("- tidy"),
        "the engine's own skill stays listed: {update}"
    );
}

#[tokio::test]
async fn an_mcp_server_connecting_mid_session_arrives_as_an_update() {
    let h = harness().await;
    h.provider.push(text("one")).push(text("two"));
    ask(&h, "first").await;
    h.engine.store.save_mcp_server("echo", &echo_server()).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
    ask(&h, "second").await;

    assert_eq!(system(&h, 1), system(&h, 0));
    assert!(
        sent(&h, 1).contains("Echo repeats what it is given."),
        "{}",
        sent(&h, 1)
    );
}

#[tokio::test]
async fn a_fork_keeps_its_sources_prefix_and_compaction_and_undo_start_a_new_baseline() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/AGENTS.md"), "Be brief.\n").unwrap();
    h.provider.push(text("one")).push(text("two"));
    ask(&h, "first").await;
    std::fs::write(h._dir.join("ws/AGENTS.md"), "Be thorough.\n").unwrap();
    ask(&h, "second").await;

    let fork = h.engine.fork(&h.session.id, None).unwrap();
    h.provider.push(text("forked"));
    h.engine.submit(&fork.id, prompt("go on")).await.await_ok();
    until_session_idle(&h, &fork.id).await;
    assert_eq!(
        system(&h, 2),
        system(&h, 0),
        "the fork sends its source's system prompt"
    );
    assert_eq!(updates(&sent(&h, 2)), 1, "and the source's update, not a second one");

    h.provider.push(text("SUMMARY")).push(text("three"));
    h.engine.start_compaction(&h.session.id).unwrap();
    until_idle(&h).await;
    ask(&h, "third").await;
    let last = h.provider.requests.lock().unwrap().len() - 1;
    assert!(
        system(&h, last).contains("Be thorough.") && !system(&h, last).contains("Be brief."),
        "re-based"
    );

    std::fs::write(h._dir.join("ws/AGENTS.md"), "Be precise.\n").unwrap();
    let third = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .rev()
        .find(|message| message.info.role == Role::User)
        .unwrap()
        .info
        .id
        .clone();
    h.engine.revert(&h.session.id, &third).await.unwrap();
    h.provider.push(text("after undo"));
    ask(&h, "again").await;
    let last = h.provider.requests.lock().unwrap().len() - 1;
    assert!(
        system(&h, last).contains("Be precise."),
        "a committed undo starts a new baseline too"
    );
    assert_eq!(updates(&sent(&h, last)), 0);
}
