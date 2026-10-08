use super::*;

fn mention(path: &Path) -> Part {
    let path = path.to_string_lossy().replace('\\', "/");
    let url = if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    };

    file("text/plain", "mention", &url)
}

fn file(mime: &str, name: &str, url: &str) -> Part {
    Part::File {
        mime: mime.into(),
        name: name.into(),
        url: url.into(),
        path: None,
    }
}

fn with_files(text: &str, files: Vec<Part>) -> Prompt {
    let mut prompt = prompt(text);
    prompt.parts.extend(files);
    prompt
}

async fn sent_text(h: &Harness, prompt: Prompt) -> String {
    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt).await.await_ok();
    until_idle(h).await;

    texts_sent(h.provider.requests.lock().unwrap().last().unwrap()).join("\n")
}

#[tokio::test]
async fn a_mentioned_workspace_file_is_read_into_the_prompt() {
    let h = harness().await;
    let workspace = h._dir.join("ws");
    std::fs::write(workspace.join("notes.md"), "remember the milk\n").unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "").unwrap();
    let sent = sent_text(
        &h,
        with_files(
            "see @notes.md and @src",
            vec![mention(&workspace.join("notes.md")), mention(&workspace.join("src"))],
        ),
    )
    .await;

    assert!(sent.contains("<file path=\"notes.md\">\nremember the milk"), "{sent}");
    assert!(
        sent.contains("<file path=\"src\">\nlib.rs"),
        "a directory lists its entries: {sent}"
    );
    let stored: Vec<_> = transcript(&h)[0]
        .parts
        .iter()
        .filter_map(|row| match &row.part {
            Part::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        stored,
        [Some("notes.md".to_string()), Some("src".to_string())],
        "a mention remembers the file it was, for the client to open"
    );

    let forged = Part::File {
        mime: "text/plain".into(),
        name: "x".into(),
        url: "data:text/plain;base64,eA==".into(),
        path: Some("../secret".into()),
    };
    h.provider.push(text("ok"));
    h.engine
        .submit(&h.session.id, with_files("pasted", vec![forged]))
        .await
        .await_ok();
    until_idle(&h).await;
    let last_prompt = transcript(&h)
        .into_iter()
        .rev()
        .find(|message| message.info.role == Role::User)
        .unwrap();
    assert!(
        last_prompt
            .parts
            .iter()
            .all(|row| !matches!(&row.part, Part::File { path: Some(_), .. })),
        "a client cannot claim a part is a mention"
    );
}

#[tokio::test]
async fn a_mention_counts_as_read_only_once_its_prompt_is_admitted() {
    let h = harness().await;
    let notes = h._dir.join("ws/notes.md");
    std::fs::write(&notes, "milk\n").unwrap();
    let audio = file("audio/wav", "memo.wav", "data:audio/wav;base64,UklGRg==");
    assert!(
        h.engine
            .submit(&h.session.id, with_files("see", vec![mention(&notes), audio]))
            .await
            .is_err()
    );
    assert!(
        h.engine.store.read_files(&h.session.id).unwrap().is_empty(),
        "a refused prompt showed the model nothing"
    );

    sent_text(&h, with_files("see", vec![mention(&notes)])).await;
    assert_eq!(h.engine.store.read_files(&h.session.id).unwrap().len(), 1);
}

#[tokio::test]
async fn a_mentioned_secret_or_outside_file_is_not_read_without_a_rule() {
    let h = harness().await;
    let workspace = h._dir.join("ws");
    std::fs::write(workspace.join(".env"), "API_KEY=hunter2\n").unwrap();
    let outside = h._dir.join("outside.txt");
    std::fs::write(&outside, "far away\n").unwrap();
    let sent = sent_text(
        &h,
        with_files("look", vec![mention(&workspace.join(".env")), mention(&outside)]),
    )
    .await;
    assert!(!sent.contains("hunter2") && !sent.contains("far away"), "{sent}");
    assert!(
        sent.contains("@.env was mentioned but not read: it may hold secrets. Use the read tool"),
        "{sent}"
    );
    assert!(sent.contains("it is outside the workspace"), "{sent}");

    let resolved = crate::tool::canonical(&outside).to_string_lossy().into_owned();
    std::fs::write(
        workspace.join("drift.json"),
        json!({ "permissions": [{ "kind": "read", "pattern": resolved, "decision": "allow" }] }).to_string(),
    )
    .unwrap();
    let allowed = sent_text(&h, with_files("again", vec![mention(&outside)])).await;
    assert!(
        allowed.contains("far away"),
        "a rule that allows the read lets the mention in: {allowed}"
    );
}

#[tokio::test]
async fn files_a_model_cannot_take_are_refused_not_dropped() {
    let h = harness().await;
    let image = file("image/png", "shot.png", "data:image/png;base64,iVBORw0KGgo=");
    mutate_model(&h, |model| model.attachment = false);
    let refused = h
        .engine
        .submit(&h.session.id, with_files("look", vec![image]))
        .await
        .unwrap_err();
    let TurnError::Attachment(message) = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        message.contains("cannot read images") && message.contains("shot.png"),
        "{refused:?}"
    );
    assert!(
        !h.engine.turns.is_running(&h.session.id),
        "a refused prompt leaves the session free"
    );
    let audio = file("audio/wav", "memo.wav", "data:audio/wav;base64,UklGRg==");
    assert!(matches!(
        h.engine.submit(&h.session.id, with_files("hear", vec![audio])).await,
        Err(TurnError::Attachment(_))
    ));
    let remote = file("text/plain", "remote", "https://example.com/a.txt");
    assert!(matches!(
        h.engine.submit(&h.session.id, with_files("fetch", vec![remote])).await,
        Err(TurnError::Attachment(_))
    ));
    assert!(h.provider.requests.lock().unwrap().is_empty());

    let note = file("text/plain", "note.txt", "data:text/plain;base64,aGVsbG8gdGhlcmU=");
    assert!(
        sent_text(&h, with_files("read this", vec![note]))
            .await
            .contains("hello there"),
        "text travels as text"
    );
}

#[tokio::test]
async fn a_pdf_goes_whole_to_a_model_that_reads_pdfs_and_is_refused_by_one_that_does_not() {
    let h = harness().await;
    let pdf = file(
        "application/pdf",
        "spec.pdf",
        "data:application/pdf;base64,JVBERi0xLjcK",
    );
    mutate_model(&h, |model| model.pdf = false);
    let refused = h
        .engine
        .submit(&h.session.id, with_files("read", vec![pdf.clone()]))
        .await
        .unwrap_err();
    let TurnError::Attachment(message) = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        message.contains("cannot read PDFs") && message.contains("spec.pdf"),
        "{refused:?}"
    );
    let fake = file("application/pdf", "spec.pdf", "data:application/pdf;base64,aGVsbG8=");
    mutate_model(&h, |model| model.pdf = true);
    let refused = h.engine.submit(&h.session.id, with_files("read", vec![fake])).await;
    assert!(matches!(refused, Err(TurnError::Attachment(message)) if message.contains("not one")));

    h.provider.push(text("read it"));
    h.engine
        .submit(&h.session.id, with_files("read", vec![pdf]))
        .await
        .await_ok();
    until_idle(&h).await;
    let request = h.provider.requests.lock().unwrap().last().unwrap().clone();
    assert!(
        request.messages[0]
            .blocks
            .iter()
            .any(|block| matches!(block, llm::Block::Pdf { base64 } if base64 == "JVBERi0xLjcK")),
        "{:?}",
        request.messages[0].blocks
    );
}

#[tokio::test]
async fn malformed_attachments_are_refused_before_admission() {
    let h = harness().await;
    for (mime, url, why) in [
        (
            "text/plain",
            "data:text/plain;base64,@@not base64@@",
            "could not be decoded",
        ),
        ("text/plain", "data:text/plain;base64,/w==", "could not be decoded"),
        ("image/png", "data:image/png;base64,***", "not valid base64"),
        ("image/png", "data:image/png,rawbytes", "not valid base64"),
        (
            "image/png",
            "data:image/jpeg;base64,iVBORw0KGgo=",
            "its data is image/jpeg",
        ),
    ] {
        let refused = h
            .engine
            .submit(&h.session.id, with_files("look", vec![file(mime, "bad", url)]))
            .await
            .unwrap_err();
        assert!(
            matches!(&refused, TurnError::Attachment(message) if message.contains(why)),
            "{url}: {refused:?}"
        );
    }

    assert!(transcript(&h).is_empty(), "nothing was admitted");
    assert!(h.provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_file_read_in_through_a_mention_can_be_edited_straight_away() {
    let h = harness().await;
    rule(&h, "edit", "*", Decision::Allow);
    let workspace = h._dir.join("ws");
    std::fs::write(workspace.join("notes.md"), "old line\n").unwrap();
    h.provider
        .push(tool_call(
            "edit",
            r#"{"path": "notes.md", "old_string": "old line", "new_string": "new line"}"#,
        ))
        .push(text("edited"));
    h.engine
        .submit(
            &h.session.id,
            with_files("fix @notes.md", vec![mention(&workspace.join("notes.md"))]),
        )
        .await
        .await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let edited = tool(&messages[1].parts[0]);
    assert_eq!(edited.status, ToolStatus::Done, "{:?}", edited.output);
    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "new line\n"
    );
}

#[tokio::test]
async fn a_steered_image_is_judged_against_the_model_the_next_request_runs_on() {
    let h = harness().await;
    let other = {
        let mut catalog = h.engine.catalog.write().unwrap();
        let models = &mut catalog.providers.get_mut("anthropic").unwrap().models;
        models.get_mut("claude-sonnet-4-5").unwrap().attachment = false;
        models
            .values()
            .find(|model| model.id != "claude-sonnet-4-5" && model.attachment)
            .unwrap()
            .id
            .clone()
    };
    h.provider
        .push_slow(Duration::from_millis(600), text("done"))
        .push(text("seen it"));
    h.engine.submit(&h.session.id, prompt("slow")).await.await_ok();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let image = file("image/png", "shot.png", "data:image/png;base64,iVBORw0KGgo=");
    let mut steered = with_files("look at this", vec![image.clone()]);
    steered.model = None;
    let refused = h.engine.submit(&h.session.id, steered).await.unwrap_err();
    assert!(
        matches!(&refused, TurnError::Attachment(message) if message.contains("cannot read images")),
        "the running model decides: {refused:?}"
    );

    let mut elsewhere = with_files("look at this", vec![image]);
    elsewhere.model = Some(ModelRef {
        provider: "anthropic".into(),
        model: other.clone(),
    });
    h.engine
        .submit(&h.session.id, elsewhere)
        .await
        .expect("the model it switches to reads images");
    until_idle(&h).await;
    assert_eq!(
        h.provider.requests.lock().unwrap().last().unwrap().model,
        other,
        "answered on the model it named"
    );
}
