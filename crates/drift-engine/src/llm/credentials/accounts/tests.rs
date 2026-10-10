use super::*;

fn store() -> (Credentials, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("drift-accounts-{}.json", crate::random_hex(4)));

    (Credentials::in_file(path.clone()), path)
}

fn signed_in(access: &str) -> Credential {
    Credential::OAuth {
        access: access.into(),
        refresh: format!("{access}-refresh"),
        expires_at: i64::MAX,
        account: None,
    }
}

fn person(identity: &str) -> Profile {
    Profile {
        identity: Some(identity.into()),
        email: Some(format!("{identity}@example.com")),
    }
}

fn keys(store: &Credentials, provider: &str) -> Vec<String> {
    store
        .accounts(provider)
        .into_iter()
        .map(|account| account.key)
        .collect()
}

#[test]
fn a_second_person_signing_in_joins_the_end_of_the_list() {
    let (store, path) = store();

    let first = store.add_account("openai", &signed_in("a"), &person("ann")).unwrap();
    let second = store.add_account("openai", &signed_in("b"), &person("bob")).unwrap();

    assert_eq!(first, "openai", "the first account keeps the provider's own key");
    assert!(second.starts_with("openai~"), "{second}");
    assert_eq!(keys(&store, "openai"), [first.clone(), second.clone()]);
    assert_eq!(
        store.get("openai"),
        Some(signed_in("a")),
        "the first account is the one used"
    );
    assert_eq!(store.account(&second), Some(signed_in("b")));
    assert_eq!(store.accounts("openai")[1].label.as_deref(), Some("bob@example.com"));
    assert_eq!(store.providers(), ["openai"]);

    std::fs::remove_file(path).ok();
}

#[test]
fn signing_in_as_someone_listed_replaces_their_account_in_place() {
    let (store, path) = store();
    store.add_account("openai", &signed_in("a"), &person("ann")).unwrap();
    let bob = store.add_account("openai", &signed_in("b"), &person("bob")).unwrap();

    let again = store.add_account("openai", &signed_in("b2"), &person("bob")).unwrap();

    assert_eq!(again, bob);
    assert_eq!(store.accounts("openai").len(), 2);
    assert_eq!(store.account(&bob), Some(signed_in("b2")));

    std::fs::remove_file(path).ok();
}

#[test]
fn an_api_key_replaces_every_sign_in_and_a_sign_in_replaces_a_key() {
    let (store, path) = store();
    store.add_account("openai", &signed_in("a"), &person("ann")).unwrap();
    let bob = store.add_account("openai", &signed_in("b"), &person("bob")).unwrap();

    let key = Credential::ApiKey { key: "sk".into() };
    store.add_account("openai", &key, &Profile::default()).unwrap();
    assert_eq!(keys(&store, "openai"), ["openai"]);
    assert_eq!(store.get("openai"), Some(key));
    assert!(
        store.account(&bob).is_none(),
        "the replaced account's secret is deleted"
    );

    store.add_account("openai", &signed_in("c"), &person("cat")).unwrap();
    assert_eq!(keys(&store, "openai"), ["openai"]);
    assert_eq!(store.get("openai"), Some(signed_in("c")));

    std::fs::remove_file(path).ok();
}

#[test]
fn a_credential_saved_before_accounts_is_the_first_account_and_is_recognised() {
    let (store, path) = store();
    let token = |sub: &str| {
        let claims = format!(r#"{{"sub":"{sub}","email":"{sub}@example.com"}}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims);
        signed_in(&format!("h.{payload}.s"))
    };
    store
        .write("xai", &serde_json::to_string(&token("ann")).unwrap())
        .unwrap();

    assert_eq!(keys(&store, "xai"), ["xai"]);
    assert_eq!(store.resolve_account("xai", &[]).unwrap().0.as_deref(), Some("xai"));

    let profile = Profile::from_jwt(match &token("ann") {
        Credential::OAuth { access, .. } => access,
        _ => unreachable!(),
    });
    store.add_account("xai", &token("ann"), &profile).unwrap();
    assert_eq!(
        keys(&store, "xai"),
        ["xai"],
        "the same person signing in again is not a second account"
    );

    std::fs::remove_file(path).ok();
}

#[test]
fn accounts_reorder_rename_and_sign_out_one_at_a_time() {
    let (store, path) = store();
    let ann = store.add_account("anthropic", &signed_in("a"), &person("ann")).unwrap();
    let bob = store.add_account("anthropic", &signed_in("b"), &person("bob")).unwrap();

    assert!(
        !store.reorder_accounts("anthropic", std::slice::from_ref(&bob)).unwrap(),
        "every account must be named"
    );
    assert!(
        store
            .reorder_accounts("anthropic", &[bob.clone(), ann.clone()])
            .unwrap()
    );
    assert_eq!(store.get("anthropic"), Some(signed_in("b")));

    assert!(store.rename_account("anthropic", &bob, " Work ").unwrap());
    assert_eq!(store.accounts("anthropic")[0].label.as_deref(), Some("Work"));

    assert!(store.remove_account("anthropic", &bob).unwrap());
    assert_eq!(keys(&store, "anthropic"), std::slice::from_ref(&ann));
    assert_eq!(store.get("anthropic"), Some(signed_in("a")));

    store.remove("anthropic").unwrap();
    assert!(store.accounts("anthropic").is_empty());
    assert!(store.account(&ann).is_none());
    assert!(store.providers().is_empty());

    std::fs::remove_file(path).ok();
}

#[test]
fn the_lists_survive_reopening() {
    let (store, path) = store();
    store.add_account("openai", &signed_in("a"), &person("ann")).unwrap();
    store.add_account("openai", &signed_in("b"), &person("bob")).unwrap();

    let reopened = Credentials::in_file(path.clone());

    assert_eq!(reopened.accounts("openai"), store.accounts("openai"));

    std::fs::remove_file(path).ok();
}

#[test]
fn a_chatgpt_token_names_the_account_and_the_user() {
    let claims = r#"{"sub":"auth0|x","https://api.openai.com/auth":{"chatgpt_account_id":"acc","chatgpt_user_id":"user-1"},"https://api.openai.com/profile":{"email":"ann@example.com"}}"#;
    let token = format!(
        "h.{}.s",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims)
    );

    let profile = Profile::from_jwt(&token);

    assert_eq!(profile.identity.as_deref(), Some("acc/user-1"));
    assert_eq!(profile.email.as_deref(), Some("ann@example.com"));
    assert_eq!(Profile::from_jwt("opaque-token"), Profile::default());
}
