use super::*;
use crate::llm::catalog::Variant;

/// The harness's Claude with effort levels, taking per-message effort when `marks` is set.
fn effort_model(h: &Harness, marks: bool) {
    let mut catalog = h.engine.catalog.write().unwrap();
    let model = catalog
        .providers
        .get_mut("anthropic")
        .unwrap()
        .models
        .get_mut("claude-sonnet-4-5")
        .unwrap();
    model.variants = ["low", "high"]
        .map(|level| Variant {
            name: level.into(),
            reasoning: Reasoning::Effort { level: level.into() },
        })
        .to_vec();
    model.effort_updates = marks;
}

async fn at(h: &Harness, text: &str, level: &str) {
    let mut asked = prompt(text);
    asked.variant = Some(Some(level.into()));
    h.engine.submit(&h.session.id, asked).await.await_ok();
    until_idle(h).await;
}

fn effort(level: &str) -> Option<Reasoning> {
    Some(Reasoning::Effort { level: level.into() })
}

#[tokio::test]
async fn a_level_switch_keeps_the_top_level_effort_and_marks_where_it_changed() {
    let h = harness().await;
    effort_model(&h, true);
    h.provider.push(text("one")).push(text("two")).push(text("three"));
    at(&h, "first", "high").await;
    at(&h, "second", "low").await;
    at(&h, "third", "low").await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert!(
        requests.iter().all(|request| request.reasoning == effort("high")),
        "the prefix holds"
    );
    assert!(requests[0].effort_marks.is_empty());
    let mark = crate::llm::EffortMark {
        after: 2,
        effort: "low".into(),
    };
    assert_eq!(
        requests[1].effort_marks,
        std::slice::from_ref(&mark),
        "after the prompt that switched"
    );
    assert_eq!(
        requests[2].effort_marks,
        [mark],
        "replayed where it was, and not added again"
    );
    assert_eq!(requests[2].system, requests[0].system);
}

#[tokio::test]
async fn a_model_without_the_capability_changes_the_top_level_effort_as_before() {
    let h = harness().await;
    effort_model(&h, false);
    h.provider.push(text("one")).push(text("two"));
    at(&h, "first", "high").await;
    at(&h, "second", "low").await;

    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests[1].reasoning, effort("low"));
    assert!(requests[1].effort_marks.is_empty());
}

#[tokio::test]
async fn an_undo_past_a_mark_drops_it_with_the_prompt_it_rode_on() {
    let h = harness().await;
    effort_model(&h, true);
    h.provider.push(text("one")).push(text("two")).push(text("three"));
    at(&h, "first", "high").await;
    at(&h, "second", "low").await;
    let second = transcript(&h)[2].info.id.clone();
    h.engine.revert(&h.session.id, &second).await.unwrap();
    at(&h, "again", "high").await;

    let requests = h.provider.requests.lock().unwrap().clone();
    let last = requests.last().unwrap();
    assert_eq!(last.reasoning, effort("high"));
    assert!(last.effort_marks.is_empty(), "{:?}", last.effort_marks);
}
