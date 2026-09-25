use super::*;

fn store() -> (Store, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!("drift-remote-auth-{}", random_hex(8)));
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
    for malformed in ["", "pbkdf2-sha256$0$AA$AA", "md5$1000$AA$AA", "pbkdf2-sha256$x$AA$AA", "a$b$c"] {
        assert!(!verify_password("correct horse", malformed), "{malformed}");
    }
    let vector = hash_password("passwd", b"salt", 1);
    let key = STANDARD_NO_PAD.decode(vector.rsplit('$').next().unwrap()).unwrap();
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
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
fn codes_avoid_look_alikes_and_normalize_for_typing() {
    for _ in 0..200 {
        let code = random_code();
        assert_eq!(code.len(), CODE_LENGTH);
        assert!(code.bytes().all(|byte| CODE_ALPHABET.contains(&byte)), "{code}");
    }
    assert_eq!(display_code("ABCDEFGH"), "ABCD-EFGH");
    assert_eq!(normalize_code(" abcd-efgh "), "ABCDEFGH");
}

#[test]
fn a_device_links_only_after_its_code_is_entered_on_the_desktop() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    let (handle, code) = auth.request_link(address(20), "Android Chrome".into()).unwrap();
    let pending = auth.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "Android Chrome");
    assert!(!serde_json::to_string(&pending).unwrap().contains(&code));
    assert!(matches!(auth.poll(&handle, &store).unwrap(), Poll::Pending));
    assert!(auth.approve("WRONG123").is_err());
    let typed = format!("{}-{}", &code[..4].to_lowercase(), &code[4..]);
    assert_eq!(auth.approve(&typed).unwrap(), "Android Chrome");
    assert!(auth.approve(&code).is_err(), "a code approves once");
    assert!(auth.pending().is_empty());
    let Poll::Approved(token) = auth.poll(&handle, &store).unwrap() else { panic!("expected approval") };
    assert!(matches!(auth.poll(&handle, &store).unwrap(), Poll::Expired), "the token is delivered once");
    let device = auth.device(&token, &store).unwrap();
    assert_eq!((device.name.as_str(), device.method.as_str()), ("Android Chrome", "link"));
    assert!(auth.device("not-a-token", &store).is_none());
    let reloaded = Auth::load(&store).unwrap().devices();
    assert_eq!(reloaded, vec![device.clone()]);
    assert!(!serde_json::to_string(&reloaded).unwrap().contains(&device.token_hash));
    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn link_requests_expire_and_are_capped_per_address() {
    let (store, directory) = store();
    let mut auth = Auth::load(&store).unwrap();
    for _ in 0..MAX_LINKS_PER_ADDRESS {
        auth.request_link(address(30), "Phone".into()).unwrap();
    }
    assert!(auth.request_link(address(30), "Phone".into()).is_err());
    let (handle, code) = auth.request_link(address(31), "Tablet".into()).unwrap();
    auth.prune(Instant::now() + LINK_TTL + Duration::from_secs(1));
    assert!(auth.links.is_empty());
    assert!(auth.approve(&code).is_err());
    assert!(matches!(auth.poll(&handle, &store).unwrap(), Poll::Expired));
    drop(store);
    let _ = std::fs::remove_dir_all(directory);
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
    auth.set_password(Some(("kyle".into(), hash_password("longenough", b"salt", 1))), &store).unwrap();
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
    assert!(auth.locked_for(address(41), now).is_none(), "other addresses are unaffected");
    for _ in 0..20 {
        auth.fail(address(40), now);
    }
    assert!(auth.locked_for(address(40), now).unwrap() <= MAX_LOCK);
    assert!(auth.locked_for(address(40), now + MAX_LOCK + Duration::from_secs(1)).is_none());
    drop(store);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn session_tokens_come_from_bearer_or_cookie_only() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
    assert_eq!(supplied_token(&headers), Some("abc"));
    headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
    assert_eq!(supplied_token(&headers), None);
    headers.insert(header::COOKIE, HeaderValue::from_static("theme=dark; drift_remote=xyz"));
    assert_eq!(supplied_token(&headers), Some("xyz"));
    let cookie = session_cookie("abc");
    let cookie = cookie.to_str().unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age="] {
        assert!(cookie.contains(attribute), "{attribute}");
    }
}

#[test]
fn only_sign_in_routes_are_public() {
    for path in ["/auth/options", "/auth/link", "/auth/link/abc", "/auth/login", "/auth/certificate"] {
        assert!(public_path(path), "{path}");
    }
    for path in ["/auth/logout", "/auth/me", "/engine/session", "/api/invoke", "/companion", "/assets/app.js"] {
        assert!(!public_path(path), "{path}");
    }
    assert!(sign_in_path("/") && sign_in_path("/companion") && sign_in_path("/companion/x"));
    assert!(!sign_in_path("/assets/app.js") && !sign_in_path("/engine"));
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
    for route in ["/auth/link", "/auth/login", "/auth/options", "/auth/certificate"] {
        assert!(SIGN_IN_PAGE.contains(route), "{route}");
    }
}
