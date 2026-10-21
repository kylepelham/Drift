use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::websearch::{SETTINGS_KEY, SearchProvider, WebSearch};

/// Settings > Web search: the choice, and every provider with its keys shown by their last characters.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WebSearchState {
    pub settings: WebSearch,
    pub providers: Vec<SearchProvider>,
}

fn state(engine: &Engine) -> Json<WebSearchState> {
    Json(WebSearchState {
        settings: engine.web_search_settings(),
        providers: engine.search_providers(),
    })
}

#[utoipa::path(get, path = "/websearch", operation_id = "getWebSearch", responses((status = 200, body = WebSearchState)))]
pub(super) async fn get(State(engine): State<Arc<Engine>>) -> Json<WebSearchState> {
    state(&engine)
}

/// Chooses the provider (`auto`, `off` or an id) and the order `auto` tries them in.
#[utoipa::path(put, path = "/websearch", operation_id = "putWebSearch", request_body = WebSearch, responses((status = 200, body = WebSearchState)))]
pub(super) async fn put(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<WebSearch>,
) -> Result<Json<WebSearchState>, ApiError> {
    let known: Vec<String> = engine
        .search_providers()
        .into_iter()
        .map(|provider| provider.id)
        .collect();
    if !["auto", "off"].contains(&body.provider.as_str()) && !known.contains(&body.provider) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "provider",
            format!("{} is not a web search provider", body.provider),
        ));
    }
    engine.store.set_setting(SETTINGS_KEY, &body)?;

    Ok(state(&engine))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchKey {
    /// A built-in provider's id.
    pub provider: String,
    pub key: String,
}

/// Adds a key to a built-in provider's list; keys set aside as refused are all tried again.
#[utoipa::path(post, path = "/websearch/keys", operation_id = "addWebSearchKey", request_body = SearchKey, responses((status = 200, body = WebSearchState)))]
pub(super) async fn add_key(
    State(engine): State<Arc<Engine>>,
    Json(body): Json<SearchKey>,
) -> Result<Json<WebSearchState>, ApiError> {
    engine
        .add_search_key(&body.provider, &body.key)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, "key", error))?;

    Ok(state(&engine))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct KeyAt {
    pub provider: String,
    /// The key's place in the provider's list, from 0.
    pub index: usize,
}

#[utoipa::path(delete, path = "/websearch/keys", operation_id = "removeWebSearchKey", params(KeyAt), responses((status = 200, body = WebSearchState)))]
pub(super) async fn remove_key(
    State(engine): State<Arc<Engine>>,
    Query(query): Query<KeyAt>,
) -> Result<Json<WebSearchState>, ApiError> {
    engine
        .remove_search_key(&query.provider, query.index)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, "key", error))?;

    Ok(state(&engine))
}
