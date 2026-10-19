use super::*;

/// Waits until the session is idle with nothing left in its queue.
async fn until_queue_runs_out(h: &Harness) {
    for _ in 0..500 {
        let empty = h.engine.queued_prompts(&h.session.id).unwrap().is_empty();
        if empty && !h.engine.turns.is_running(&h.session.id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the queue never ran out");
}

fn last_prompt(request: &Request) -> String {
    texts_sent(request).pop().unwrap_or_default()
}

#[tokio::test]
async fn two_queued_prompts_run_as_two_turns_in_order_and_a_steer_sent_meanwhile_lands_first() {
    let h = harness().await;
    h.provider
        .push_slow(Duration::from_millis(600), text("first answer"))
        .push(text("noted"))
        .push(text("queued one answer"))
        .push(text("queued two answer"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let first = h.engine.queue_prompt(&h.session.id, prompt("queued one")).unwrap();
    h.engine.queue_prompt(&h.session.id, prompt("queued two")).unwrap();
    assert_eq!(h.engine.queued_prompts(&h.session.id).unwrap()[0].id, first.id);
    h.engine.submit(&h.session.id, prompt("steer")).await.expect("steered");
    until_queue_runs_out(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 4);
    assert_eq!(last_prompt(&requests[1]), "steer", "the steer joins the running turn");
    assert_eq!(last_prompt(&requests[2]), "queued one");
    assert_eq!(last_prompt(&requests[3]), "queued two");
    assert!(
        texts_sent(&requests[3]).contains(&"queued one answer".to_string()),
        "the second waited for the first turn to end"
    );
    let prompts: Vec<_> = transcript(&h)
        .into_iter()
        .filter(|message| message.info.role == Role::User)
        .collect();
    assert_eq!(
        prompts.len(),
        4,
        "each queued prompt is in the transcript once it starts"
    );
}

#[tokio::test]
async fn after_stop_the_queue_starts_nothing_until_the_user_sends_a_prompt() {
    let h = harness().await;
    h.provider.push_stall();
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.engine.queue_prompt(&h.session.id, prompt("later")).unwrap();
    assert!(h.engine.abort(&h.session.id));
    until_idle(&h).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        h.engine.queued_prompts(&h.session.id).unwrap().len(),
        1,
        "Stop starts none"
    );
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);

    h.provider.push(text("going")).push(text("later answer"));
    h.engine.submit(&h.session.id, prompt("go on")).await.await_ok();
    until_queue_runs_out(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        last_prompt(&requests[2]),
        "later",
        "it starts after the prompt the user sent"
    );
}

#[tokio::test]
async fn a_queued_prompt_starts_at_once_when_nothing_runs_and_can_be_edited_discarded_or_sent_now() {
    let h = harness().await;
    h.provider.push(text("right away"));
    h.engine.queue_prompt(&h.session.id, prompt("now")).unwrap();
    until_queue_runs_out(&h).await;
    assert_eq!(last_prompt(&h.provider.requests.lock().unwrap()[0]), "now");

    h.provider
        .push_slow(Duration::from_millis(500), text("busy"))
        .push(text("sent now"));
    h.engine.submit(&h.session.id, prompt("work")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let edited = h.engine.queue_prompt(&h.session.id, prompt("draft")).unwrap();
    assert!(h.engine.edit_queued(&edited.id, &prompt("edited")).unwrap());
    let dropped = h.engine.queue_prompt(&h.session.id, prompt("never")).unwrap();
    assert!(h.engine.discard_queued(&dropped.id).unwrap());
    h.engine.send_queued_now(&edited.id).await.unwrap().expect("sent");
    until_queue_runs_out(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        3,
        "sent now joins the running turn; the discarded one never runs"
    );
    assert_eq!(last_prompt(&requests[2]), "edited");
}
