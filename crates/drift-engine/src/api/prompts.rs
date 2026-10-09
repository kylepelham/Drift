//! The base prompt each model family starts with, and the user's replacements: one for every model
//! (`all`) and one per family, which wins over it. A turn reads them when it builds its system prompt.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::Engine;
use crate::llm::catalog::PromptFamily;
use crate::session::prompt;

/// A replacement longer than this is refused; the prompt is sent with every request.
const MAX_PROMPT_BYTES: usize = 64 * 1024;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct BasePrompt {
    /// `all` (every model, unless its family has its own) or a family: `codex`, `claude`, `gemini`, `default`.
    pub id: String,
    /// Drift's text; empty for `all`, which has none of its own.
    pub default: String,
    /// The user's replacement, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct BasePrompts {
    pub prompts: Vec<BasePrompt>,
    /// Follows every base prompt, Drift's or the user's, and cannot be replaced: tools, `<system-reminder>`, the worktree, the answer's shape.
    pub shared: String,
}

#[derive(Deserialize, ToSchema)]
pub(super) struct PromptBody {
    pub text: String,
}

fn ids() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once((prompt::ALL_MODELS, "")).chain(
        PromptFamily::ALL
            .into_iter()
            .map(|family| (family.as_str(), prompt::family_prompt(family).trim())),
    )
}

fn known(id: &str) -> Result<(), ApiError> {
    if ids().any(|(known, _)| known == id) {
        return Ok(());
    }
    Err(ApiError::not_found("base prompt"))
}

fn listed(engine: &Engine) -> Result<BasePrompts, ApiError> {
    let mut prompts = Vec::new();
    for (id, default) in ids() {
        let custom = engine.store.setting::<String>(&prompt::custom_key(id))?;
        prompts.push(BasePrompt {
            id: id.into(),
            default: default.into(),
            custom,
        });
    }

    Ok(BasePrompts {
        prompts,
        shared: prompt::shared_rules().into(),
    })
}

#[utoipa::path(get, path = "/prompts", operation_id = "listBasePrompts", responses((status = 200, body = BasePrompts)))]
pub(super) async fn list(State(engine): State<Arc<Engine>>) -> Result<Json<BasePrompts>, ApiError> {
    Ok(Json(listed(&engine)?))
}

/// Replaces base prompt `id` from the next turn on; the shared rules still follow it.
#[utoipa::path(put, path = "/prompts/{id}", operation_id = "saveBasePrompt", request_body = PromptBody, responses((status = 200, body = BasePrompts), (status = 400), (status = 404)))]
pub(super) async fn save(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Result<Json<BasePrompts>, ApiError> {
    known(&id)?;

    if body.text.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty",
            "a base prompt cannot be empty; reset it to use Drift's",
        ));
    }
    if body.text.len() > MAX_PROMPT_BYTES {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "too_long",
            format!("a base prompt holds at most {} KB", MAX_PROMPT_BYTES / 1024),
        ));
    }

    engine.store.set_setting(&prompt::custom_key(&id), &body.text)?;
    Ok(Json(listed(&engine)?))
}

/// Goes back to Drift's text for `id` (or, for a family, to the `all` replacement when there is one).
#[utoipa::path(delete, path = "/prompts/{id}", operation_id = "resetBasePrompt", responses((status = 200, body = BasePrompts), (status = 404)))]
pub(super) async fn reset(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
) -> Result<Json<BasePrompts>, ApiError> {
    known(&id)?;
    engine.store.remove_setting(&prompt::custom_key(&id))?;
    Ok(Json(listed(&engine)?))
}
