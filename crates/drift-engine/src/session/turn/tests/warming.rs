use super::*;
use crate::session::turn::warming::{PROMPT_CACHE_KEY, PromptCache};

const KEEP_ALIVE: &str = "keep it warm";

fn warm(h: &Harness, window_minutes: u32) {
    let settings = PromptCache {
        warming: true,
        interval_minutes: 1,
        window_minutes,
        prompt: KEEP_ALIVE.into(),
    };
    h.engine.store.set_setting(PROMPT_CACHE_KEY, &settings).unwrap();
}

/// The keep-alive requests sent so far, which end with the warming prompt and never call tools.
fn keep_alives(h: &Harness) -> Vec<llm::Request> {
    h.provider
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| format!("{:?}", request.messages.last()).contains(KEEP_ALIVE))
        .cloned()
        .collect()
}

#[tokio::test]
async fn an_idle_conversation_is_kept_warm_until_the_window_ends_and_nothing_is_saved() {
    let h = harness().await;
    warm(&h, 3);
    h.provider.push(text("one"));
    for _ in 0..5 {
        h.provider.push(text("OK"));
    }
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    let before = transcript(&h).last().unwrap().info.cost;
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let sent = keep_alives(&h);
    assert_eq!(sent.len(), 3, "one each interval, until the window ends");
    assert!(sent.iter().all(|request| request.no_tool_calls));
    let real = h.provider.requests.lock().unwrap()[0].clone();
    assert_eq!(sent[0].system, real.system, "the same prefix the next turn would send");
    let messages = transcript(&h);
    assert_eq!(messages.len(), 2, "nothing reaches the transcript");
    assert!(
        messages[1].info.cost > before,
        "its cost goes on the reply it kept warm"
    );
}

#[tokio::test]
async fn a_real_prompt_starts_the_timers_again_and_a_switched_off_setting_sends_none() {
    let h = harness().await;
    warm(&h, 2);
    h.provider.push(text("one"));
    h.provider.push(text("OK"));
    h.provider.push(text("two"));
    for _ in 0..3 {
        h.provider.push(text("OK"));
    }
    h.engine.submit(&h.session.id, prompt("first")).await.await_ok();
    until_idle(&h).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.engine.submit(&h.session.id, prompt("second")).await.await_ok();
    until_idle(&h).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        keep_alives(&h).len(),
        3,
        "one after the first prompt, then a fresh window of two"
    );

    h.engine
        .store
        .set_setting(PROMPT_CACHE_KEY, &PromptCache::default())
        .unwrap();
    h.provider.push(text("three"));
    h.engine.submit(&h.session.id, prompt("third")).await.await_ok();
    until_idle(&h).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(keep_alives(&h).len(), 3, "off stops it");
}

#[tokio::test]
async fn a_worker_is_never_kept_warm() {
    let h = harness().await;
    warm(&h, 2);
    h.provider
        .push(tool_call("task", r#"{"description": "Look", "prompt": "Say hi"}"#))
        .push(text("hi from the worker"))
        .push(text("done"));
    for _ in 0..4 {
        h.provider.push(text("OK"));
    }
    h.engine.submit(&h.session.id, prompt("delegate")).await.await_ok();
    until_idle(&h).await;
    tokio::time::sleep(Duration::from_millis(700)).await;

    let sent = keep_alives(&h);
    assert_eq!(sent.len(), 2, "the conversation's window, nothing for its worker");
    assert!(
        sent.iter()
            .all(|request| format!("{:?}", request.messages[0]).contains("delegate")),
        "every keep-alive is the conversation's own"
    );
}
