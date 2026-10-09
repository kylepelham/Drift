use super::{Formatter, Uses};
use crate::config::FormatterConfig;
use std::collections::BTreeMap;

struct Builtin {
    name: &'static str,
    /// `$FILE` becomes the path.
    command: &'static [&'static str],
    extensions: &'static [&'static str],
    uses: Uses,
    stdin: bool,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "prettier",
        command: &["prettier", "--write", "$FILE"],
        extensions: &[
            ".ts", ".tsx", ".js", ".jsx", ".json", ".css", ".md", ".html", ".yaml", ".yml",
        ],
        uses: Uses::Prettier,
        stdin: false,
    },
    // stdin confines rustfmt to the edited file instead of following its child modules.
    Builtin {
        name: "rustfmt",
        command: &["rustfmt", "--emit", "stdout", "--edition", "$EDITION"],
        extensions: &[".rs"],
        uses: Uses::Rustfmt,
        stdin: true,
    },
    Builtin {
        name: "gofmt",
        command: &["gofmt", "-w", "$FILE"],
        extensions: &[".go"],
        uses: Uses::Always,
        stdin: false,
    },
    Builtin {
        name: "ruff",
        command: &["ruff", "format", "$FILE"],
        extensions: &[".py"],
        uses: Uses::Ruff,
        stdin: false,
    },
    Builtin {
        name: "black",
        command: &["black", "-q", "$FILE"],
        extensions: &[".py"],
        uses: Uses::Black,
        stdin: false,
    },
];

/// The formatters that may apply in a workspace: built-ins not disabled (each checked per file
/// against the project, and for an install in it or on PATH), plus custom ones.
pub fn resolve(overrides: &BTreeMap<String, FormatterConfig>) -> Vec<Formatter> {
    let mut formatters = Vec::new();
    for builtin in BUILTINS {
        let uses = match overrides.get(builtin.name) {
            Some(FormatterConfig::Enabled(false)) => continue,
            Some(FormatterConfig::Custom { command, extensions }) => {
                formatters.push(Formatter::custom(builtin.name, command, extensions));
                continue;
            }
            Some(FormatterConfig::Enabled(true)) => Uses::Always,
            None => builtin.uses,
        };

        formatters.push(Formatter {
            name: builtin.name.into(),
            command: builtin.command.iter().map(ToString::to_string).collect(),
            extensions: builtin.extensions.iter().map(ToString::to_string).collect(),
            uses,
            stdin: builtin.stdin,
        });
    }

    for (name, config) in overrides {
        if let FormatterConfig::Custom { command, extensions } = config
            && !formatters.iter().any(|formatter| &formatter.name == name)
        {
            formatters.push(Formatter::custom(name, command, extensions));
        }
    }

    formatters
}
