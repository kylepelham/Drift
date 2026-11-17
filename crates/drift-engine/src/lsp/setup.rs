//! What Settings shows for each built-in language server: the language it serves, where its install guide is, and the
//! commands that install it with a package manager the machine already has. Drift runs one only when the user asks.

use std::time::Duration;

use crate::hook::ServerSetup;

/// How long an install may take before it is stopped.
const INSTALL_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How much of a failed install's output is kept to show.
const OUTPUT_CHARS: usize = 2_000;

pub(super) struct Setup {
    pub(super) name: &'static str,
    /// The language as people name it.
    pub(super) label: &'static str,
    pub(super) guide: &'static str,
    /// npm packages that hold it, installed globally with npm or bun.
    pub(super) npm: &'static [&'static str],
    /// Other commands that install it, the first word the program each needs.
    pub(super) other: &'static [&'static [&'static str]],
}

const fn setup(name: &'static str, label: &'static str, guide: &'static str) -> Setup {
    Setup {
        name,
        label,
        guide,
        npm: &[],
        other: &[],
    }
}

const fn npm(name: &'static str, label: &'static str, guide: &'static str, packages: &'static [&'static str]) -> Setup {
    Setup {
        name,
        label,
        guide,
        npm: packages,
        other: &[],
    }
}

const fn with(
    name: &'static str,
    label: &'static str,
    guide: &'static str,
    other: &'static [&'static [&'static str]],
) -> Setup {
    Setup {
        name,
        label,
        guide,
        npm: &[],
        other,
    }
}

pub(super) const SETUP: &[Setup] = &[
    with(
        "rust-analyzer",
        "Rust",
        "https://rust-analyzer.github.io/book/installation.html",
        &[&["rustup", "component", "add", "rust-analyzer"]],
    ),
    npm(
        "typescript",
        "TypeScript and JavaScript",
        "https://github.com/typescript-language-server/typescript-language-server#installing",
        &["typescript-language-server", "typescript"],
    ),
    setup(
        "deno",
        "Deno",
        "https://docs.deno.com/runtime/getting_started/installation/",
    ),
    npm(
        "pyright",
        "Python",
        "https://microsoft.github.io/pyright/#/installation",
        &["pyright"],
    ),
    with(
        "gopls",
        "Go",
        "https://go.dev/gopls/",
        &[&["go", "install", "golang.org/x/tools/gopls@latest"]],
    ),
    setup("clangd", "C and C++", "https://clangd.llvm.org/installation"),
    with(
        "csharp",
        "C#",
        "https://github.com/razzmatazz/csharp-language-server#installation",
        &[&["dotnet", "tool", "install", "--global", "csharp-ls"]],
    ),
    with(
        "fsharp",
        "F#",
        "https://github.com/ionide/FsAutoComplete#installation",
        &[&["dotnet", "tool", "install", "--global", "fsautocomplete"]],
    ),
    setup(
        "java",
        "Java",
        "https://github.com/eclipse-jdtls/eclipse.jdt.ls#installation",
    ),
    with(
        "ruby",
        "Ruby",
        "https://shopify.github.io/ruby-lsp/",
        &[&["gem", "install", "ruby-lsp"]],
    ),
    npm("php", "PHP", "https://intelephense.com/", &["intelephense"]),
    setup("swift", "Swift", "https://www.swift.org/install/"),
    setup("dart", "Dart", "https://dart.dev/get-dart"),
    setup(
        "elixir",
        "Elixir",
        "https://github.com/elixir-lsp/elixir-ls#installation",
    ),
    setup("zls", "Zig", "https://zigtools.org/zls/install/"),
    setup("lua", "Lua", "https://luals.github.io/#install"),
    with(
        "ocaml",
        "OCaml",
        "https://github.com/ocaml/ocaml-lsp#installation",
        &[&["opam", "install", "--yes", "ocaml-lsp-server"]],
    ),
    with(
        "haskell",
        "Haskell",
        "https://haskell-language-server.readthedocs.io/en/latest/installation.html",
        &[&["ghcup", "install", "hls"]],
    ),
    setup("gleam", "Gleam", "https://gleam.run/getting-started/installing/"),
    setup("clojure", "Clojure", "https://clojure-lsp.io/installation/"),
    setup("nix", "Nix", "https://github.com/nix-community/nixd"),
    npm(
        "svelte",
        "Svelte",
        "https://github.com/sveltejs/language-tools/tree/master/packages/language-server",
        &["svelte-language-server"],
    ),
    npm(
        "prisma",
        "Prisma",
        "https://www.npmjs.com/package/@prisma/language-server",
        &["@prisma/language-server"],
    ),
    npm(
        "yaml",
        "YAML",
        "https://github.com/redhat-developer/yaml-language-server",
        &["yaml-language-server"],
    ),
    npm(
        "bash",
        "Shell scripts",
        "https://github.com/bash-lsp/bash-language-server#installation",
        &["bash-language-server"],
    ),
    setup(
        "terraform",
        "Terraform",
        "https://github.com/hashicorp/terraform-ls/blob/main/docs/installation.md",
    ),
    setup("texlab", "LaTeX", "https://github.com/latex-lsp/texlab#installation"),
    setup("tinymist", "Typst", "https://myriad-dreamin.github.io/tinymist/"),
    npm(
        "dockerfile",
        "Dockerfiles",
        "https://github.com/rcjsuen/dockerfile-language-server#installation-instructions",
        &["dockerfile-language-server-nodejs"],
    ),
    setup("slang", "Slang", "https://github.com/shader-slang/slang/releases"),
];

pub(super) fn of(name: &str) -> Option<&'static Setup> {
    SETUP.iter().find(|setup| setup.name == name)
}

impl Setup {
    /// How it is installed: Drift's commands for it with its guide, or the guide alone.
    pub(super) fn server_setup(&self) -> ServerSetup {
        let npm = (!self.npm.is_empty()).then(|| {
            let global = |manager: &[&str]| manager.iter().chain(self.npm).map(ToString::to_string).collect();
            [
                global(&["npm", "install", "--global"]),
                global(&["bun", "add", "--global"]),
            ]
        });
        let commands: Vec<Vec<String>> = npm
            .into_iter()
            .flatten()
            .chain(
                self.other
                    .iter()
                    .map(|command| command.iter().map(ToString::to_string).collect()),
            )
            .collect();

        if commands.is_empty() {
            return ServerSetup::Guide(self.guide.to_owned());
        }
        ServerSetup::Install {
            commands,
            guide: self.guide.to_owned(),
        }
    }
}

/// The first install command whose program `found` finds, with that program's path.
pub(super) fn recipe(
    setup: &ServerSetup,
    found: impl Fn(&str) -> Option<std::path::PathBuf>,
) -> Option<(std::path::PathBuf, Vec<String>)> {
    let ServerSetup::Install { commands, .. } = setup else {
        return None;
    };

    commands.iter().find_map(|command| {
        let (program, args) = command.split_first()?;
        found(program).map(|path| (path, args.to_vec()))
    })
}

/// The programs an install needs, one of which is missing when there is no recipe to run.
pub(super) fn needs(setup: &ServerSetup) -> Vec<String> {
    let ServerSetup::Install { commands, .. } = setup else {
        return Vec::new();
    };
    let mut needs: Vec<String> = commands.iter().filter_map(|command| command.first().cloned()).collect();
    needs.dedup();

    needs
}

/// Runs `program` with `args` to install a server, with the current PATH; its output's tail when it fails.
pub(super) async fn run(program: &std::path::Path, args: &[String]) -> Result<(), String> {
    let mut command = tokio::process::Command::new(program);
    crate::platform::process::use_current_path(&mut command, &Default::default());
    command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);

    let output = match tokio::time::timeout(INSTALL_LIMIT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("could not run {}: {error}", program.display())),
        Err(_) => return Err(format!("stopped after {} minutes", INSTALL_LIMIT.as_secs() / 60)),
    };
    if output.status.success() {
        return Ok(());
    }

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = text.trim();
    let tail = text
        .char_indices()
        .rev()
        .nth(OUTPUT_CHARS)
        .map_or(text, |(at, _)| &text[at..]);
    Err(format!("exited with {}: {tail}", output.status))
}
