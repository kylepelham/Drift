use super::*;
use crate::config::references::{Place, Reference};

fn git(directory: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn the_baseline_lists_each_reference_and_a_projects_says_reading_there_asks() {
    let h = harness().await;
    std::fs::write(
        h._dir.join("ws/drift.json"),
        json!({ "references": { "spec": { "path": "../spec", "description": "The wire spec" } } }).to_string(),
    )
    .unwrap();
    h.provider.push(text("ok"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;

    let system = h.provider.requests.lock().unwrap()[0].system.clone();
    assert!(system.contains("# References"), "{system}");
    assert!(system.contains("- spec: The wire spec"), "{system}");
    assert!(
        system.contains("named by this project, so reading there asks first"),
        "{system}"
    );
    assert!(system.contains(&crate::tool::canonical(&h._dir.join("spec")).display().to_string()));
}

#[tokio::test]
async fn a_users_repository_reference_is_cloned_shallow_in_the_background() {
    let h = harness().await;
    let origin = h._dir.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q"]);
    std::fs::write(origin.join("README.md"), "hello\n").unwrap();
    git(&origin, &["add", "README.md"]);
    git(
        &origin,
        &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "one"],
    );
    let reference = Reference {
        name: "origin".into(),
        description: String::new(),
        place: Place::Repository {
            url: origin.to_string_lossy().into_owned(),
            branch: None,
        },
        project: false,
    };

    let first = h.engine.references_for_turn(std::slice::from_ref(&reference));
    assert_eq!(first[0].note, Some("still being cloned; look again in a moment"));
    let clone = first[0].path.clone();
    assert!(
        clone.starts_with(crate::tool::canonical(&h.engine.data_dir)),
        "kept in Drift's data"
    );
    for _ in 0..500 {
        if clone.join("README.md").exists() && h.engine.reference_fetches.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md")).unwrap().trim(),
        "hello"
    );

    let again = h.engine.references_for_turn(&[reference]);
    assert_eq!(again[0].note, None, "cloned, and fresh for a day");
    assert!(
        h.engine.reference_fetches.0.lock().unwrap().is_empty(),
        "no fetch within the day"
    );
}
