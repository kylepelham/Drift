use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine as _;
use serde::Serialize;
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::imagegen::{ImageGeneration, ImageProvider, PROVIDERS, SETTINGS_KEY, Size};

/// Settings > Image generation: the choice, and each provider with its models and whether it has a key.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImageGenerationState {
    pub settings: ImageGeneration,
    pub providers: Vec<ImageProvider>,
}

fn state(engine: &Engine) -> Json<ImageGenerationState> {
    Json(ImageGenerationState {
        settings: engine.image_settings(),
        providers: engine.image_providers(),
    })
}

#[utoipa::path(get, path = "/image-generation", operation_id = "getImageGeneration", responses((status = 200, body = ImageGenerationState)))]
pub(super) async fn get(State(engine): State<Arc<Engine>>) -> Json<ImageGenerationState> {
    state(&engine)
}

/// Chooses the provider (`off` or an id), its model (empty for the default) and the default size.
#[utoipa::path(put, path = "/image-generation", operation_id = "putImageGeneration", request_body = ImageGeneration, responses((status = 200, body = ImageGenerationState), (status = 400)))]
pub(super) async fn put(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<ImageGeneration>,
) -> Result<Json<ImageGenerationState>, ApiError> {
    if body.provider != "off" && !PROVIDERS.iter().any(|provider| provider.id == body.provider) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "provider",
            format!("{} is not an image provider", body.provider),
        ));
    }
    Size::parse(&body.size).map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, "size", error))?;
    engine.store.set_setting(SETTINGS_KEY, &body)?;

    Ok(state(&engine))
}

/// An image a tool call returned, which its metadata names by hash.
#[derive(Serialize, ToSchema)]
pub(super) struct BlobImage {
    pub mime: String,
    /// Base64.
    pub data: String,
}

#[utoipa::path(get, path = "/blobs/{hash}", operation_id = "getBlob", responses((status = 200, body = BlobImage), (status = 404)))]
pub(super) async fn blob(
    State(engine): State<Arc<Engine>>,
    Path(hash): Path<String>,
) -> Result<Json<BlobImage>, ApiError> {
    let bytes = engine.store.blob(&hash)?.ok_or_else(|| ApiError::not_found("image"))?;
    let mime = crate::tool::image::sniff(&bytes).unwrap_or("application/octet-stream");

    Ok(Json(BlobImage {
        mime: mime.into(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    }))
}
