//! The system prompt: who the model is, how to work here, and what the workspace says about itself.

use std::path::Path;

use crate::config::{Agent, Config};

const IDENTITY: &str = include_str!("prompts/system.txt");

pub fn system(workspace: &Path, config: &Config, agent: Option<&Agent>) -> String {
    let mut prompt = IDENTITY.trim().to_string();
    if let Some(agent) = agent.filter(|a| !a.prompt.is_empty()) {
        prompt.push_str(&format!("\n\n{}", agent.prompt));
    }
    prompt.push_str("\n\n# Environment\n\n");
    prompt.push_str(&format!("Working directory: {}\n", workspace.display()));
    prompt.push_str(&format!("Platform: {}\n", std::env::consts::OS));
    prompt.push_str(&format!("Date: {}\n", today()));
    if !config.skills.is_empty() {
        prompt.push_str("\n# Skills\n\nLoad one with the `skill` tool when its description matches the task.\n\n");
        for skill in &config.skills {
            prompt.push_str(&format!("- {}: {}\n", skill.name, skill.description));
        }
    }
    for instruction in &config.instructions {
        prompt.push_str(&format!("\n# Instructions from {}\n\n{}\n", instruction.name, instruction.text));
    }
    prompt
}
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's algorithm; avoids pulling a date crate in for one line.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_identity_environment_and_agents_file() {
        let workspace = std::env::temp_dir().join(format!("drift-prompt-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::write(workspace.join("AGENTS.md"), "agent rules").unwrap();
        let config = Config::load_with_home(&workspace, None);
        let prompt = system(&workspace, &config, config.agent("plan"));
        assert!(prompt.starts_with("You are Drift"));
        assert!(prompt.contains("# Plan mode"));
        assert!(prompt.contains("Working directory: "));
        assert!(prompt.contains("# Instructions from AGENTS.md\n\nagent rules"));
        assert!(!prompt.contains("claude rules"));
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}
