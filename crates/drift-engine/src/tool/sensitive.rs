//! Files that usually hold secrets. Reading one always asks, even inside the workspace, and searches
//! skip them. Committed examples of them (`.env.example`) hold no secrets and are ordinary files.

use std::path::Path;

/// Exact file names, compared case-insensitively.
const SECRET_NAMES: [&str; 13] = [
    ".envrc", ".npmrc", ".pypirc", ".netrc", "_netrc", ".git-credentials", ".htpasswd", "credentials", "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".pgpass",
];
/// Key, certificate and keystore extensions.
const SECRET_EXTENSIONS: [&str; 9] = ["pem", "key", "p12", "pfx", "jks", "keystore", "kdbx", "ppk", "asc"];
/// A name part marking a file as a template rather than the real thing.
const EXAMPLE_PARTS: [&str; 7] = ["example", "sample", "template", "dist", "defaults", "schema", "tmpl"];

pub fn is_sensitive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let name = name.to_ascii_lowercase();
    if name.split('.').any(|part| EXAMPLE_PARTS.contains(&part)) {
        return false;
    }
    let env_file = name == ".env" || name.starts_with(".env.") || name.ends_with(".env");
    env_file || SECRET_NAMES.contains(&name.as_str()) || name.rsplit_once('.').is_some_and(|(_, ext)| SECRET_EXTENSIONS.contains(&ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_recognised_and_their_examples_are_not() {
        for secret in [".env", ".env.local", ".ENV.production", "prod.env", ".envrc", "sub/.envrc", "server.pem", "tls.key", ".npmrc", "id_ed25519", "C:/Users/me/.aws/credentials", "store.p12"] {
            assert!(is_sensitive(Path::new(secret)), "{secret}");
        }
        for ordinary in [".env.example", ".env.sample", ".env.template", "config.env.dist", ".envrc.example", "id_ed25519.pub", "keyboard.rs", "environment.ts", "README.md", "src/.envrc.md"] {
            assert!(!is_sensitive(Path::new(ordinary)), "{ordinary}");
        }
    }
}
