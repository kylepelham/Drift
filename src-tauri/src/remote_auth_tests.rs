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
        (
            AuthError::UnknownTicket,
            "That pairing code has expired or was already used. Show a new one on the PC.",
        ),
    ];

    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn a_pairing_code_signs_in_one_app_once_and_a_new_code_replaces_the_last() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let replaced = auth.new_ticket();
    let ticket = auth.new_ticket();

    assert!(matches!(
        auth.redeem(&replaced, "Pixel".into(), &store),
        Err(AuthError::UnknownTicket)
    ));
    let token = auth.redeem(&ticket, "Pixel".into(), &store).unwrap();
    assert!(
        matches!(
            auth.redeem(&ticket, "Pixel".into(), &store),
            Err(AuthError::UnknownTicket)
        ),
        "a code works once"
    );

    let device = auth.device(&token, &store).unwrap();
    assert_eq!((device.name.as_str(), device.method.as_str()), ("Pixel", "app"));
    assert!(auth.device("not-a-token", &store).is_none());
    assert_device_persisted_without_token(&store, &device);

    let expired = auth.new_ticket();
    auth.ticket.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    assert!(matches!(
        auth.redeem(&expired, "Pixel".into(), &store),
        Err(AuthError::UnknownTicket)
    ));

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
    for path in ["/auth/options", "/auth/login", "/auth/pair", "/auth/certificate"] {
        assert!(public_path(path), "{path}");
    }

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
        "/auth/pair",
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
