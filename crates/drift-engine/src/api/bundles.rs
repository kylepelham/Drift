use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::config::bundles::{Bundle, BundleError, BundlePartKind, InstallBundle};

fn refused(error: BundleError) -> ApiError {
    match error {
        BundleError::NoBundle => ApiError::not_found("bundle"),
        error => ApiError::new(StatusCode::BAD_REQUEST, "bundle", error.to_string()),
    }
}

/// Every installed bundle with its parts and switches.
#[utoipa::path(get, path = "/bundles", operation_id = "listBundles", responses((status = 200, body = Vec<Bundle>)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Json<Vec<Bundle>> {
    Json(engine.bundles())
}

/// Installs a bundle from a registry: the pinned archive is unpacked under the user's config and its MCP servers saved.
#[utoipa::path(post, path = "/bundles", operation_id = "installBundle", request_body = InstallBundle, responses((status = 200, body = Vec<Bundle>), (status = 400)))]
pub(super) async fn install(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<InstallBundle>,
) -> Result<Json<Vec<Bundle>>, ApiError> {
    engine.install_bundle(body).await.map(Json).map_err(refused)
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(super) struct BundleId {
    pub id: String,
}

/// Removes a bundle: its files, its data, its MCP servers and its switches.
#[utoipa::path(delete, path = "/bundles", operation_id = "removeBundle", params(BundleId), responses((status = 200, body = Vec<Bundle>), (status = 404)))]
pub(super) async fn remove(
    State(engine): State<Arc<Engine>>,
    Query(query): Query<BundleId>,
) -> Result<Json<Vec<Bundle>>, ApiError> {
    engine.remove_bundle(&query.id).await.map(Json).map_err(refused)
}

#[derive(Deserialize, ToSchema)]
pub(super) struct BundleEnabled {
    pub id: String,
    pub enabled: bool,
}

/// The bundle's one switch; off, none of its parts apply.
#[utoipa::path(put, path = "/bundles/enabled", operation_id = "setBundleEnabled", request_body = BundleEnabled, responses((status = 200, body = Vec<Bundle>), (status = 404)))]
pub(super) async fn set_enabled(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<BundleEnabled>,
) -> Result<Json<Vec<Bundle>>, ApiError> {
    engine
        .set_bundle_enabled(&body.id, body.enabled)
        .await
        .map(Json)
        .map_err(refused)
}

#[derive(Deserialize, ToSchema)]
pub(super) struct PartSwitch {
    pub id: String,
    pub kind: BundlePartKind,
    /// The part's name as the bundle lists it; ignored for `programs`.
    #[serde(default)]
    pub name: String,
    pub enabled: bool,
}

/// One part's switch inside a bundle.
#[utoipa::path(put, path = "/bundles/parts", operation_id = "setBundlePart", request_body = PartSwitch, responses((status = 200, body = Vec<Bundle>), (status = 404)))]
pub(super) async fn set_part(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<PartSwitch>,
) -> Result<Json<Vec<Bundle>>, ApiError> {
    engine
        .set_bundle_part(&body.id, body.kind, &body.name, body.enabled)
        .await
        .map(Json)
        .map_err(refused)
}
