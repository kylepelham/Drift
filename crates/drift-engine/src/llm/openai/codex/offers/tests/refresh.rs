//! Fetching the list: once at a time, only with a sign-in, and never outliving a sign-out.

use super::*;

#[tokio::test]
async fn one_fetch_serves_concurrent_refreshes_and_the_cache_serves_the_next() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (base, task) = server(list(), StatusCode::OK, calls.clone()).await;
    let (offers, credential, client) = (
        Offers::default(),
        credential("user", "account"),
        crate::llm::http::client(),
    );

    let (first, second) = tokio::join!(
        offers.refresh(&client, &base, &credential),
        offers.refresh(&client, &base, &credential)
    );
    let again = offers.refresh(&client, &base, &credential).await;

    assert!(first || second, "the first fetch changed the catalog");
    assert!(!again, "a current list is not fetched again");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let cached = offers.state.read().unwrap().cached.as_ref().unwrap().offers.clone();
    assert_eq!(cached["gpt-6-sol"].program.as_deref(), Some("daybreak_blue"));
    assert_eq!(cached["gpt-6-sol"].tiers, ["priority"]);
    assert_eq!(cached["unknown"].program, None, "an unknown program is ignored");
    task.abort();
}

#[tokio::test]
async fn a_failed_or_unreadable_list_changes_nothing_and_waits_before_trying_again() {
    for (body, status) in [
        (list(), StatusCode::FORBIDDEN),
        (json!({ "unexpected": [] }), StatusCode::OK),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let (base, task) = server(body, status, calls.clone()).await;
        let credential = credential("user", "account");
        let offers = cached(&credential);
        offers.state.write().unwrap().cached.as_mut().unwrap().until = Instant::now() - RETRY;
        let client = crate::llm::http::client();

        assert!(
            offers.refresh(&client, &base, &credential).await,
            "the old list is dropped"
        );
        assert!(
            !offers.refresh(&client, &base, &credential).await,
            "and not fetched again at once"
        );

        let mut provider = provider();
        offers.apply(&mut provider, &credential);
        assert_eq!(provider, self::provider());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test]
async fn an_api_key_is_never_sent_to_the_list() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (base, task) = server(list(), StatusCode::OK, calls.clone()).await;
    let offers = Offers::default();

    offers
        .refresh(
            &crate::llm::http::client(),
            &base,
            &Credential::ApiKey { key: "key".into() },
        )
        .await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn a_sign_out_during_a_fetch_leaves_the_cache_empty() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signals = (entered.clone(), release.clone());

    // The list answers only once the test says so, so the sign-out lands mid-fetch.
    let answer = move || {
        let (entered, release) = signals.clone();
        async move {
            entered.notify_one();
            release.notified().await;
            axum::Json(list())
        }
    };
    let app = axum::Router::new().route("/models", axum::routing::get(answer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (offers, credential, client) = (
        Offers::default(),
        credential("user", "account"),
        crate::llm::http::client(),
    );
    let (changed, ()) = tokio::join!(offers.refresh(&client, &base, &credential), async {
        entered.notified().await;
        offers.clear();
        release.notify_one();
    });

    assert!(!changed);
    assert!(offers.state.read().unwrap().cached.is_none());
    task.abort();
}
