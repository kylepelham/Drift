//! The language servers Drift knows, with opencode's root markers. Each is used only when its command
//! is installed (on PATH, or a project's `node_modules/.bin`); Drift never downloads one.

use std::path::{Path, PathBuf};

/// Where a server for a file is rooted.
#[derive(Debug, PartialEq)]
pub enum Root {
    /// The folder of the nearest marker, trying each group in turn; the workspace when none is found.
    Nearest(&'static [&'static [&'static str]]),
    /// The folder of the nearest marker; with none the server is not used for the file.
    Strict(&'static [&'static str]),
    /// A crate's folder, or the Cargo workspace above it.
    Cargo,
}

pub struct Builtin {
    pub name: &'static str,
    /// Commands to try in order; the first installed one runs.
    pub commands: &'static [&'static [&'static str]],
    pub extensions: &'static [&'static str],
    pub root: Root,
    /// Markers that hand the file to another server (Deno's, for TypeScript).
    pub unless: &'static [&'static str],
}

const JS_LOCKS: &[&str] = &["package-lock.json", "bun.lockb", "bun.lock", "pnpm-lock.yaml", "yarn.lock"];
const DOTNET: &[&str] = &["*.slnx", "*.sln", "*.csproj", "*.fsproj", "global.json"];

const fn server(name: &'static str, commands: &'static [&'static [&'static str]], extensions: &'static [&'static str], root: Root) -> Builtin {
    Builtin { name, commands, extensions, root, unless: &[] }
}

pub const BUILTIN: &[Builtin] = &[
    server("rust-analyzer", &[&["rust-analyzer"]], &[".rs"], Root::Cargo),
    Builtin {
        name: "typescript",
        commands: &[&["typescript-language-server", "--stdio"]],
        extensions: &[".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"],
        root: Root::Nearest(&[JS_LOCKS]),
        unless: &["deno.json", "deno.jsonc"],
    },
    server("deno", &[&["deno", "lsp"]], &[".ts", ".tsx", ".js", ".jsx", ".mjs"], Root::Strict(&["deno.json", "deno.jsonc"])),
    server("pyright", &[&["pyright-langserver", "--stdio"], &["basedpyright-langserver", "--stdio"]], &[".py", ".pyi"], Root::Nearest(&[&["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile", "pyrightconfig.json"]])),
    server("gopls", &[&["gopls"]], &[".go"], Root::Nearest(&[&["go.work"], &["go.mod", "go.sum"]])),
    server("clangd", &[&["clangd"]], &[".c", ".h", ".cc", ".cpp", ".cxx", ".c++", ".hpp", ".hh", ".hxx", ".h++"], Root::Nearest(&[&["compile_commands.json", "compile_flags.txt", ".clangd"]])),
    server("csharp", &[&["roslyn-language-server", "--stdio", "--autoLoadProjects"], &["csharp-ls"]], &[".cs", ".csx"], Root::Nearest(&[DOTNET])),
    server("fsharp", &[&["fsautocomplete"]], &[".fs", ".fsi", ".fsx", ".fsscript"], Root::Nearest(&[DOTNET])),
    server("java", &[&["jdtls"]], &[".java"], Root::Strict(&["pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", ".project"])),
    server("ruby", &[&["ruby-lsp"], &["rubocop", "--lsp"]], &[".rb", ".rake", ".gemspec", ".ru"], Root::Nearest(&[&["Gemfile"]])),
    server("php", &[&["intelephense", "--stdio"]], &[".php"], Root::Nearest(&[&["composer.json", "composer.lock", ".php-version"]])),
    server("swift", &[&["sourcekit-lsp"]], &[".swift"], Root::Nearest(&[&["Package.swift", "*.xcodeproj", "*.xcworkspace"]])),
    server("dart", &[&["dart", "language-server"]], &[".dart"], Root::Nearest(&[&["pubspec.yaml", "analysis_options.yaml"]])),
    server("elixir", &[&["elixir-ls"]], &[".ex", ".exs"], Root::Nearest(&[&["mix.exs", "mix.lock"]])),
    server("zls", &[&["zls"]], &[".zig", ".zon"], Root::Nearest(&[&["build.zig"]])),
    server("lua", &[&["lua-language-server"]], &[".lua"], Root::Nearest(&[&[".luarc.json", ".luarc.jsonc", ".stylua.toml", "stylua.toml", "selene.toml"]])),
    server("ocaml", &[&["ocamllsp"]], &[".ml", ".mli"], Root::Nearest(&[&["dune-project", "dune-workspace", ".merlin", "opam"]])),
    server("haskell", &[&["haskell-language-server-wrapper", "--lsp"]], &[".hs", ".lhs"], Root::Nearest(&[&["stack.yaml", "cabal.project", "hie.yaml", "*.cabal"]])),
    server("gleam", &[&["gleam", "lsp"]], &[".gleam"], Root::Nearest(&[&["gleam.toml"]])),
    server("clojure", &[&["clojure-lsp"]], &[".clj", ".cljs", ".cljc", ".edn"], Root::Nearest(&[&["deps.edn", "project.clj", "shadow-cljs.edn", "bb.edn", "build.boot"]])),
    server("nix", &[&["nixd"]], &[".nix"], Root::Nearest(&[&["flake.nix"]])),
    server("svelte", &[&["svelteserver", "--stdio"]], &[".svelte"], Root::Nearest(&[JS_LOCKS])),
    server("prisma", &[&["prisma-language-server", "--stdio"]], &[".prisma"], Root::Nearest(&[&["schema.prisma", "prisma"]])),
    server("yaml", &[&["yaml-language-server", "--stdio"]], &[".yaml", ".yml"], Root::Nearest(&[JS_LOCKS])),
    server("bash", &[&["bash-language-server", "start"]], &[".sh", ".bash", ".zsh", ".ksh"], Root::Nearest(&[])),
    server("terraform", &[&["terraform-ls", "serve"]], &[".tf", ".tfvars"], Root::Nearest(&[&[".terraform.lock.hcl", "terraform.tfstate"]])),
    server("texlab", &[&["texlab"]], &[".tex", ".bib"], Root::Nearest(&[&[".latexmkrc", "latexmkrc", ".texlabroot", "texlabroot"]])),
    server("tinymist", &[&["tinymist"]], &[".typ", ".typc"], Root::Nearest(&[&["typst.toml"]])),
    server("dockerfile", &[&["docker-langserver", "--stdio"]], &["dockerfile"], Root::Nearest(&[])),
];

/// Where the server for `file` roots, or `None` when it is not used for that file.
pub fn root_for(root: &Root, unless: &[&str], file: &Path, workspace: &Path) -> Option<PathBuf> {
    let start = file.parent()?;
    if !unless.is_empty() && nearest(start, workspace, unless).is_some() {
        return None;
    }
    match root {
        Root::Nearest(groups) => Some(groups.iter().find_map(|markers| nearest(start, workspace, markers)).unwrap_or_else(|| workspace.to_path_buf())),
        Root::Strict(markers) => nearest(start, workspace, markers),
        Root::Cargo => {
            let krate = nearest(start, workspace, &["Cargo.toml"]).unwrap_or_else(|| workspace.to_path_buf());
            let above = krate.ancestors().take_while(|dir| dir.starts_with(workspace)).find(|dir| std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|text| text.contains("[workspace]")));
            Some(above.map_or(krate.clone(), Path::to_path_buf))
        }
    }
}

/// The nearest folder from `start` up to `stop` holding one of `markers`; `*.ext` names any file ending so.
fn nearest(start: &Path, stop: &Path, markers: &[&str]) -> Option<PathBuf> {
    start.ancestors().take_while(|dir| dir.starts_with(stop)).find(|dir| markers.iter().any(|marker| holds(dir, marker))).map(Path::to_path_buf)
}

fn holds(dir: &Path, marker: &str) -> bool {
    match marker.strip_prefix('*') {
        Some(ending) => std::fs::read_dir(dir).into_iter().flatten().flatten().any(|entry| entry.file_name().to_string_lossy().ends_with(ending)),
        None => dir.join(marker).exists(),
    }
}

/// The first of `commands` installed: on PATH, else in a `node_modules/.bin` from `root` up to the workspace.
pub fn installed(commands: &[Vec<String>], root: &Path, workspace: &Path) -> Option<(PathBuf, Vec<String>)> {
    let local: Vec<PathBuf> = root.ancestors().take_while(|dir| dir.starts_with(workspace)).map(|dir| dir.join("node_modules").join(".bin")).collect();
    commands.iter().find_map(|command| {
        let (program, args) = command.split_first()?;
        let found = crate::platform::process::which(program).or_else(|| crate::platform::process::find_in(program, local.iter().cloned()))?;
        Some((found, args.to_vec()))
    })
}
