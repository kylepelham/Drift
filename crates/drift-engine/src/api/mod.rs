//! HTTP and WebSocket surface. Every route is documented in the OpenAPI the TS client is built from.

mod auth;
mod browser;
mod bundles;
mod cors;
mod error;
mod events;
pub use events::Lease;
mod health;
mod images;
mod local_servers;
mod mcp;
mod permissions;
mod processes;
mod prompts;
mod provider_accounts;
mod providers;
mod questions;
mod queue;
mod review;
mod sessions;
mod settings;
mod voice;
mod websearch;
mod workspaces;

use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::Engine;

#[derive(OpenApi)]
#[openapi(
    info(title = "Drift Engine", version = crate::VERSION),
    components(schemas(events::Frame, events::Incoming, error::ErrorBody, crate::browser::ViewInput))
)]
struct Api;

fn documented() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::with_openapi(Api::openapi())
        .routes(routes!(health::get))
        .merge(workspace_routes())
        .merge(session_routes())
        .merge(session_action_routes())
        .merge(settings_routes())
        .merge(media_routes())
        .merge(plugin_routes())
        .merge(skill_routes())
        .merge(bundle_routes())
        .merge(provider_routes())
        .merge(permission_routes())
        .merge(question_routes())
        .merge(task_routes())
        .merge(browser_routes())
        .merge(mcp_routes())
        .merge(review_routes())
        .routes(routes!(events::get))
}

fn workspace_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(workspaces::list, workspaces::create))
        .routes(routes!(workspaces::config))
        .routes(routes!(workspaces::files))
        .routes(routes!(workspaces::purge))
}

fn session_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(sessions::list, sessions::create))
        .routes(routes!(sessions::get, sessions::update, sessions::delete))
        .routes(routes!(sessions::messages))
        .routes(routes!(sessions::submit))
        .routes(routes!(queue::list, queue::add))
        .routes(routes!(queue::edit, queue::discard))
        .routes(routes!(queue::send_now))
        .routes(routes!(sessions::abort))
        .routes(routes!(sessions::command))
}

fn session_action_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(sessions::spawn))
        .routes(routes!(sessions::fork))
        .routes(routes!(sessions::move_session))
        .routes(routes!(sessions::compact))
        .routes(routes!(sessions::switch_retry_model))
        .routes(routes!(sessions::revert))
        .routes(routes!(sessions::unrevert))
        .routes(routes!(sessions::export_sessions))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(sessions::import_sessions))
                .layer(axum::extract::DefaultBodyLimit::max(MAX_BACKUP_BYTES)),
        )
}

fn settings_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(settings::get, settings::put))
        .routes(routes!(settings::tools))
        .routes(routes!(settings::fetch_registry))
        .routes(routes!(websearch::get, websearch::put))
        .routes(routes!(websearch::add_key, websearch::remove_key))
}

/// Image generation and voice: settings, keys, and the requests they make.
fn media_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(images::get, images::put))
        .routes(routes!(images::set_key))
        .routes(routes!(images::blob))
        .routes(routes!(voice::get))
        .routes(routes!(voice::set_key, voice::remove_key))
        .routes(routes!(voice::transcribe))
        .routes(routes!(voice::speak))
}

fn plugin_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(settings::plugins))
        .routes(routes!(settings::reload_plugins))
        .routes(routes!(settings::set_plugin_enabled))
        .routes(routes!(settings::install_plugin))
        .routes(routes!(settings::remove_plugin))
        .routes(routes!(settings::configure_plugin))
}

fn skill_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(
            settings::skill_packs,
            settings::install_skill_pack,
            settings::remove_skill_pack
        ))
        .routes(routes!(settings::skills))
        .routes(routes!(settings::set_skill_enabled))
}

fn bundle_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(bundles::list, bundles::install, bundles::remove))
        .routes(routes!(bundles::set_enabled))
        .routes(routes!(bundles::set_part))
}

fn provider_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(prompts::list))
        .routes(routes!(prompts::save, prompts::reset))
        .routes(routes!(providers::list))
        .routes(routes!(providers::set_key))
        .routes(routes!(providers::remove))
        .routes(routes!(providers::oauth_start))
        .routes(routes!(providers::oauth_finish))
        .routes(routes!(local_servers::get, local_servers::set))
        .routes(routes!(provider_accounts::reorder))
        .routes(routes!(provider_accounts::change, provider_accounts::remove))
}

fn permission_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(permissions::list))
        .routes(routes!(permissions::reply))
        .routes(routes!(permissions::grants, permissions::revoke_all))
        .routes(routes!(permissions::revoke))
        .routes(routes!(permissions::rules, permissions::save_rules))
}

fn question_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(questions::list))
        .routes(routes!(questions::reply))
        .routes(routes!(questions::reject))
}

fn task_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(sessions::todos))
        .routes(routes!(sessions::tasks))
        .routes(routes!(sessions::task))
        .routes(routes!(sessions::abort_task))
        .routes(routes!(processes::list))
        .routes(routes!(processes::stop))
}

/// A conversation's browser and its view socket, and what Settings > Browser manages.
fn browser_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(browser::get))
        .routes(routes!(browser::stop))
        .routes(routes!(browser::view))
        .routes(routes!(browser::engines))
        .routes(routes!(browser::download))
        .routes(routes!(browser::set_path))
        .routes(routes!(browser::workspace))
        .routes(routes!(browser::save_credential, browser::remove_credential))
}

fn mcp_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(mcp::list))
        .routes(routes!(mcp::save, mcp::remove))
        .routes(routes!(mcp::rename))
        .routes(routes!(mcp::connect_route))
        .routes(routes!(mcp::disconnect))
        .routes(routes!(mcp::set_enabled))
        .routes(routes!(mcp::sign_in, mcp::sign_out))
}

fn review_routes() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::new()
        .routes(routes!(review::turn))
        .routes(routes!(review::workspace))
        .routes(routes!(review::refs))
}

/// The largest request the engine reads: prompts carry attachments as base64 (a 40 MB video is 53 MB); axum's default is 2 MB.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
/// A restored backup carries whole conversations, attachments included.
pub const MAX_BACKUP_BYTES: usize = 1024 * 1024 * 1024;

pub fn router(engine: Arc<Engine>) -> Router {
    let (router, openapi) = documented().split_for_parts();
    router
        .route("/openapi.json", get(move || async move { Json(openapi) }))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(axum::middleware::from_fn_with_state(
            engine.clone(),
            auth::require_token,
        ))
        // Outside auth so browser preflights, which carry no token, are answered.
        .layer(cors::layer())
        .with_state(engine)
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    documented().split_for_parts().1
}

#[cfg(test)]
mod tests;
