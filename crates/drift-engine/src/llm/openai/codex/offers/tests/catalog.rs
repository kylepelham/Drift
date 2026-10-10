//! What the account's offers do to the catalog a ChatGPT sign-in sees.

use super::*;

#[test]
fn speed_modes_the_account_does_not_offer_are_removed_and_offered_ones_stay() {
    let credential = credential("user", "account");
    let mut provider = provider();

    cached(&credential).apply(&mut provider, &credential);

    assert!(
        provider.models.contains_key("gpt-6-sol-fast"),
        "the list offers priority"
    );
    assert!(
        !provider.models.contains_key("gpt-6-sol-ultrafast"),
        "the list does not offer ultrafast"
    );
    assert!(provider.models.contains_key("gpt-6-sol"), "plain models stay");
}

#[test]
fn models_the_list_does_not_name_keep_every_speed_mode() {
    let credential = credential("user", "account");
    let offers = Offers::default();
    offers.state.write().unwrap().cached = Some(Cached {
        identity: identity(&credential).unwrap(),
        until: Instant::now() + TTL,
        offers: list::read(json!({ "models": [{ "slug": "gpt-6-astra" }] })),
    });
    let mut provider = provider();

    offers.apply(&mut provider, &credential);

    assert!(provider.models.contains_key("gpt-6-sol-ultrafast"));
}

#[test]
fn daybreak_entries_copy_their_model_and_select_only_an_offered_program() {
    let credential = credential("user", "account");
    let offers = cached(&credential);
    let mut provider = provider();
    let ordinary = provider.models.clone();

    offers.apply(&mut provider, &credential);

    assert!(
        !provider.models.contains_key("gpt-6-astra-daybreak"),
        "Astra offers no program"
    );
    for (id, program) in [("gpt-6-sol", "daybreak_blue"), ("gpt-5.6-cyber", "daybreak_red")] {
        let base = &ordinary[id];
        let copy = &provider.models[&format!("{id}-daybreak")];

        assert_eq!(copy.wire(&copy.id), id);
        assert_eq!(copy.name, format!("{} Daybreak", base.name));
        assert_eq!(
            (&copy.limit, &copy.cost, &copy.variants, copy.profile),
            (&base.limit, &base.cost, &base.variants, base.profile)
        );
        assert_eq!(copy.mode.as_ref().unwrap().body["access_programs"]["cyber"], program);
        assert_eq!(provider.models[id], *base, "the plain entry is unchanged");
    }

    // Applying again adds nothing: a Daybreak entry is never copied.
    let once = provider.clone();
    offers.apply(&mut provider, &credential);
    assert_eq!(provider, once);
}

#[test]
fn a_daybreak_copy_of_a_speed_mode_keeps_its_tier_and_headers() {
    let credential = credential("user", "account");
    let mut provider = provider();

    cached(&credential).apply(&mut provider, &credential);

    let copy = &provider.models["gpt-6-sol-fast-daybreak"];
    let mode = copy.mode.as_ref().unwrap();
    assert_eq!(copy.wire(&copy.id), "gpt-6-sol");
    assert_eq!(mode.body["service_tier"], "priority");
    assert_eq!(mode.headers["x-test"], "fast");
    assert!(
        !provider.models.contains_key("gpt-6-sol-ultrafast-daybreak"),
        "a removed mode gets no copy"
    );
}

#[test]
fn offers_never_cross_users_accounts_api_keys_or_their_expiry() {
    let owner = credential("user", "account");
    let offers = cached(&owner);

    for other in [
        credential("another", "account"),
        credential("user", "another"),
        Credential::ApiKey { key: "key".into() },
    ] {
        let mut provider = provider();
        offers.apply(&mut provider, &other);
        assert_eq!(provider, self::provider());
    }

    offers.state.write().unwrap().cached.as_mut().unwrap().until = Instant::now() - RETRY;
    let mut provider = provider();
    offers.apply(&mut provider, &owner);
    assert_eq!(provider, self::provider(), "an expired list changes nothing");

    assert!(offers.clear());
    assert!(offers.state.read().unwrap().cached.is_none());
}

#[test]
fn a_refreshed_token_keeps_the_same_account_identity() {
    let before = credential("user", "account");
    let mut after = before.clone();
    if let Credential::OAuth { access, refresh, .. } = &mut after {
        access.push_str("new-signature");
        *refresh = "new-refresh".into();
    }

    assert_eq!(identity(&before), identity(&after));
}
