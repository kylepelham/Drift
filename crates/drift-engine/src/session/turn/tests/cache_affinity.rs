use super::*;

fn keys(h: &Harness, index: usize) -> (Option<String>, Option<String>) {
    let request = h.provider.requests.lock().unwrap()[index].clone();
    (request.cache_key, request.connection)
}

#[tokio::test]
async fn forks_spawned_threads_and_workers_share_their_roots_cache_but_never_its_connection() {
    let h = harness().await;
    let root = h.session.id.clone();
    h.provider
        .push(tool_call("task", r#"{"description": "Look", "prompt": "Say hi"}"#))
        .push(text("hi from the worker"))
        .push(text("done"));
    h.engine.submit(&root, prompt("delegate")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(keys(&h, 0), (Some(root.clone()), Some(root.clone())));
    let (cache, connection) = keys(&h, 1);
    assert_eq!(
        cache.as_deref(),
        Some(root.as_str()),
        "the worker uses its parent's cache"
    );
    assert_ne!(connection.as_deref(), Some(root.as_str()), "but its own connection");

    let fork = h.engine.fork(&root, None).unwrap();
    h.provider.push(text("forked"));
    h.engine.submit(&fork.id, prompt("go on")).await.await_ok();
    until_session_idle(&h, &fork.id).await;
    assert_eq!(keys(&h, 3), (Some(root.clone()), Some(fork.id.clone())));

    let forked_again = h.engine.fork(&fork.id, None).unwrap();
    assert_eq!(
        h.engine.store.cache_root(&forked_again.id).unwrap(),
        root,
        "a fork of a fork still shares the first conversation's cache"
    );

    let spawned = h.engine.spawn(&root, "carry on elsewhere").await.unwrap();
    assert_eq!(h.engine.store.cache_root(&spawned.id).unwrap(), root);
}

#[tokio::test]
async fn an_agents_md_edit_mid_session_leaves_the_system_prompt_and_arrives_as_an_update() {
    let h = harness().await;
    std::fs::write(
        h._dir.join("ws/AGENTS.md"),
        "Be brief.
",
    )
    .unwrap();
    h.provider.push(text("one")).push(text("two"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    std::fs::write(
        h._dir.join("ws/AGENTS.md"),
        "Be thorough.
",
    )
    .unwrap();
    h.engine.submit(&h.session.id, prompt("second")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests[1].system, requests[0].system);
    assert!(requests[0].system.contains("Be brief."));
    let segments = |index: usize| crate::session::turn::cache_watch::segments(&requests[index]);
    assert_eq!(
        crate::session::turn::cache_watch::first_change(&segments(0), &segments(1)),
        None,
        "the first request is an exact prefix of the second"
    );
    let sent = format!("{:?}", requests[1].messages);
    assert!(
        sent.contains("These instructions changed") && sent.contains("Be thorough."),
        "{sent}"
    );
}

/// A reply whose request had `input` uncached and `cache_read` cached prompt tokens.
fn answered(input: u64, cache_read: u64, reply: &str) -> Vec<Chunk> {
    let mut chunks = vec![Chunk::Usage(Usage {
        input,
        cache_read,
        ..Usage::default()
    })];
    chunks.extend(text(reply).into_iter().skip(1));
    chunks
}

#[tokio::test]
async fn a_reply_whose_request_missed_the_cache_keeps_why() {
    let h = harness().await;
    h.provider
        .push(answered(6_000, 0, "first"))
        .push(answered(200, 6_000, "second"))
        .push(answered(6_500, 0, "third"));
    for ask in ["one", "two", "three"] {
        h.engine.submit(&h.session.id, prompt(ask)).await.await_ok();
        until_idle(&h).await;
    }

    let misses: Vec<Option<String>> = transcript(&h)
        .into_iter()
        .filter(|message| message.info.role == Role::Assistant)
        .map(|message| message.info.cache_miss)
        .collect();
    assert_eq!(misses[..2], [None, None]);
    let why = misses[2].clone().unwrap();
    assert!(why.ends_with("(0 of 6.2k cached)"), "{why}");
}
