use super::*;

#[tokio::test]
async fn resources_are_listed_and_read_and_prompts_become_commands() {
    let engine = engine();
    let ServerConfig::Stdio { command, args, .. } = echo_config() else {
        unreachable!()
    };
    let config = ServerConfig::Stdio {
        command,
        args,
        env: [("RICH".to_string(), "1".to_string())].into(),
        cwd: None,
        timeout_seconds: None,
    };
    saved(&engine, "notes", &config).await;
    engine.connect_mcp_in("notes", Some(&here())).await.unwrap();
    let context = context(&engine);

    let names: Vec<String> = engine
        .mcp
        .tools(&engine.store, Some(&here()))
        .iter()
        .map(|tool| tool.spec().name)
        .collect();
    assert!(
        names.contains(&"mcp_resources".to_string()) && names.contains(&"mcp_read_resource".to_string()),
        "{names:?}"
    );
    let listed = tool(&engine, "mcp_resources")
        .run(&context, json!({}))
        .await
        .unwrap()
        .output;
    assert!(
        listed.contains("notes note://readme readme (text/plain): The notes"),
        "{listed}"
    );

    let read = tool(&engine, "mcp_read_resource");
    let note = read
        .run(&context, json!({ "server": "notes", "uri": "note://readme" }))
        .await
        .unwrap();
    assert!(note.output.contains("remember the milk"));
    let shot = read
        .run(&context, json!({ "server": "notes", "uri": "note://shot" }))
        .await
        .unwrap();
    assert_eq!(
        crate::tool::image::returned(&shot.metadata)[0].mime,
        "image/png",
        "an image resource comes back to look at"
    );

    let commands = engine.mcp.prompt_commands(Some(&here()));
    assert_eq!(
        (commands[0].name.as_str(), commands[0].arguments.clone()),
        ("notes:review", vec!["file".to_string(), "focus".to_string()])
    );
    let arguments = commands[0].named_arguments("src/a.rs error handling");
    let filled = engine
        .mcp
        .get_prompt("notes", Some(&here()), "review", arguments)
        .await
        .unwrap();
    assert_eq!(
        filled, "Review src/a.rs for error handling",
        "one word each, the last taking the rest"
    );
}
