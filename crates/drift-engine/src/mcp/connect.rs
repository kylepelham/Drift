use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};

use super::{Client, DriftClient, Era, Error, Failure, Listing, Live, PROBE_WAIT, STEP_LIMIT, ServerConfig, Transport};
use super::{lifecycle, oauth, sse};
use crate::platform::process::Tree;

/// The signed-in server a connect is for, when it has a sign-in to use.
#[derive(Clone, Copy)]
pub(super) struct SignIn<'a> {
    pub(super) server: &'a str,
    pub(super) credentials: Option<&'a Arc<crate::llm::credentials::Credentials>>,
}

pub(super) async fn open(
    config: &ServerConfig,
    hash: String,
    sign_in: SignIn<'_>,
    known: Option<Era>,
    workspace: Option<&Path>,
) -> Result<Live, Failure> {
    let (service, tree) = begin(config, sign_in, known, workspace).await?;
    let (tools, ttl) = within("list its tools", list_tools(&service)).await?;

    let info = service.peer_info();
    let era = info
        .as_ref()
        .map_or(Era::Legacy, |info| Era::of(&info.protocol_version));
    let instructions = info
        .as_ref()
        .and_then(|info| info.instructions.clone())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let resources = info.as_ref().is_some_and(|info| info.capabilities.resources.is_some());

    // A server whose prompts cannot be listed still serves its tools; it simply offers no commands.
    let prompts = match info.as_ref().is_some_and(|info| info.capabilities.prompts.is_some()) {
        true => within("list its prompts", async {
            service.list_all_prompts().await.map_err(Error::Service)
        })
        .await
        .unwrap_or_default(),
        false => Vec::new(),
    };

    Ok(Live {
        service,
        era,
        transport: Transport::of(config),
        listing: Mutex::new(Listing::new(tools, ttl)),
        instructions,
        prompts,
        resources,
        timeout: config.timeout(),
        hash,
        since: Instant::now(),
        tree,
    })
}

/// Every page of the server's tools, and the shortest freshness any page gave.
pub(super) async fn list_tools(service: &Client) -> Result<(Vec<rmcp::model::Tool>, Option<Duration>), Error> {
    let (mut tools, mut ttl, mut cursor) = (Vec::new(), None::<u64>, None);

    loop {
        let page = service
            .list_tools(Some(rmcp::model::PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .map_err(Error::Tools)?;
        ttl = match (ttl, page.ttl_ms) {
            (Some(previous), Some(current)) => Some(previous.min(current)),
            (previous, current) => previous.or(current),
        };

        tools.extend(page.tools);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok((tools, ttl.map(Duration::from_millis)));
        }
    }
}

/// The eras a connect tries in turn (`None` is rmcp's probe-then-handshake), and whether a try that timed out moves on.
pub(super) fn attempts(config: &ServerConfig, known: Option<Era>) -> (Vec<Option<Era>>, bool) {
    match (config, known) {
        (ServerConfig::Sse { .. }, _) => (vec![Some(Era::Legacy)], false),
        // rmcp's 10 s probe fallback can overlap a late stdio reply, so each era starts in a fresh process.
        (ServerConfig::Stdio { .. }, None) => (vec![Some(Era::Stateless), Some(Era::Legacy)], true),
        (ServerConfig::Stdio { .. }, Some(era)) => (vec![Some(era), Some(era.other())], false),
        (ServerConfig::Http { .. }, None) => (vec![None], false),
        (ServerConfig::Http { .. }, Some(era)) => (vec![Some(era), None], false),
    }
}

/// Starts in each era `attempts` gives until one answers; the last failure is the one reported.
async fn begin(
    config: &ServerConfig,
    sign_in: SignIn<'_>,
    known: Option<Era>,
    workspace: Option<&Path>,
) -> Result<(Client, Option<Tree>), Failure> {
    let (tries, past_timeouts) = attempts(config, known);
    let mut failed = Failure::from(String::new());

    for era in tries {
        let limit = if era.is_none() {
            STEP_LIMIT + PROBE_WAIT
        } else {
            STEP_LIMIT
        };
        match tokio::time::timeout(limit, start(config, sign_in, era, workspace)).await {
            Ok(Ok(started)) => return Ok(started),
            Ok(Err(error)) => failed = error,
            Err(_) => {
                failed = Failure::from(format!("the server did not start within {limit:?}"));
                if !past_timeouts {
                    break;
                }
            }
        }
    }

    Err(failed)
}

pub(super) async fn within<T>(what: &str, step: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    tokio::time::timeout(STEP_LIMIT, step).await.unwrap_or_else(|_| {
        Err(Error::Timeout {
            what: what.to_owned(),
            limit: STEP_LIMIT,
        })
    })
}

/// Opens the transport and starts the server's era; `known` unset probes, except for HTTP+SSE.
/// A stdio server for a workspace runs there (its own `cwd` wins) and is told it as its root.
async fn start(
    config: &ServerConfig,
    sign_in: SignIn<'_>,
    known: Option<Era>,
    workspace: Option<&Path>,
) -> Result<(Client, Option<Tree>), Failure> {
    match config {
        ServerConfig::Stdio {
            command,
            args,
            env,
            cwd,
            ..
        } => {
            let command_process = stdio_command(command, args, env, cwd.as_deref(), workspace)?;
            let transport = TokioChildProcess::new(command_process)
                .map_err(|error| format!("could not start {command}: {error}"))?;

            // Adopt the process tree before the handshake so cancellation also kills server children.
            let tree = transport.id().and_then(|pid| Tree::adopt(pid).ok());
            let service = DriftClient::rooted(workspace)
                .serve_with_lifecycle(transport, lifecycle(known))
                .await?;

            Ok((service, tree))
        }
        ServerConfig::Http {
            url,
            headers,
            oauth: app,
            ..
        } => {
            let config = http_config(url, headers);

            // rmcp's authorized client refreshes a signed-in server's token itself.
            let signed_in = match sign_in.credentials {
                Some(credentials) => oauth::signed_in_client(credentials, sign_in.server, url, app.as_ref()).await,
                None => None,
            };
            let service = match signed_in {
                Some(client) => {
                    DriftClient::rooted(None)
                        .serve_with_lifecycle(
                            StreamableHttpClientTransport::with_client(client, config),
                            lifecycle(known),
                        )
                        .await
                }
                None => {
                    DriftClient::rooted(None)
                        .serve_with_lifecycle(
                            StreamableHttpClientTransport::with_client(crate::llm::http::client(), config),
                            lifecycle(known),
                        )
                        .await
                }
            };

            Ok((service?, None))
        }
        ServerConfig::Sse {
            url,
            headers,
            oauth: app,
            ..
        } => {
            let mut headers = header_map(headers);

            // rmcp authorizes streamable HTTP only; SSE uses its refreshed token as a header.
            if let Some(token) = match sign_in.credentials {
                Some(credentials) => oauth::signed_in_token(credentials, sign_in.server, url, app.as_ref()).await,
                None => None,
            } && let Ok(value) = format!("Bearer {token}").parse()
            {
                headers.insert(http::header::AUTHORIZATION, value);
            }

            let transport = sse::SseTransport::connect(crate::llm::http::client(), url, headers).await?;
            let service = DriftClient::rooted(None)
                .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
                .await?;

            Ok((service, None))
        }
    }
}

/// A stdio server's command with its current environment and workspace directory.
fn stdio_command(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
    workspace: Option<&Path>,
) -> Result<tokio::process::Command, Failure> {
    // Resolve the current PATH so programs installed while Drift runs are found without a restart.
    let program = crate::platform::process::which(command)
        .ok_or_else(|| format!("{command} was not found on PATH; install it, or give its full path"))?;
    let mut process = tokio::process::Command::new(program);
    crate::platform::process::use_current_path(&mut process, env);
    process.args(args).envs(env);

    let directory = cwd
        .filter(|cwd| !cwd.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| workspace.map(Path::to_path_buf));
    if let Some(directory) = directory {
        process.current_dir(directory);
    }

    crate::platform::process::prepare(&mut process);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);

    Ok(process)
}

/// Headers as given, any that are not valid HTTP left out.
fn header_map(headers: &BTreeMap<String, String>) -> http::HeaderMap {
    headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                name.parse::<http::HeaderName>().ok()?,
                value.parse::<http::HeaderValue>().ok()?,
            ))
        })
        .collect()
}

fn http_config(url: &str, headers: &BTreeMap<String, String>) -> StreamableHttpClientTransportConfig {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    let mut custom = HashMap::new();

    for (name, value) in headers {
        if name.eq_ignore_ascii_case("authorization") {
            config = config.auth_header(value.trim_start_matches("Bearer ").to_string());
            continue;
        }
        let (Ok(name), Ok(value)) = (name.parse::<http::HeaderName>(), value.parse::<http::HeaderValue>()) else {
            continue;
        };
        custom.insert(name, value);
    }

    config.custom_headers(custom)
}
