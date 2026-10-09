use super::{CheckConfig, Config, FormatterConfig};
use std::collections::BTreeMap;
use std::path::PathBuf;

impl Config {
    /// Records the commands a project file names, so they run only once the user trusts them; a `false` there runs nothing.
    pub(super) fn note_project_commands(
        &mut self,
        formatters: &BTreeMap<String, FormatterConfig>,
        checks: &BTreeMap<String, CheckConfig>,
    ) {
        let named = |kind: &str, name: &String, custom: bool| (format!("{kind}:{name}"), custom);
        let formatter_entries = formatters
            .iter()
            .map(|(name, config)| named("formatter", name, matches!(config, FormatterConfig::Custom { .. })));
        let check_entries = checks
            .iter()
            .map(|(name, config)| named("check", name, matches!(config, CheckConfig::Custom { .. })));

        for (key, custom) in formatter_entries.chain(check_entries) {
            if custom {
                self.project_commands.insert(key);
            } else {
                self.project_commands.remove(&key);
            }
        }
    }

    /// One of the project's own commands as the user is asked to trust it (`check lint: eslint $FILE`),
    /// with the extensions it runs on.
    fn project_line(&self, key: &str) -> Option<(String, &[String])> {
        let (kind, name) = key.split_once(':')?;
        let (command, extensions) = match kind {
            "check" => match self.checks.get(name)? {
                CheckConfig::Custom { command, extensions } => (command, extensions),
                CheckConfig::Enabled(_) => return None,
            },
            _ => match self.formatters.get(name)? {
                FormatterConfig::Custom { command, extensions } => (command, extensions),
                FormatterConfig::Enabled(_) => return None,
            },
        };

        Some((format!("{kind} {name}: {}", command.join(" ")), extensions))
    }

    /// The project's own commands of `kind` (`check` or `formatter`) that would run on one of `files`.
    pub fn project_command_lines(&self, kind: &str, files: &[PathBuf]) -> Vec<String> {
        let covers = |extensions: &[String]| {
            files.iter().any(|file| {
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                extensions
                    .iter()
                    .any(|extension| name.ends_with(&extension.to_lowercase()))
            })
        };

        self.project_commands
            .iter()
            .filter(|key| key.split_once(':').is_some_and(|(namespace, _)| namespace == kind))
            .filter_map(|key| self.project_line(key))
            .filter(|(_, extensions)| covers(extensions))
            .map(|(line, _)| line)
            .collect()
    }

    /// Formatters and checks without the project's own commands the user has not allowed (`allowed`
    /// judges each by its line); built-in formatters and the user's own always stay.
    pub fn only_allowed(
        &self,
        allowed: impl Fn(&str) -> bool,
    ) -> (BTreeMap<String, FormatterConfig>, BTreeMap<String, CheckConfig>) {
        let keeps = |kind: &str, name: &String| {
            let key = format!("{kind}:{name}");
            !self.project_commands.contains(&key) || self.project_line(&key).is_some_and(|(line, _)| allowed(&line))
        };
        let formatters = self
            .formatters
            .iter()
            .filter(|(name, _)| keeps("formatter", name))
            .map(|(name, config)| (name.clone(), config.clone()))
            .collect();
        let checks = self
            .checks
            .iter()
            .filter(|(name, _)| keeps("check", name))
            .map(|(name, config)| (name.clone(), config.clone()))
            .collect();

        (formatters, checks)
    }
}
