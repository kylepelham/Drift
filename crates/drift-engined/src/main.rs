//! Headless Drift engine for conformance tests and remote hosts.
//!
//! `drift-engined [--data-dir DIR] [--port N] [--file-credentials]` serves and prints `url` and `token` lines.
//! `drift-engined --openapi` prints the API document and exits.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

struct Args {
    data_dir: PathBuf,
    port: u16,
    openapi: bool,
    file_credentials: bool,
}

fn parse() -> Result<Args, String> {
    let mut args = Args {
        data_dir: std::env::temp_dir().join("drift-engined"),
        port: 0,
        openapi: false,
        file_credentials: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--openapi" => args.openapi = true,
            "--file-credentials" => args.file_credentials = true,
            "--data-dir" => args.data_dir = iter.next().ok_or("--data-dir needs a path")?.into(),
            "--port" => {
                let value = iter.next().ok_or("--port needs a number")?;
                args.port = value.parse().map_err(|_| format!("bad port: {value}"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

#[tokio::main]
async fn main() {
    // reqwest is built without a bundled TLS provider so the engine can share the shell's.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = match parse() {
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
    let options = drift_engine::Options { file_credentials: args.file_credentials, ..Default::default() };
    let engine = drift_engine::Engine::open_with(&args.data_dir, options).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, args.port));
    let server = drift_engine::listen(engine.clone(), addr).await.unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    println!("url {}", server.url());
    println!("token {}", engine.token);
    let _ = tokio::signal::ctrl_c().await;
    server.stop();
}
