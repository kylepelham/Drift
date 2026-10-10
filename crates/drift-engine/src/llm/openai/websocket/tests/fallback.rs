//! When SSE takes over, and when a failure must stay a failure rather than be sent again.

use super::*;

#[tokio::test]
async fn an_endpoint_without_websockets_falls_back_to_sse_and_is_not_asked_again() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::METHOD_NOT_ALLOWED,
        StatusCode::UPGRADE_REQUIRED,
    ] {
        let h = fixture(Server::default(), Some(status)).await;

        collect(&h.provider, &request(), &key()).await;
        collect(&h.provider, &request(), &key()).await;

        assert_eq!((h.server.posts(), h.server.connections()), (2, 0), "{status}");
    }
}

#[tokio::test]
async fn refused_credentials_are_reported_not_retried_over_sse() {
    let h = fixture(Server::default(), Some(StatusCode::UNAUTHORIZED)).await;

    let refused = h.provider.stream(&request(), &key()).await;

    assert!(matches!(refused, Err(Error::Unauthenticated(_))));
    assert_eq!(h.server.posts(), 0);
}

#[tokio::test]
async fn a_101_without_a_valid_accept_key_is_refused() {
    let h = fixture(Server::default(), Some(StatusCode::SWITCHING_PROTOCOLS)).await;

    let refused = h.provider.stream(&request(), &key()).await;

    assert!(matches!(refused, Err(Error::Malformed(_))));
    assert_eq!(h.server.posts(), 0);
}

#[tokio::test]
async fn a_reply_cut_off_midway_fails_without_being_sent_again_over_sse() {
    let h = fixture(
        Server {
            partial: true,
            ..Server::default()
        },
        None,
    )
    .await;

    let chunks = collect(&h.provider, &request(), &key()).await;

    assert!(chunks.iter().any(Result::is_err));
    assert_eq!((h.server.requests(), h.server.posts()), (1, 0));
}

#[tokio::test]
async fn a_silent_server_times_out_without_switching_transport() {
    let mut h = fixture(
        Server {
            stall: true,
            ..Server::default()
        },
        None,
    )
    .await;
    h.provider.timeouts.headers = Duration::from_millis(100);

    let chunks = collect(&h.provider, &request(), &key()).await;

    let timed_out =
        |chunk: &Result<Chunk, Error>| matches!(chunk, Err(Error::Transport(message)) if message.contains("no event"));
    assert!(chunks.iter().any(timed_out));
    assert_eq!((h.server.requests(), h.server.posts()), (1, 0));
}
