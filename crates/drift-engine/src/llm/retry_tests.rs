use std::time::{Duration, SystemTime};

use super::{Error, STREAMED};

fn headers(pairs: &[(&'static str, String)]) -> ::http::HeaderMap {
    pairs
        .iter()
        .map(|(name, value)| (::http::HeaderName::from_static(name), value.parse().unwrap()))
        .collect()
}

fn wait(error: &Error) -> Option<Duration> {
    let Error::Api { retry_after, .. } = error else {
        panic!()
    };

    *retry_after
}

#[test]
fn faults_inside_a_stream_retry_by_name_and_request_errors_do_not() {
    for kind in [
        "overloaded_error",
        "rate_limit_error",
        "api_error",
        "server_error",
        "UNAVAILABLE",
        "RESOURCE_EXHAUSTED",
    ] {
        assert!(
            matches!(Error::api(STREAMED, kind, "x"), Error::Api { retryable: true, .. }),
            "{kind}"
        );
    }

    for kind in [
        "invalid_request_error",
        "authentication_error",
        "insufficient_quota",
        "INVALID_ARGUMENT",
    ] {
        assert!(
            matches!(Error::api(STREAMED, kind, "x"), Error::Api { retryable: false, .. }),
            "{kind}"
        );
    }

    assert!(
        matches!(Error::api(400, "api_error", "x"), Error::Api { retryable: false, .. }),
        "a status decides when there is one"
    );
    assert!(matches!(
        Error::api(529, "anything", "x"),
        Error::Api { retryable: true, .. }
    ));
}

#[test]
fn a_spent_quota_never_retries_whatever_its_status_or_headers_say() {
    let spent = Error::api(429, "insufficient_quota", "You exceeded your current quota");
    assert!(matches!(spent, Error::Api { retryable: false, .. }));

    let told = spent.with_headers(&headers(&[
        ("x-should-retry", "true".into()),
        ("retry-after", "1".into()),
    ]));
    assert!(matches!(told, Error::Api { retryable: false, .. }));
    assert!(matches!(
        Error::api(STREAMED, "billing_hard_limit_reached", "x"),
        Error::Api { retryable: false, .. }
    ));
    assert!(
        matches!(
            Error::api(429, "rate_limit_exceeded", "x"),
            Error::Api { retryable: true, .. }
        ),
        "an ordinary rate limit still retries"
    );
}

#[test]
fn the_providers_wait_is_read_in_every_form() {
    let busy = || Error::api(429, "rate_limit_error", "slow down");
    assert_eq!(
        wait(&busy().with_headers(&headers(&[
            ("retry-after-ms", "1500".into()),
            ("retry-after", "9".into())
        ]))),
        Some(Duration::from_millis(1500)),
        "ms wins"
    );
    assert_eq!(
        wait(&busy().with_headers(&headers(&[("retry-after", "7".into())]))),
        Some(Duration::from_secs(7))
    );

    let later = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
    let dated = wait(&busy().with_headers(&headers(&[("retry-after", later)]))).unwrap();
    assert!(
        dated > Duration::from_secs(110) && dated <= Duration::from_secs(120),
        "{dated:?}"
    );

    let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(60));
    assert_eq!(
        wait(&busy().with_headers(&headers(&[("retry-after", past)]))),
        Some(Duration::ZERO)
    );
    assert_eq!(
        wait(&busy().with_headers(&headers(&[("retry-after", "soon".into())]))),
        None
    );

    for huge in ["1e300", "18446744073709551616", "inf"] {
        assert_eq!(
            wait(&busy().with_headers(&headers(&[("retry-after", huge.into())]))),
            Some(Duration::MAX),
            "{huge} saturates"
        );
    }
    for huge in ["1e300", "inf"] {
        assert_eq!(
            wait(&busy().with_headers(&headers(&[("retry-after-ms", huge.into())]))),
            Some(Duration::MAX),
            "{huge} ms saturates"
        );
    }

    assert!(matches!(
        busy().with_headers(&headers(&[("x-should-retry", "false".into())])),
        Error::Api { retryable: false, .. }
    ));
    assert!(matches!(
        Error::api(400, "x", "y").with_headers(&headers(&[("x-should-retry", "true".into())])),
        Error::Api { retryable: true, .. }
    ));
}
