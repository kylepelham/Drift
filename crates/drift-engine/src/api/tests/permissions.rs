use super::*;

#[tokio::test]
async fn a_workspaces_kept_grants_are_listed_and_revoked() {
    let harness = harness().await;
    let (workspace_id, session_id) = session_with_model(&harness).await;
    harness.engine.bind_permissions(&session_id, &workspace_id);
    let request = crate::permission::new_request(
        &session_id,
        "m",
        "c",
        "bash",
        crate::tool::Ask::shell(crate::tool::command::Dialect::Bash, "cargo build", "cargo build"),
    );
    let asking = {
        let engine = harness.engine.clone();
        tokio::spawn(async move {
            engine
                .permissions
                .check(
                    &engine.hub,
                    &crate::permission::Policy::default(),
                    request,
                    &Default::default(),
                )
                .await
        })
    };

    while harness.engine.permissions.pending().is_empty() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let id = harness.engine.permissions.pending()[0].id.clone();
    harness
        .post(&format!("/permissions/{id}/reply"))
        .json(&json!({ "reply": "always" }))
        .send()
        .await
        .unwrap();
    assert_eq!(asking.await.unwrap(), crate::permission::Outcome::Allowed);
    let listed = json_response(harness.get(&format!("/workspaces/{workspace_id}/permission-grants"))).await;
    assert_eq!(listed, json!([{ "grant": "subcommand", "prefix": "cargo build" }]));

    let revoke_path = format!("/workspaces/{workspace_id}/permission-grants/revoke");
    assert_eq!(response_status(harness.post(&revoke_path).json(&listed[0])).await, 204);
    assert_eq!(response_status(harness.post(&revoke_path).json(&listed[0])).await, 404);
    assert_eq!(
        response_status(harness.delete(&format!("/workspaces/{workspace_id}/permission-grants"))).await,
        204
    );
    assert_unknown_grant_routes(&harness, &listed[0]).await;
    assert_grant_storage_cleanup(&harness, &workspace_id);
}

async fn assert_unknown_grant_routes(harness: &Harness, grant: &Value) {
    assert_eq!(
        response_status(harness.get("/workspaces/nope/permission-grants")).await,
        404
    );
    assert_eq!(
        response_status(harness.delete("/workspaces/nope/permission-grants")).await,
        404,
        "the same for every route"
    );
    assert_eq!(
        response_status(harness.post("/workspaces/nope/permission-grants/revoke").json(grant)).await,
        404
    );
}

fn assert_grant_storage_cleanup(harness: &Harness, workspace_id: &str) {
    let grant = crate::permission::Grant::Subcommand {
        prefix: "cargo test".into(),
    };
    harness
        .engine
        .store
        .set_setting(&format!("permissionGrants:{workspace_id}"), &vec![grant.clone()])
        .unwrap();
    harness
        .engine
        .store
        .set_setting(
            &format!("trustedCommands:{workspace_id}"),
            &vec!["check lint: eslint".to_string()],
        )
        .unwrap();
    assert!(
        harness.engine.permission_grants(workspace_id).is_empty(),
        "the cache still holds the emptied list"
    );

    harness.engine.permissions.forget_workspace(workspace_id);
    assert_eq!(
        harness.engine.permission_grants(workspace_id),
        [grant],
        "dropped from the cache, the stored list is read again"
    );
    harness.engine.forget_workspace(workspace_id).unwrap();
    assert!(
        harness
            .engine
            .store
            .setting::<Vec<crate::permission::Grant>>(&format!("permissionGrants:{workspace_id}"))
            .unwrap()
            .is_none()
    );
    assert!(
        harness
            .engine
            .store
            .setting::<Vec<String>>(&format!("trustedCommands:{workspace_id}"))
            .unwrap()
            .is_none()
    );
    assert!(
        harness.engine.permission_grants(workspace_id).is_empty(),
        "nothing is left in the cache either"
    );
}

#[tokio::test]
async fn settings_rules_apply_at_once_survive_a_restart_and_refuse_what_could_never_match() {
    let harness = harness().await;
    assert_eq!(json_response(harness.get("/permission-rules")).await, json!([]));
    let rules = json!([{ "kind": "bash", "pattern": "git push*", "decision": "deny" },
        { "kind": "webfetch", "pattern": "*", "decision": "ask" }]);
    let saved = json_response(harness.put("/permission-rules").json(&rules)).await;
    assert_eq!(saved, rules);
    let push = crate::tool::Ask::new("bash", "git push origin", "");
    assert_eq!(
        harness
            .engine
            .permissions
            .decide_now("s", &crate::permission::Policy::default(), &push),
        crate::permission::Decision::Deny,
        "the next call follows them"
    );

    let reopened = Engine::open_with(
        &harness.directory.0,
        crate::Options {
            file_credentials: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(reopened.permission_rules()).unwrap(),
        rules,
        "kept across a restart"
    );
    for bad in [
        json!([{ "kind": "Bash!", "pattern": "*", "decision": "deny" }]),
        json!([{ "kind": "bash", "pattern": " ", "decision": "deny" }]),
        json!([{ "kind": "read", "pattern": "a[", "decision": "deny" }]),
    ] {
        assert_eq!(
            response_status(harness.put("/permission-rules").json(&bad)).await,
            400,
            "{bad}"
        );
    }
    assert_eq!(
        json_response(harness.get("/permission-rules")).await,
        rules,
        "a refused save changes nothing"
    );
}
