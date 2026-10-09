use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, ToSchema)]
pub(super) struct Health {
    pub version: String,
}

#[utoipa::path(get, path = "/health", operation_id = "health", responses((status = 200, body = Health)))]
pub(super) async fn get() -> Json<Health> {
    Json(Health {
        version: crate::VERSION.into(),
    })
}
