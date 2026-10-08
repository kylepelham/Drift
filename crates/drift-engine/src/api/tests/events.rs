use super::*;

#[tokio::test]
async fn creating_a_workspace_lists_it_and_publishes_an_event() {
    let harness = harness().await;
    let mut socket = harness.ws("").await;
    let hello = next_json(&mut socket).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["seq"], 0);

    let created = json_response(
        harness
            .post("/workspaces")
            .json(&json!({ "path": "C:/repo", "name": "repo" })),
    )
    .await;
    assert_eq!(created["name"], "repo");
    let listed = json_response(harness.get("/workspaces")).await;
    assert_eq!(listed[0]["id"], created["id"]);
    let event = next_json(&mut socket).await;
    assert_eq!(event["type"], "workspace.created");
    assert_eq!(event["seq"], 1);
    assert_eq!(event["workspace"]["id"], created["id"]);
}

#[tokio::test]
async fn reconnecting_with_a_cursor_replays_missed_events() {
    let harness = harness().await;
    for name in ["a", "b", "c"] {
        harness.engine.hub.publish(Event::WorkspaceCreated {
            workspace: harness.workspace(name),
        });
    }

    let mut socket = harness.ws("&cursor=1").await;
    let hello = next_json(&mut socket).await;
    assert_eq!(hello["seq"], 3);
    assert_eq!(next_json(&mut socket).await["seq"], 2);
    assert_eq!(next_json(&mut socket).await["seq"], 3);
    harness.engine.hub.publish(Event::WorkspaceCreated {
        workspace: harness.workspace("d"),
    });
    assert_eq!(next_json(&mut socket).await["seq"], 4);
}

#[tokio::test]
async fn stale_cursor_gets_resync() {
    let harness = harness().await;
    let engine = Engine::open_with(
        &harness.directory.0.join("stale"),
        crate::Options {
            event_history: 2,
            file_credentials: true,
        },
    )
    .unwrap();
    let server = listen(engine.clone(), SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    for name in ["a", "b", "c"] {
        engine.hub.publish(Event::WorkspaceCreated {
            workspace: harness.workspace(name),
        });
    }

    let url = format!("ws://{}/events?token={}&cursor=0", server.addr, engine.token);
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    assert_eq!(next_json(&mut socket).await["type"], "hello");
    let resync = next_json(&mut socket).await;
    assert_eq!(resync["type"], "resync");
    assert_eq!(resync["seq"], 3);
    server.stop();
}

#[tokio::test]
async fn client_close_ends_the_stream() {
    let harness = harness().await;
    let mut socket = harness.ws("").await;
    next_json(&mut socket).await;
    socket.send(Message::Close(None)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        harness.engine.hub.publish(Event::WorkspaceCreated {
            workspace: harness.workspace("x")
        }),
        1
    );
}

#[tokio::test]
async fn a_question_reply_on_the_socket_hears_how_it_ended() {
    let harness = harness().await;
    let mut socket = harness.ws("").await;
    next_json(&mut socket).await;
    let reply = json!({ "type": "question.reply", "requestId": "que_gone", "answers": [["yes"]] });
    socket.send(Message::Text(reply.to_string().into())).await.unwrap();

    let result = until(&mut socket, "question.result").await;
    assert_eq!(result["requestId"], "que_gone");
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "not_found");
    assert!(result.get("seq").is_none(), "a reply's result is not an event");
}

#[tokio::test]
async fn a_socket_a_host_leases_closes_when_the_lease_is_cancelled() {
    let harness = harness().await;
    let lease = tokio_util::sync::CancellationToken::new();
    let held = lease.clone();
    let router = crate::api::router(harness.engine.clone()).layer(axum::middleware::from_fn(
        move |mut request: axum::extract::Request, next: axum::middleware::Next| {
            request.extensions_mut().insert(crate::api::Lease(held.clone()));
            next.run(request)
        },
    ));
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/events?token={}", harness.engine.token))
            .await
            .unwrap();
    assert!(
        matches!(socket.next().await, Some(Ok(Message::Text(_)))),
        "hello arrives while the lease holds"
    );
    lease.cancel();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                None | Some(Err(_) | Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the socket closed once the host took the lease back");
}

#[tokio::test]
async fn a_workspace_a_client_has_open_keeps_its_mcp_servers_until_its_socket_closes() {
    let harness = harness().await;
    let directory = harness.directory.0.join("open-ws");
    std::fs::create_dir_all(&directory).unwrap();
    let here = crate::tool::canonical(&directory);
    let mut socket = harness.ws("").await;

    socket
        .send(Message::Text(
            json!({ "type": "workspace.open", "directory": directory })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    for _ in 0..100 {
        if harness.engine.mcp.is_open(&here) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(harness.engine.mcp.is_open(&here), "the socket's workspace is open");

    socket.send(Message::Close(None)).await.unwrap();
    drop(socket);
    for _ in 0..100 {
        if !harness.engine.mcp.is_open(&here) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !harness.engine.mcp.is_open(&here),
        "and no longer once the socket closes"
    );
}
