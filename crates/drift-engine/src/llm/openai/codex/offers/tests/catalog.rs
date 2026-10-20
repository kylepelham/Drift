//! What the account's offers do to the catalog a ChatGPT sign-in sees.

use super::*;

#[test]
fn speed_modes_the_account_does_not_offer_are_removed_and_offered_ones_stay() {
    let credential = credential("user", "account");
    let mut provider = provider();

    cached(&credential).apply(&mut provider, std::slice::from_ref(&credential));

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
fn the_list_says_which_models_load_tools_through_tool_search() {
    let credential = credential("user", "account");
    let mut provider = provider();
    provider.models.get_mut("gpt-6-astra").unwrap().tool_search = true;

    cached(&credential).apply(&mut provider, std::slice::from_ref(&credential));

    let searches = |id: &str| provider.models[id].tool_search;
    assert!(searches("gpt-6-sol") && searches("gpt-6-sol-fast") && searches("gpt-6-sol-daybreak"));
    assert!(!searches("gpt-6-astra"), "the list's word replaces the catalog's");
}

#[test]
fn models_the_list_does_not_name_keep_every_speed_mode() {
    let credential = credential("user", "account");
    let offers = Offers::default();
    offers.state.write().unwrap().cached.insert(
        identity(&credential).unwrap(),
        Cached {
            until: Instant::now() + TTL,
            offers: list::read(json!({ "models": [{ "slug": "gpt-6-astra" }] })),
        },
    );
    let mut provider = provider();

    offers.apply(&mut provider, std::slice::from_ref(&credential));

    assert!(provider.models.contains_key("gpt-6-sol-ultrafast"));
}

#[test]
fn daybreak_entries_copy_their_model_and_select_only_an_offered_program() {
    let credential = credential("user", "account");
    let offers = cached(&credential);
    let mut provider = provider();
    let mut ordinary = provider.models.clone();
    // The list's word on tool search is the one change it makes to a plain entry.
    ordinary.get_mut("gpt-6-sol").unwrap().tool_search = true;

    offers.apply(&mut provider, std::slice::from_ref(&credential));

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
    offers.apply(&mut provider, std::slice::from_ref(&credential));
    assert_eq!(provider, once);
}

#[test]
fn a_daybreak_copy_of_a_speed_mode_keeps_its_tier_and_headers() {
    let credential = credential("user", "account");
    let mut provider = provider();

    cached(&credential).apply(&mut provider, std::slice::from_ref(&credential));

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
        offers.apply(&mut provider, std::slice::from_ref(&other));
        assert_eq!(provider, self::provider());
    }

    offers.state.write().unwrap().cached.values_mut().next().unwrap().until = Instant::now() - RETRY;
    let mut provider = provider();
    offers.apply(&mut provider, std::slice::from_ref(&owner));
    assert_eq!(provider, self::provider(), "an expired list changes nothing");

    assert!(offers.clear());
    assert!(offers.state.read().unwrap().cached.is_empty());
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

#[test]
fn models_any_account_offers_are_listed_and_each_account_answers_for_its_own() {
    let (ann, bob) = (credential("ann", "account-a"), credential("bob", "account-b"));
    let offers = Offers::default();
    let without_programs = json!({ "models": [
        { "slug": "gpt-6-sol", "available_access_programs": { "cyber": ["standard"] } }
    ] });
    for (credential, listed) in [(&ann, without_programs), (&bob, list())] {
        offers.state.write().unwrap().cached.insert(
            identity(credential).unwrap(),
            Cached {
                until: Instant::now() + TTL,
                offers: list::read(listed),
            },
        );
    }

    // Ann's account has no Daybreak, Bob's does: the catalog lists it either way.
    let mut provider = provider();
    offers.apply(&mut provider, &[ann.clone(), bob.clone()]);
    let daybreak = provider.models["gpt-6-sol-daybreak"].clone();
    assert_eq!(
        daybreak.mode.as_ref().unwrap().body["access_programs"]["cyber"],
        "daybreak_blue"
    );

    // Only Bob can be sent the Daybreak entry; the plain model is not a question of offers.
    assert_eq!(offers.serves(&ann, &daybreak), Some(false));
    assert_eq!(offers.serves(&bob, &daybreak), Some(true));
    assert_eq!(offers.serves(&ann, &provider.models["gpt-6-sol"]), None);

    // A request carries the program of the account it goes to.
    let mut sent = daybreak.clone();
    sent.mode
        .as_mut()
        .unwrap()
        .body
        .insert("access_programs".into(), json!({ "cyber": "daybreak_red" }));
    offers.fit(&mut sent, &bob);
    assert_eq!(sent.mode.unwrap().body["access_programs"]["cyber"], "daybreak_blue");
}
