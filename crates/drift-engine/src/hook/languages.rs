//! Languages plugins add: language servers Drift starts as drift.json's own, and grammars the app highlights with.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::Hooks;
use crate::config::LspConfig;

/// A language server a plugin declares, as a drift.json `lsp` entry would.
#[derive(Clone, Debug, PartialEq)]
pub struct LanguageServer {
    pub id: String,
    pub command: Vec<String>,
    pub extensions: Vec<String>,
    pub language: Option<String>,
}

/// A TextMate grammar a plugin adds, as the app loads it into its highlighter.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Grammar {
    /// The language's name, which a code fence uses; it replaces a bundled language of the same name.
    pub name: String,
    pub aliases: Vec<String>,
    /// File name endings (`.roc`), or whole file names (`Rocfile`), it highlights.
    pub extensions: Vec<String>,
    /// The `.tmLanguage.json` object.
    #[schema(value_type = Object)]
    pub grammar: Value,
    /// The plugin that added it.
    pub plugin: String,
}

impl Grammar {
    /// A grammar from its JSON text, or why the text is not a TextMate grammar.
    pub fn parse(
        plugin: &str,
        name: String,
        aliases: Vec<String>,
        extensions: Vec<String>,
        json: &str,
    ) -> Result<Self, String> {
        let grammar: Value = serde_json::from_str(json).map_err(|error| format!("is not JSON: {error}"))?;
        let textmate = grammar["scopeName"].is_string() && grammar["patterns"].is_array();
        if name.trim().is_empty() || !textmate {
            return Err("needs a name, and a scopeName and patterns".into());
        }

        Ok(Self {
            name,
            aliases,
            extensions,
            grammar,
            plugin: plugin.to_owned(),
        })
    }
}

impl Hooks {
    /// Every plugin's language servers, in plugin order; a repeated id keeps its first plugin.
    pub fn language_servers(&self) -> Vec<LanguageServer> {
        let mut seen = HashSet::new();

        self.list()
            .iter()
            .flat_map(|hook| hook.language_servers())
            .filter(|server| seen.insert(server.id.clone()))
            .collect()
    }

    /// Every plugin's grammars, in plugin order; a repeated name keeps its first plugin.
    pub fn grammars(&self) -> Vec<Grammar> {
        let mut seen = HashSet::new();

        self.list()
            .iter()
            .flat_map(|hook| hook.grammars())
            .filter(|grammar| seen.insert(grammar.name.to_lowercase()))
            .collect()
    }

    /// The language servers to use: the plugins' first, then the config's, which replace or turn off theirs by name.
    pub fn lsp_config(&self, config: &BTreeMap<String, LspConfig>) -> BTreeMap<String, LspConfig> {
        let mut servers: BTreeMap<String, LspConfig> = self
            .language_servers()
            .into_iter()
            .map(|server| {
                let custom = LspConfig::Custom {
                    command: server.command,
                    extensions: server.extensions,
                    language: server.language,
                };
                (server.id, custom)
            })
            .collect();
        servers.extend(config.clone());

        servers
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::hook::Hook;

    struct Adds(&'static str, Vec<LanguageServer>, Vec<Grammar>);

    #[async_trait::async_trait]
    impl Hook for Adds {
        fn name(&self) -> &str {
            self.0
        }

        fn language_servers(&self) -> Vec<LanguageServer> {
            self.1.clone()
        }

        fn grammars(&self) -> Vec<Grammar> {
            self.2.clone()
        }
    }

    fn server(id: &str, command: &str) -> LanguageServer {
        LanguageServer {
            id: id.into(),
            command: vec![command.into()],
            extensions: vec![".roc".into()],
            language: None,
        }
    }

    fn grammar(plugin: &str, name: &str) -> Grammar {
        let json = r#"{ "scopeName": "source.roc", "patterns": [] }"#;
        Grammar::parse(plugin, name.into(), vec![], vec![".roc".into()], json).unwrap()
    }

    #[test]
    fn plugins_add_servers_the_config_replaces_or_turns_off_and_the_first_plugin_keeps_a_name() {
        let hooks = Hooks::default();
        let first = Adds(
            "a",
            vec![server("roc", "roc_ls"), server("hare", "hare-lsp")],
            vec![grammar("a", "Roc")],
        );
        let second = Adds(
            "b",
            vec![server("roc", "other")],
            vec![grammar("b", "roc"), grammar("b", "hare")],
        );
        hooks.set(vec![Arc::new(first), Arc::new(second)], vec![]);

        let config = BTreeMap::from([
            ("hare".to_string(), LspConfig::Enabled(false)),
            ("gopls".to_string(), LspConfig::Enabled(false)),
        ]);
        let servers = hooks.lsp_config(&config);
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["gopls", "hare", "roc"]);
        assert_eq!(servers["hare"], LspConfig::Enabled(false));
        assert!(matches!(&servers["roc"], LspConfig::Custom { command, .. } if command == &["roc_ls"]));

        let names: Vec<_> = hooks.grammars().into_iter().map(|g| (g.plugin, g.name)).collect();
        assert_eq!(names, [("a".into(), "Roc".into()), ("b".into(), "hare".into())]);
    }

    #[test]
    fn a_grammar_must_be_a_textmate_grammar() {
        let parse = |json: &str| Grammar::parse("p", "roc".into(), vec![], vec![], json);
        assert!(parse(r#"{ "scopeName": "source.roc", "patterns": [] }"#).is_ok());
        assert!(parse("not json").unwrap_err().starts_with("is not JSON"));
        assert!(parse(r#"{ "patterns": [] }"#).is_err());
        assert!(
            Grammar::parse(
                "p",
                " ".into(),
                vec![],
                vec![],
                r#"{ "scopeName": "s", "patterns": [] }"#
            )
            .is_err()
        );
    }
}
