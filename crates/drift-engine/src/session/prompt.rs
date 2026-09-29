//! The system prompt: who the model is, how to work here, and what the workspace says about itself.

use std::path::Path;

const IDENTITY: &str = include_str!("prompts/system.txt");
const INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
/// Instruction files beyond this are cut; a model that needs more can read the file.
const MAX_INSTRUCTION_CHARS: usize = 40_000;

pub fn system(workspace: &Path) -> String {
    let mut prompt = IDENTITY.trim().to_string();
    prompt.push_str("\n\n# Environment\n\n");
    prompt.push_str(&format!("Working directory: {}\n", workspace.display()));
    prompt.push_str(&format!("Platform: {}\n", std::env::consts::OS));
    prompt.push_str(&format!("Date: {}\n", today()));
    if let Some((name, text)) = instructions(workspace) {
        prompt.push_str(&format!("\n# Instructions from {name}\n\n{text}\n"));
    }
    prompt
}

/// The first instruction file found at the workspace root; AGENTS.md wins over CLAUDE.md.
fn instructions(workspace: &Path) -> Option<(String, String)> {
    INSTRUCTION_FILES.iter().find_map(|name| {
        let text = std::fs::read_to_string(workspace.join(name)).ok()?;
        let text = if text.chars().count() > MAX_INSTRUCTION_CHARS {
            let cut: String = text.chars().take(MAX_INSTRUCTION_CHARS).collect();
            format!("{cut}\n\n(truncated; read {name} for the rest)")
        } else {
            text
        };
        Some((name.to_string(), text.trim().to_string()))
    })
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
        let prompt = system(&workspace);
        assert!(prompt.starts_with("You are Drift"));
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
