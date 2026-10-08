use super::*;

fn auth() -> Value {
    json!({
        "anthropic": { "type": "oauth", "access": "a", "refresh": "r", "expires": 123 },
        "openai": { "type": "oauth", "access": "a", "refresh": "r", "expires": 1, "accountId": "acc" },
        "xai": { "type": "oauth", "access": "x", "refresh": "xr", "expires": 7 },
        "github-copilot": { "type": "oauth", "access": "a", "refresh": "r", "expires": 1 },
        "zai": { "type": "api", "key": "z-key" },
        "nvidia": { "type": "api", "key": "n-key" },
        "lmstudio": { "type": "api", "key": "lm" },
    })
}

fn assert_imported_credentials(engine: &Engine) {
    assert_eq!(
        engine.credentials.get("xai"),
        Some(Credential::OAuth {
            access: "x".into(),
            refresh: "xr".into(),
            expires_at: 7,
            account: None,
        }),
        "SuperGrok, which Drift renews"
    );
    assert_eq!(
        engine.credentials.get("anthropic"),
        Some(Credential::OAuth {
            access: "a".into(),
            refresh: "r".into(),
            expires_at: 123,
            account: None,
        })
    );
    assert_eq!(
        engine.credentials.get("openai"),
        Some(Credential::ApiKey { key: "mine".into() }),
        "a sign-in made in Drift stays"
    );
    assert_eq!(
        engine.credentials.get("zai"),
        Some(Credential::ApiKey { key: "z-key".into() })
    );
}

#[test]
fn keys_and_renewable_sign_ins_come_in_once_and_never_over_drifts_own() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    engine
        .credentials
        .set("openai", &Credential::ApiKey { key: "mine".into() })
        .unwrap();
    let settings = settings(&fixture.dir.0, auth(), json!({}), vec![]);

    let report = fixture.import(&fixture.dir.0, &settings);

    assert_eq!(report.credentials, ["anthropic", "xai", "zai"]);
    assert_imported_credentials(engine);
    assert_eq!(
        report.left_out.sign_ins,
        ["github-copilot", "nvidia"],
        "a sign-in Drift already had is kept, not left out"
    );
    let log = report.skipped.join("\n");
    assert!(
        log.contains("openai: already signed in")
            && log.contains("github-copilot: Drift cannot renew")
            && log.contains("nvidia: Drift has no provider")
            && !log.contains("lmstudio"),
        "{log}"
    );

    engine.credentials.remove("anthropic").unwrap();
    let again = fixture.import(&fixture.dir.0, &settings);

    assert_eq!((again.credentials.len(), again.skipped.len()), (0, 0), "{again:?}");
    assert!(
        engine.credentials.get("anthropic").is_none(),
        "a sign-in the user removed stays removed"
    );
}
