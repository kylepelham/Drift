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

/// v2's `credential` table, with rows written by v2's import of `auth.json` and by signing in again since.
fn write_credentials(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE credential(
             id TEXT PRIMARY KEY, integration_id TEXT, label TEXT NOT NULL, value TEXT NOT NULL,
             connector_id TEXT, method_id TEXT, active INTEGER, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
         );",
    )
    .unwrap();
    let rows = [
        ("c1", Some("anthropic"), json!({ "type": "key", "key": "old" }), None, 1),
        (
            "c2",
            Some("anthropic"),
            json!({ "type": "oauth", "methodID": "oauth", "access": "a2", "refresh": "r2", "expires": 9 }),
            Some(true),
            2,
        ),
        (
            "c3",
            Some("anthropic"),
            json!({ "type": "key", "key": "unselected" }),
            Some(false),
            3,
        ),
        (
            "c4",
            Some("openai"),
            json!({
                "type": "oauth", "methodID": "chatgpt-browser", "access": "o", "refresh": "or", "expires": 5,
                "metadata": { "accountID": "acc" },
            }),
            None,
            1,
        ),
        (
            "c5",
            Some("azure"),
            json!({ "type": "external", "methodID": "azure-cli" }),
            None,
            1,
        ),
        ("c6", None, json!({ "type": "key", "key": "connector" }), None, 1),
    ];
    for (id, provider, value, active, created) in rows {
        conn.execute(
            "INSERT INTO credential VALUES(?1, ?2, 'API key', ?3, NULL, NULL, ?4, ?5, ?5)",
            rusqlite::params![id, provider, value.to_string(), active, created],
        )
        .unwrap();
    }
}

#[test]
fn v2_sign_ins_come_from_its_credential_table_over_auth_json() {
    let fixture = Fixture::new();
    let data = fixture.dir.0.join("opencode");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("auth.json"),
        json!({ "anthropic": { "type": "api", "key": "stale" }, "zai": { "type": "api", "key": "z" } }).to_string(),
    )
    .unwrap();
    write_credentials(&data.join("opencode.db"));
    let mut settings = settings(&fixture.dir.0, json!({}), json!({}), vec![]);
    settings.auth = sign_ins(&data);

    let report = fixture.import(&fixture.dir.0, &settings);

    assert_eq!(report.credentials, ["anthropic", "openai", "zai"]);
    assert_eq!(
        fixture.engine.credentials.get("anthropic"),
        Some(Credential::OAuth {
            access: "a2".into(),
            refresh: "r2".into(),
            expires_at: 9,
            account: None,
        }),
        "the active credential, not auth.json's older key"
    );
    assert!(
        matches!(fixture.engine.credentials.get("openai"), Some(Credential::OAuth { account: Some(account), .. }) if account == "acc")
    );
    assert_eq!(report.left_out.sign_ins, ["azure"], "a CLI's own sign-in is named");
    assert_eq!(
        sign_ins(&fixture.dir.0.join("nothing")),
        None,
        "no file and no database"
    );
}
