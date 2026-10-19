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
async fn an_agents_md_edit_mid_session_is_named_as_the_first_change_in_the_prompt() {
    let h = harness().await;
    std::fs::write(h._dir.join("ws/AGENTS.md"), "Be brief.\n").unwrap();
    h.provider.push(text("one")).push(text("two"));
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    std::fs::write(h._dir.join("ws/AGENTS.md"), "Be thorough.\n").unwrap();
    h.engine.submit(&h.session.id, prompt("second")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    let segments = |index: usize| crate::session::turn::cache_watch::segments(&requests[index]);
    let changed = crate::session::turn::cache_watch::first_change(&segments(0), &segments(1)).unwrap();
    assert!(changed.contains("Instructions from AGENTS.md"), "{changed}");
}
