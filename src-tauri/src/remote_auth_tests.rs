use super::*;

fn store() -> (Store, std::path::PathBuf) {
    let directory = drift_engine::platform::temp::test_run().join(format!("drift-remote-auth-{}", random_hex(8)));
    (crate::store::open(&directory).unwrap(), directory)
}

fn address(last: u8) -> IpAddr {
    IpAddr::from([192, 168, 1, last])
}

#[test]
fn password_hashes_verify_only_the_original_password() {
    let hash = hash_password("correct horse", b"0123456789abcdef", 1_000);
    assert!(hash.starts_with("pbkdf2-sha256$1000$"));
    assert!(verify_password("correct horse", &hash));
    assert!(!verify_password("correct horsf", &hash));

    for malformed in [
        "",
        "pbkdf2-sha256$0$AA$AA",
        "md5$1000$AA$AA",
        "pbkdf2-sha256$x$AA$AA",
        "a$b$c",
    ] {
        assert!(!verify_password("correct horse", malformed), "{malformed}");
    }

    let vector = hash_password("passwd", b"salt", 1);
    let key = STANDARD_NO_PAD.decode(vector.rsplit('$').next().unwrap()).unwrap();
    let mut hex = String::with_capacity(key.len() * 2);
    for byte in key {
        write!(hex, "{byte:02x}").unwrap();
    }

    assert_eq!(hex, "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc");
    assert_ne!(new_password_hash("same"), new_password_hash("same"));
}

#[test]
fn credentials_have_sane_bounds() {
    assert!(validate_credentials("kyle", "longenough").is_ok());
    assert!(validate_credentials("  ", "longenough").is_err());
    assert!(validate_credentials(&"u".repeat(65), "longenough").is_err());
    assert!(validate_credentials("kyle", "short").is_err());
}

#[test]
fn auth_errors_keep_the_command_boundary_text() {
    let cases = [
        (AuthError::InvalidUsername, "Username must be 1 to 64 characters."),
        (AuthError::InvalidPassword, "Password must be 8 to 256 characters."),
    ];

    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn a_device_asking_to_connect_is_shown_once_it_sent_its_secret_with_the_code_its_screen_shows() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let now = Instant::now();
    let (authority, device_secret, computer_secret) = ([7u8; 32], [1u8; 32], [2u8; 32]);

    let (id, commitment) = auth
        .requests
        .open("Pixel".into(), address(20), computer_secret, now)
        .unwrap();
    assert_eq!(commitment, drift_pairing::commitment(&computer_secret));
    assert!(
        auth.prompts().is_empty(),
        "nothing is asked before the device sent its secret"
    );

    let revealed = auth.requests.reveal(&id, &device_secret, &authority, now).unwrap();
    assert_eq!(revealed, computer_secret);
    assert_eq!(
        auth.requests.reveal(&id, &[9u8; 32], &authority, now),
        Err(requests::RequestError::Unknown),
        "the device's secret is taken once, so it cannot be changed to fit"
    );
    let prompts = auth.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        (prompts[0].name.as_str(), prompts[0].address.as_str()),
        ("Pixel", "192.168.1.20")
    );
    assert_eq!(
        prompts[0].code,
        drift_pairing::matching_code(&authority, &device_secret, &computer_secret)
    );

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_accepted_device_is_signed_in_once() {
    use requests::Answer;

    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let now = Instant::now();
    let (id, _) = auth.requests.open("Pixel".into(), address(20), [2u8; 32], now).unwrap();
    auth.requests.reveal(&id, &[1u8; 32], &[7u8; 32], now).unwrap();

    let answer = auth.requests.watch(&id).unwrap();
    assert!(auth.requests.settle(&id).is_none(), "still waiting");
    assert!(auth.answer(&id, true));
    assert_eq!(*answer.borrow(), Answer::Accepted);
    assert!(
        auth.prompts().is_empty(),
        "an answered request is no longer asked about"
    );
    let (settled, name) = auth.requests.settle(&id).unwrap();
    assert_eq!((settled, name.as_str()), (Answer::Accepted, "Pixel"));
    assert!(auth.requests.watch(&id).is_none(), "settled once");

    let token = auth.create_device(name, "app", &store).unwrap();
    let device = auth.device(&token, &store).unwrap();
    assert_eq!((device.name.as_str(), device.method.as_str()), ("Pixel", "app"));
    assert!(auth.device("not-a-token", &store).is_none());
    assert_device_persisted_without_token(&store, &device);

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn requests_are_declined_replaced_limited_and_expire() {
    use requests::{Answer, REQUEST_TTL, RequestError};

    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let now = Instant::now();
    let open = |auth: &mut Auth, last: u8, at: Instant| {
        auth.requests
            .open("Phone".into(), address(last), [0u8; 32], at)
            .map(|(id, _)| id)
    };

    let old = open(&mut auth, 30, now).unwrap();
    let replaced = auth.requests.watch(&old).unwrap();
    let new = open(&mut auth, 30, now).unwrap();
    assert_eq!(
        *replaced.borrow(),
        Answer::Gone,
        "a newer request from the same address replaces it"
    );

    auth.requests.reveal(&new, &[1u8; 32], &[7u8; 32], now).unwrap();
    assert!(auth.answer(&new, false));
    assert_eq!(
        auth.requests.settle(&new).map(|(answer, _)| answer),
        Some(Answer::Declined)
    );
    assert!(
        auth.failures.contains_key(&address(30)),
        "a decline counts toward the lockout"
    );
    assert!(!auth.answer("never-asked", true));

    let waiting: Vec<String> = (40..43).map(|last| open(&mut auth, last, now).unwrap()).collect();
    assert_eq!(open(&mut auth, 43, now), Err(RequestError::TooMany));
    assert!(auth.requests.cancel(&waiting[0]));
    open(&mut auth, 43, now).unwrap();

    let expired = auth.requests.watch(&waiting[1]).unwrap();
    open(&mut auth, 50, now + REQUEST_TTL).unwrap();
    assert_eq!(*expired.borrow(), Answer::Gone);
    assert!(auth.requests.watch(&waiting[1]).is_none());

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

fn assert_device_persisted_without_token(store: &Store, device: &RemoteDevice) {
    let reloaded = Auth::load(store).unwrap().devices();
    assert_eq!(reloaded, vec![device.clone()]);
    assert!(!serde_json::to_string(&reloaded).unwrap().contains(&device.token_hash));
}

#[test]
fn revoking_signs_out_one_device_or_all() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let first = auth.create_device("One".into(), "link", &store).unwrap();
    let second = auth.create_device("Two".into(), "link", &store).unwrap();
    let id = auth.device(&first, &store).unwrap().id;

    auth.revoke(Some(&id), &store).unwrap();
    assert!(auth.device(&first, &store).is_none());
    assert!(auth.device(&second, &store).is_some());

    auth.revoke(None, &store).unwrap();
    assert!(auth.devices().is_empty());
    assert!(Auth::load(&store).unwrap().devices().is_empty());

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn changing_the_password_signs_out_only_password_sessions() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    auth.set_password(Some(("kyle".into(), hash_password("longenough", b"salt", 1))), &store)
        .unwrap();
    let linked = auth.create_device("Linked".into(), "link", &store).unwrap();
    let signed_in = auth.create_device("Signed in".into(), "password", &store).unwrap();
    assert_eq!(Auth::load(&store).unwrap().password_username().as_deref(), Some("kyle"));

    auth.set_password(None, &store).unwrap();
    assert!(auth.device(&linked, &store).is_some());
    assert!(auth.device(&signed_in, &store).is_none());
    assert!(Auth::load(&store).unwrap().password_username().is_none());

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_password_verified_before_a_change_is_no_longer_current() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    auth.set_password(Some(("kyle".into(), hash_password("first-pass", b"salt", 1))), &store)
        .unwrap();
    let verified = auth.password.clone().unwrap();
    assert!(auth.password_is_current(&verified));

    auth.set_password(Some(("kyle".into(), hash_password("second-pass", b"salt", 1))), &store)
        .unwrap();
    assert!(
        !auth.password_is_current(&verified),
        "a rotated password rejects the in-flight check"
    );

    auth.set_password(None, &store).unwrap();
    assert!(
        !auth.password_is_current(&verified),
        "turning sign-in off rejects it too"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn repeated_failures_lock_an_address_with_growing_delays() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let now = Instant::now();

    for _ in 0..FREE_FAILURES - 1 {
        auth.fail(address(40), now);
    }
    assert!(auth.locked_for(address(40), now).is_none());

    auth.fail(address(40), now);
    let first = auth.locked_for(address(40), now).unwrap();
    auth.fail(address(40), now);
    assert!(auth.locked_for(address(40), now).unwrap() > first);
    assert!(
        auth.locked_for(address(41), now).is_none(),
        "other addresses are unaffected"
    );

    for _ in 0..20 {
        auth.fail(address(40), now);
    }
    assert!(auth.locked_for(address(40), now).unwrap() <= MAX_LOCK);
    assert!(
        auth.locked_for(address(40), now + MAX_LOCK + Duration::from_secs(1))
            .is_none()
    );
    auth.fail(address(41), now + FORGET_FAILURES);
    assert_eq!(auth.failures.len(), 1, "an address quiet for an hour is forgotten");

    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn session_tokens_come_from_bearer_or_a_cookie_while_browsers_may_sign_in() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
    assert_eq!(supplied_token(&headers, false), Some("abc"));
    headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
    assert_eq!(supplied_token(&headers, true), None);
    headers.insert(header::COOKIE, HeaderValue::from_static("theme=dark; drift_remote=xyz"));
    assert_eq!(supplied_token(&headers, true), Some("xyz"));
    assert_eq!(
        supplied_token(&headers, false),
        None,
        "browser access off refuses browser sessions"
    );

    let cookie = session_cookie("abc");
    let cookie = cookie.to_str().unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age="] {
        assert!(cookie.contains(attribute), "{attribute}");
    }
}

#[test]
fn only_sign_in_routes_are_public() {
    for path in [
        "/auth/options",
        "/auth/login",
        "/auth/request",
        "/auth/request/wait",
        "/auth/certificate",
    ] {
        assert!(public_path(path), "{path}");
    }
    assert!(request_path("/auth/request") && request_path("/auth/request/reveal"));
    assert!(!request_path("/auth/requested") && !request_path("/auth/login"));

    for path in [
        "/auth/logout",
        "/auth/me",
        "/engine/session",
        "/api/invoke",
        "/companion",
        "/assets/app.js",
    ] {
        assert!(!public_path(path), "{path}");
    }
    assert!(sign_in_path("/") && sign_in_path("/companion") && sign_in_path("/companion/x"));
    assert!(!sign_in_path("/assets/app.js") && !sign_in_path("/engine"));
}

#[test]
fn only_the_browser_version_and_its_sign_in_close_with_browser_access() {
    for path in ["/", "/companion", "/assets/app.js", "/auth/login", "/auth/certificate"] {
        assert!(browser_path(path), "{path}");
    }
    for path in [
        "/auth/options",
        "/auth/request",
        "/auth/me",
        "/auth/logout",
        "/engine/session",
        "/api/invoke",
        "/api/ui-state/events",
    ] {
        assert!(!browser_path(path), "{path}");
    }
}

#[test]
fn device_names_are_bounded_and_printable() {
    assert_eq!(device_name(None), "Browser");
    assert_eq!(device_name(Some("  \u{7}  ".into())), "Browser");
    assert_eq!(device_name(Some("Pixel\n8".into())), "Pixel8");
    assert_eq!(device_name(Some("x".repeat(100))).chars().count(), 60);
}

#[test]
fn the_sign_in_page_is_self_contained() {
    assert!(!SIGN_IN_PAGE.contains("http://") && !SIGN_IN_PAGE.contains("https://"));
    assert!(!SIGN_IN_PAGE.contains('\u{2014}'));
    for route in ["/auth/login", "/auth/options", "/auth/certificate"] {
        assert!(SIGN_IN_PAGE.contains(route), "{route}");
    }
    assert!(
        !SIGN_IN_PAGE.contains("/auth/link"),
        "devices no longer link by typed code"
    );
}
