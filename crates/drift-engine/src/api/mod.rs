//! HTTP and WebSocket surface. Every route is documented in the OpenAPI the TS client is built from.

mod auth;
mod events;
mod health;
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
    components(schemas(events::Frame))
)]
struct Api;

fn documented() -> OpenApiRouter<Arc<Engine>> {
    OpenApiRouter::with_openapi(Api::openapi())
        .routes(routes!(health::get))
        .routes(routes!(workspaces::list, workspaces::create))
        .routes(routes!(events::get))
}

pub fn router(engine: Arc<Engine>) -> Router {
    let (router, openapi) = documented().split_for_parts();
    router
        .route("/openapi.json", get(move || async move { Json(openapi) }))
        .layer(axum::middleware::from_fn_with_state(engine.clone(), auth::require_token))
        .with_state(engine)
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    documented().split_for_parts().1
}

#[cfg(test)]
mod tests;
