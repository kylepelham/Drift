use super::*;

async fn until_engine_idle(engine: &Arc<Engine>, session_id: &str) {
    for _ in 0..1000 {
        if !engine.turns.is_running(session_id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn never finished");
}

fn sent(h: &Harness) -> Vec<String> {
    h.provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| format!("{:?}", request.messages))
        .collect()
}

#[tokio::test]
async fn a_turn_cut_off_by_a_restart_starts_again_once_with_a_note() {
    let h = harness().await;
    h.provider.push_stall();
    h.engine.submit(&h.session.id, prompt("long job")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let restarted = reopen(&h);
    h.provider.push(text("checked and carried on"));
    restarted.resume_turns().await;
    until_engine_idle(&restarted, &h.session.id).await;

    let requests = sent(&h);
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("long job") && requests[1].contains("Drift restarted while this turn was running"));
    assert!(
        restarted.store.claimed_turns().unwrap().is_empty(),
        "the resumed turn finished and let go"
    );
}

#[tokio::test]
async fn a_resumed_turn_whose_workers_were_restarted_is_told_not_to_launch_them_again() {
    let h = harness().await;
    h.provider.push_stall();
    h.engine.submit(&h.session.id, prompt("delegate it")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let restarted = reopen(&h);
    restarted
        .turns
        .restarted_owners
        .lock()
        .unwrap()
        .insert(h.session.id.clone());
    h.provider.push(text("waiting on them"));
    restarted.resume_turns().await;
    until_engine_idle(&restarted, &h.session.id).await;

    let requests = sent(&h);
    assert!(
        requests[1].contains("background subagents that were running were restarted too"),
        "{}",
        requests[1]
    );
}

#[tokio::test]
async fn a_turn_the_user_stopped_and_a_third_restart_in_one_prompt_do_not_resume() {
    let h = harness().await;
    h.provider.push_stall();
    h.engine.submit(&h.session.id, prompt("stop me")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    reopen(&h).resume_turns().await;
    assert_eq!(sent(&h).len(), 1, "a stop is the user's");

    h.engine.store.claim_turn(&h.session.id).unwrap();
    h.engine.store.count_resume(&h.session.id).unwrap();
    h.engine.store.count_resume(&h.session.id).unwrap();
    let restarted = reopen(&h);
    restarted.resume_turns().await;
    assert_eq!(sent(&h).len(), 1, "two resumes already spent on this prompt");
    assert!(restarted.store.claimed_turns().unwrap().is_empty());

    restarted.store.claim_turn(&h.session.id).unwrap();
    restarted
        .store
        .set_setting(crate::session::turn::resume::RESUME_KEY, &false)
        .unwrap();
    let off = reopen(&h);
    off.resume_turns().await;
    assert_eq!(sent(&h).len(), 1, "the switch off resumes nothing");
    assert!(off.store.claimed_turns().unwrap().is_empty());
}
