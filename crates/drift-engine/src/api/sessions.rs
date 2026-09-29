use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::error::ApiError;
use crate::event::Event;
use crate::session::turn::{Prompt, Receipt};
use crate::session::types::{MessageWithParts, ModelRef, Session, Visibility};
use crate::store::{NewSession, SessionFilter};
use crate::Engine;

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(Deserialize, IntoParams)]
pub struct ListQuery {
    /// Restrict to one workspace; omit for every workspace.
    pub workspace: Option<String>,
    /// Archived sessions instead of live ones.
    #[serde(default)]
    pub archived: bool,
    /// Page: sessions updated before this session id.
    pub before: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionBody {
    pub workspace_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// uild unless the workspace defines others; see the workspace config.
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatchSession {
    pub title: Option<String>,
    pub model: Option<ModelRef>,
    pub agent: Option<String>,
    pub archived: Option<bool>,
}

#[derive(Deserialize, IntoParams)]
pub struct MessagesQuery {
    /// Page: messages before this message id.
    pub before: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct Aborted {
    pub aborted: bool,
}

#[utoipa::path(get, path = "/sessions", operation_id = "listSessions", params(ListQuery), responses((status = 200, body = Vec<Session>)))]
pub async fn list(State(engine): State<Arc<Engine>>, Query(query): Query<ListQuery>) -> Result<Json<Vec<Session>>, ApiError> {
    let mut sessions = engine.store.sessions(SessionFilter {
        workspace_id: query.workspace.as_deref(),
        archived: query.archived,
        before: query.before.as_deref(),
        limit: query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT),
    })?;
    for session in &mut sessions {
        session.running = engine.turns.is_running(&session.id);
    }
    Ok(Json(sessions))
}

#[utoipa::path(post, path = "/sessions", operation_id = "createSession", request_body = NewSessionBody, responses((status = 201, body = Session)))]
pub async fn create(State(engine): State<Arc<Engine>>, Json(body): Json<NewSessionBody>) -> Result<(StatusCode, Json<Session>), ApiError> {
    engine.store.workspace(&body.workspace_id)?.ok_or_else(|| ApiError::not_found("workspace"))?;
    let session = engine.store.create_session(NewSession {
        workspace_id: &body.workspace_id,
        parent_id: None,
        visibility: Visibility::Sibling,
        title: &body.title,
        agent: body.agent.as_deref().unwrap_or("build"),
        model: body.model.as_ref(),
    })?;
    engine.hub.publish(Event::SessionCreated { session: session.clone() });
    Ok((StatusCode::CREATED, Json(session)))
}

#[utoipa::path(get, path = "/sessions/{id}", operation_id = "getSession", responses((status = 200, body = Session), (status = 404)))]
pub async fn get(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<Json<Session>, ApiError> {
    let mut session = engine.store.session(&id)?.ok_or_else(|| ApiError::not_found("session"))?;
    session.running = engine.turns.is_running(&id);
    Ok(Json(session))
}

#[utoipa::path(patch, path = "/sessions/{id}", operation_id = "updateSession", request_body = PatchSession, responses((status = 200, body = Session), (status = 404)))]
pub async fn update(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<PatchSession>) -> Result<Json<Session>, ApiError> {
    let mut session = engine
        .store
        .update_session(&id, body.title.as_deref(), body.model.as_ref(), body.agent.as_deref())?
        .ok_or_else(|| ApiError::not_found("session"))?;
    if let Some(archived) = body.archived {
        if archived {
            engine.abort(&id);
            engine.permissions.forget_session(&id);
        }
        session = engine.store.set_session_archived(&id, archived)?.ok_or_else(|| ApiError::not_found("session"))?;
    }
    engine.hub.publish(Event::SessionUpdated { session: session.clone() });
    Ok(Json(session))
}

#[utoipa::path(get, path = "/sessions/{id}/messages", operation_id = "listMessages", params(MessagesQuery), responses((status = 200, body = Vec<MessageWithParts>), (status = 404)))]
pub async fn messages(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Vec<MessageWithParts>>, ApiError> {
    engine.store.session(&id)?.ok_or_else(|| ApiError::not_found("session"))?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    Ok(Json(engine.store.messages(&id, query.before.as_deref(), limit)?))
}

#[utoipa::path(post, path = "/sessions/{id}/turns", operation_id = "submitTurn", request_body = Prompt, responses((status = 202, body = Receipt), (status = 409), (status = 404)))]
pub async fn submit(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(prompt): Json<Prompt>) -> Result<(StatusCode, Json<Receipt>), ApiError> {
    Ok((StatusCode::ACCEPTED, Json(engine.submit(&id, prompt).await?)))
}

#[utoipa::path(post, path = "/sessions/{id}/abort", operation_id = "abortTurn", responses((status = 200, body = Aborted)))]
pub async fn abort(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Json<Aborted> {
    Json(Aborted { aborted: engine.abort(&id) })
}

#[utoipa::path(get, path = "/sessions/{id}/todos", operation_id = "listTodos", responses((status = 200, body = Vec<crate::session::types::Todo>), (status = 404)))]
pub async fn todos(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<Json<Vec<crate::session::types::Todo>>, ApiError> {
    engine.store.session(&id)?.ok_or_else(|| ApiError::not_found("session"))?;
    Ok(Json(engine.store.todos(&id)?))
}

/// Permanent removal, for archived sessions past their retention. Live turns are aborted first.
#[utoipa::path(delete, path = "/sessions/{id}", operation_id = "deleteSession", responses((status = 204), (status = 404)))]
pub async fn delete(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    engine.abort(&id);
    engine.permissions.forget_session(&id);
    if !engine.store.delete_session(&id)? {
        return Err(ApiError::not_found("session"));
    }
    engine.hub.publish(Event::SessionDeleted { session_id: id });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct CommandBody {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default)]
    pub model: Option<ModelRef>,
}

/// Expands a workspace command's template and submits it as a turn.
#[utoipa::path(post, path = "/sessions/{id}/command", operation_id = "runCommand", request_body = CommandBody, responses((status = 202, body = Receipt), (status = 404)))]
pub async fn command(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<CommandBody>) -> Result<(StatusCode, Json<Receipt>), ApiError> {
    let session = engine.store.session(&id)?.ok_or_else(|| ApiError::not_found("session"))?;
    let workspace = engine.store.workspace(&session.workspace_id)?.ok_or_else(|| ApiError::not_found("workspace"))?;
    let config = crate::config::Config::load(&crate::tool::canonical(std::path::Path::new(&workspace.path)));
    let command = config.commands.iter().find(|c| c.name == body.name).ok_or_else(|| ApiError::not_found("command"))?;
    let text = command.template.replace("$ARGUMENTS", body.arguments.trim());
    let prompt = Prompt { parts: vec![crate::session::types::Part::Text { text }], model: body.model, thinking_budget: None, submission_id: None };
    Ok((StatusCode::ACCEPTED, Json(engine.submit(&id, prompt).await?)))
}
