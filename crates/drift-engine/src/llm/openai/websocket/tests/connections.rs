//! Which requests share a connection, and how a connection ends and is replaced.

use super::*;

#[tokio::test]
async fn conversations_credentials_and_mode_headers_never_share_a_connection() {
    let h = fixture(Server::default(), None).await;

    // Two conversations at once.
    let mut other = request();
    other.cache_key = Some("other".into());
    let (first, credential) = (request(), key());
    let (a, b) = tokio::join!(
        collect(&h.provider, &first, &credential),
        collect(&h.provider, &other, &credential)
    );
    assert!(a.iter().chain(&b).all(Result::is_ok));

    // The same conversation under another key.
    let different = Credential::ApiKey {
        key: "different".into(),
    };
    collect(&h.provider, &request(), &different).await;

    // The same conversation in a mode that sends its own header.
    let mut mode = request();
    mode.mode = Some(crate::llm::catalog::ModelMode {
        name: "fast".into(),
        base: mode.model.clone(),
        body: Default::default(),
        headers: [("x-test".into(), "fast".into())].into(),
    });
    collect(&h.provider, &mode, &key()).await;

    assert_eq!(h.server.connections(), 4);
}

#[tokio::test]
async fn a_subscription_handshake_carries_its_account_session_region_and_beta() {
    use base64::Engine as _;

    let h = fixture(Server::default(), None).await;
    let claims = json!({ "chatgpt_compute_residency": "eu" }).to_string();
    let credential = Credential::OAuth {
        access: format!(
            "h.{}.s",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims)
        ),
        refresh: String::new(),
        expires_at: i64::MAX,
        account: Some("account".into()),
    };

    collect(&h.provider, &request(), &credential).await;

    let frames = h.server.frames.lock().unwrap();
    let headers = &frames[0].headers;
    assert!(headers["authorization"].to_str().unwrap().starts_with("Bearer "));
    assert_eq!(headers["chatgpt-account-id"], "account");
    assert_eq!(headers["session-id"], "session");
    assert_eq!(headers["x-openai-internal-codex-residency"], "eu");
    assert_eq!(headers["originator"], "opencode");
    assert_eq!(headers["openai-beta"], "responses_websockets=2026-02-06");
    assert!(
        frames[0].body.get("max_output_tokens").is_none(),
        "the Codex backend refuses it"
    );
}

#[tokio::test]
async fn a_quiet_connection_answers_pings_while_tools_run_and_stays_open() {
    let h = fixture(
        Server {
            ping_idle: true,
            ..Server::default()
        },
        None,
    )
    .await;

    collect(&h.provider, &request(), &key()).await;
    eventually(async || h.server.pongs.load(Ordering::SeqCst) > 0).await;
    collect(&h.provider, &continued(request()), &key()).await;

    assert_eq!(h.server.pongs.load(Ordering::SeqCst), 1);
    assert_eq!(h.server.connections(), 1);
}

#[tokio::test]
async fn an_expired_connection_is_replaced_and_the_new_one_gets_the_whole_history() {
    let h = fixture(Server::default(), None).await;

    collect(&h.provider, &request(), &key()).await;
    h.slot().lock().await.as_mut().unwrap().opened = Instant::now() - LIFETIME;
    collect(&h.provider, &continued(request()), &key()).await;

    assert_eq!(h.server.connections(), 2);
    assert!(
        h.server.body(1).get("previous_response_id").is_none(),
        "a new socket has no previous response"
    );
    assert_eq!(h.server.body(1)["input"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn an_idle_connection_closes_its_socket_and_the_next_request_opens_a_new_one() {
    let mut h = fixture(Server::default(), None).await;
    h.provider.websockets = Arc::new(Pool {
        idle: Duration::from_millis(100),
        ..Pool::default()
    });

    collect(&h.provider, &request(), &key()).await;
    eventually(async || h.server.closed.load(Ordering::SeqCst) == 1).await;
    collect(&h.provider, &continued(request()), &key()).await;

    assert_eq!(
        h.server.closed.load(Ordering::SeqCst),
        1,
        "the idle socket was closed, not left open"
    );
    assert_eq!(h.server.connections(), 2);
    assert!(h.server.body(1).get("previous_response_id").is_none());
}

#[tokio::test]
async fn a_socket_the_server_closed_is_replaced_before_the_next_request() {
    let h = fixture(
        Server {
            close_after_reply: AtomicBool::new(true),
            ..Server::default()
        },
        None,
    )
    .await;

    collect(&h.provider, &request(), &key()).await;
    let slot = h.slot();
    eventually(async || slot.lock().await.as_ref().unwrap().jobs.is_closed()).await;
    collect(&h.provider, &continued(request()), &key()).await;

    assert_eq!(h.server.connections(), 2);
    assert!(h.server.body(1).get("previous_response_id").is_none());
}

#[tokio::test]
async fn a_stopped_response_takes_its_socket_with_it() {
    let h = fixture(
        Server {
            stall: true,
            ..Server::default()
        },
        None,
    )
    .await;

    // Dropping the stream is how a turn stops; the half-read socket must not serve the next request.
    let stopped = h.provider.stream(&request(), &key()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(stopped);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let next = h.provider.stream(&request(), &key()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(next);

    assert_eq!(h.server.connections(), 2);
    assert!((0..h.server.requests()).all(|index| h.server.body(index).get("previous_response_id").is_none()));
}

#[test]
fn the_pool_holds_at_most_its_limit_however_many_conversations_run() {
    let pool = Pool::default();

    for index in 0..100 {
        let prepared = Prepared {
            handshake: crate::llm::http::client()
                .post("http://127.0.0.1/responses")
                .build()
                .unwrap(),
            body: json!({}),
            session: Some(format!("session_{index}")),
            timeouts: Timeouts::default(),
            subscription: false,
        };
        drop(pool.slot(&prepared));
    }

    assert_eq!(pool.entries.lock().unwrap().len(), MAX_CONNECTIONS);
}
