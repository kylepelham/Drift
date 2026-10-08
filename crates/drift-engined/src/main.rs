//! Headless Drift engine for conformance tests and remote hosts.
//! drift-engined [--data-dir DIR] [--port N] [--file-credentials] serves and prints url and token lines.
//! drift-engined --openapi prints the API document and exits.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

struct Args {
    data_dir: PathBuf,
    port: u16,
    openapi: bool,
    file_credentials: bool,
}

#[derive(Debug, PartialEq, thiserror::Error)]
enum ArgumentError {
    #[error("--data-dir needs a path")]
    MissingDataDir,
    #[error("--port needs a number")]
    MissingPort,
    #[error("bad port: {0}")]
    InvalidPort(String),
    #[error("unknown argument: {0}")]
    Unknown(String),
}

/// Defaults to $XDG_DATA_HOME/drift, then ~/.local/share/drift, using the temp directory if no home is available.
fn default_data_dir() -> PathBuf {
    let home = || drift_engine::config::home().map(|home| home.join(".local").join("share"));
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(home)
        .unwrap_or_else(std::env::temp_dir);

    base.join("drift")
}

fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Args, ArgumentError> {
    let mut args = Args {
        data_dir: default_data_dir(),
        port: 0,
        openapi: false,
        file_credentials: false,
    };

    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--openapi" => args.openapi = true,
            "--file-credentials" => args.file_credentials = true,
            "--data-dir" => args.data_dir = arguments.next().ok_or(ArgumentError::MissingDataDir)?.into(),
            "--port" => {
                let value = arguments.next().ok_or(ArgumentError::MissingPort)?;
                args.port = value.parse().map_err(|_| ArgumentError::InvalidPort(value))?;
            }
            other => return Err(ArgumentError::Unknown(other.into())),
        }
    }

    Ok(args)
}

#[tokio::main]
async fn main() {
    // reqwest is built without a bundled TLS provider so the engine can share the shell's.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    if args.openapi {
        println!("{}", drift_engine::api::openapi().to_pretty_json().unwrap());
        return;
    }

    let options = drift_engine::Options {
        file_credentials: args.file_credentials,
        ..Default::default()
    };
    let engine = drift_engine::Engine::open_with(&args.data_dir, options).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, args.port));
    let server = drift_engine::listen(engine.clone(), addr)
        .await
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1);
        });

    println!("url {}", server.url());
    println!("token {}", engine.token);

    // Without a console there is no Ctrl+C to wait for; the parent kills us instead.
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }

    server.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Result<Args, ArgumentError> {
        parse(values.iter().map(|value| (*value).to_string()))
    }

    #[test]
    fn flags_set_the_server_options() {
        let args = arguments(&[
            "--data-dir",
            "data",
            "--port",
            "1234",
            "--openapi",
            "--file-credentials",
        ])
        .unwrap();

        assert_eq!(args.data_dir, PathBuf::from("data"));
        assert_eq!(args.port, 1234);
        assert!(args.openapi);
        assert!(args.file_credentials);
    }

    #[test]
    fn argument_errors_keep_their_cli_messages() {
        let cases = [
            (
                vec!["--data-dir"],
                ArgumentError::MissingDataDir,
                "--data-dir needs a path",
            ),
            (vec!["--port"], ArgumentError::MissingPort, "--port needs a number"),
            (
                vec!["--port", "65536"],
                ArgumentError::InvalidPort("65536".into()),
                "bad port: 65536",
            ),
            (
                vec!["--other"],
                ArgumentError::Unknown("--other".into()),
                "unknown argument: --other",
            ),
        ];

        for (values, expected, message) in cases {
            let error = arguments(&values).err().unwrap();

            assert_eq!(error, expected);
            assert_eq!(error.to_string(), message);
        }
    }
}
