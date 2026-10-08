use super::skills::SkillError;
use super::sources::SourceError;
use super::{AgentError, PluginPathError};

#[test]
fn config_validation_errors_keep_their_messages() {
    let errors = [
        (
            SourceError::NonRelativeFile,
            "a folder source names files relative to itself",
        ),
        (SourceError::InvalidGithubUrl, "not a GitHub repository URL"),
        (SourceError::InvalidAzureUrl, "not an Azure DevOps repository URL"),
        (SourceError::NotRepository, "only a repository source has an archive"),
        (
            SourceError::HttpRefused,
            "plain http is refused for this source; allow it in the source's settings if you must",
        ),
    ];

    for (error, expected) in errors {
        assert_eq!(error.to_string(), expected);
    }
    assert_eq!(
        PluginPathError::OutsideConfig.to_string(),
        "a plugin path must be relative and stay under the config directory"
    );
    assert_eq!(
        PluginPathError::NotComponent.to_string(),
        "a plugin is a .wasm component"
    );
    assert_eq!(
        AgentError::Unusable {
            name: "reviewer".into(),
            problem: "invalid permission decision".into()
        }
        .to_string(),
        "agent reviewer: invalid permission decision"
    );
}

#[test]
fn registry_http_status_errors_keep_their_access_hints() {
    let url = "https://registry.acme.test/plugins.json";
    let cases = [
        (401, " (a token may be needed, or the one stored may be wrong)"),
        (403, " (a token may be needed, or the one stored may be wrong)"),
        (
            404,
            " (not found; for a private repository that can also mean the token lacks access)",
        ),
        (500, ""),
    ];

    for (code, hint) in cases {
        let status = reqwest::StatusCode::from_u16(code).unwrap();
        let error = SourceError::HttpStatus {
            url: url.into(),
            status,
        };
        assert_eq!(error.to_string(), format!("could not fetch {url}: {status}{hint}"));
    }
}

#[test]
fn skill_validation_errors_keep_their_messages() {
    let errors = [
        (
            SkillError::InvalidId,
            "a pack id is letters, digits, dashes and underscores",
        ),
        (SkillError::RequiresHttps, "a pack is fetched over https only"),
        (
            SkillError::NoSkills,
            "the archive holds no SKILL.md in the folders asked for",
        ),
        (SkillError::OutsideSkillFolders, "not one of your skills"),
        (SkillError::NoPack, "no such pack"),
    ];

    for (error, expected) in errors {
        assert_eq!(error.to_string(), expected);
    }
    let error = SkillError::File {
        operation: "write",
        path: "skill-pack".into(),
        source: std::io::Error::other("disk is full"),
    };
    assert_eq!(error.to_string(), "could not write skill-pack: disk is full");
}
