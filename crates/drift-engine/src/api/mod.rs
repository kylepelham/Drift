//! HTTP and WebSocket surface. Every route is documented in the OpenAPI the TS client is built from.

mod auth;
mod cors;
mod error;
mod events;
mod health;
mod permissions;
mod providers;
mod questions;
mod sessions;
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
    components(schemas(events::Frame, events::Incoming, error::ErrorBody))
)]
struct Api;

fn documented() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::with_openapi(Api::openapi())
        .routes(routes!(health::get))
        .routes(routes!(workspaces::list, workspaces::create))
        .routes(routes!(sessions::list, sessions::create))
        .routes(routes!(sessions::get, sessions::update))
        .routes(routes!(sessions::messages))
        .routes(routes!(sessions::submit))
        .routes(routes!(sessions::abort))
        .routes(routes!(providers::list))
        .routes(routes!(providers::set_key))
        .routes(routes!(providers::remove))
        .routes(routes!(providers::oauth_start))
        .routes(routes!(providers::oauth_finish))
        .routes(routes!(permissions::list))
        .routes(routes!(permissions::reply))
        .routes(routes!(questions::list))
        .routes(routes!(questions::reply))
        .routes(routes!(questions::reject))
        .routes(routes!(sessions::todos))
        .routes(routes!(events::get))
}

pub fn router(engine: Arc<Engine>) -> Router {
    let (router, openapi) = documented().split_for_parts();
    router
        .route("/openapi.json", get(move || async move { Json(openapi) }))
        .layer(axum::middleware::from_fn_with_state(engine.clone(), auth::require_token))
        // Outside auth so browser preflights, which carry no token, are answered.
        .layer(cors::layer())
        .with_state(engine)
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    documented().split_for_parts().1
}

#[cfg(test)]
mod tests;
