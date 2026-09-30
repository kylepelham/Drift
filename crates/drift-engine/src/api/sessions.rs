use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::error::ApiError;
use crate::event::Event;
use crate::session::branch::BranchDraft;
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
    /// `build` unless the workspace defines others; see the workspace config.
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

#[derive(Deserialize, ToSchema)]
pub struct RetryModelBody {
    pub model: ModelRef,
}

/// Moves a turn that is waiting to retry onto another model; it retries at once and the session keeps
/// the model. 409 when nothing is waiting to retry; 400 or 401 when the model cannot be used.
#[utoipa::path(post, path = "/sessions/{id}/retry", operation_id = "switchRetryModel", request_body = RetryModelBody, responses((status = 204), (status = 400), (status = 401), (status = 409)))]
pub async fn switch_retry_model(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<RetryModelBody>) -> Result<StatusCode, ApiError> {
    engine.switch_retry_model(&id, &body.model).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevertBody {
    /// The prompt to go back to; it and everything after it are hidden.
    pub message_id: String,
}

/// Undoes the conversation back to a prompt, files included. Again while undone moves the point.
#[utoipa::path(post, path = "/sessions/{id}/revert", operation_id = "revertSession", request_body = RevertBody, responses((status = 200, body = Session), (status = 400), (status = 404), (status = 409)))]
pub async fn revert(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<RevertBody>) -> Result<Json<Session>, ApiError> {
    Ok(Json(engine.revert(&id, &body.message_id).await?))
}

/// Redoes everything an undo hid, files included.
#[utoipa::path(post, path = "/sessions/{id}/unrevert", operation_id = "unrevertSession", responses((status = 200, body = Session), (status = 404), (status = 409)))]
pub async fn unrevert(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<Json<Session>, ApiError> {
    Ok(Json(engine.unrevert(&id).await?))
}

/// Summarises the older history now. Runs as the session's job: 409 while a turn runs, Stop cancels it.
#[utoipa::path(post, path = "/sessions/{id}/compact", operation_id = "compactSession", responses((status = 202), (status = 404), (status = 409)))]
pub async fn compact(State(engine): State<Arc<Engine>>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    engine.start_compaction(&id)?;
    Ok(StatusCode::ACCEPTED)
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
    let config = engine.workspace_config(&crate::tool::canonical(std::path::Path::new(&workspace.path)));
    let command = config.commands.iter().find(|c| c.name == body.name).ok_or_else(|| ApiError::not_found("command"))?;
    let text = command.template.replace("$ARGUMENTS", body.arguments.trim());
    let prompt = Prompt { parts: vec![crate::session::types::Part::Text { text }], model: body.model, thinking_budget: None, submission_id: None };
    Ok((StatusCode::ACCEPTED, Json(engine.submit(&id, prompt).await?)))
}

#[derive(Deserialize, ToSchema)]
pub struct BranchGoal {
    pub goal: String,
}

/// Drafts the handoff for a branch. Makes one model request; stores nothing.
#[utoipa::path(post, path = "/sessions/{id}/branch/draft", operation_id = "draftBranch", request_body = BranchGoal, responses((status = 200, body = BranchDraft), (status = 400), (status = 404), (status = 502)))]
pub async fn draft_branch(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<BranchGoal>) -> Result<Json<BranchDraft>, ApiError> {
    Ok(Json(engine.draft_branch(&id, &body.goal).await?))
}

/// Creates a reviewed branch and starts it. The new conversation is independent of its source.
#[utoipa::path(post, path = "/sessions/{id}/branch", operation_id = "createBranch", request_body = BranchDraft, responses((status = 201, body = Session), (status = 400), (status = 404)))]
pub async fn branch(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(draft): Json<BranchDraft>) -> Result<(StatusCode, Json<Session>), ApiError> {
    Ok((StatusCode::CREATED, Json(engine.branch(&id, draft).await?)))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForkBody {
    /// Copy through this message; default is the last finished one, leaving out a turn in flight.
    #[serde(default)]
    pub at_message: Option<String>,
}

/// Copies finished history into a new, independent conversation.
#[utoipa::path(post, path = "/sessions/{id}/fork", operation_id = "forkSession", request_body = ForkBody, responses((status = 201, body = Session), (status = 400), (status = 404)))]
pub async fn fork(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<ForkBody>) -> Result<(StatusCode, Json<Session>), ApiError> {
    Ok((StatusCode::CREATED, Json(engine.fork(&id, body.at_message.as_deref())?)))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveBody {
    pub workspace_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct Moved {
    /// The session and the subagents that moved with it.
    pub moved: Vec<String>,
}

/// Moves a session and its subagents to another workspace. 409 while any of them is running.
#[utoipa::path(post, path = "/sessions/{id}/move", operation_id = "moveSession", request_body = MoveBody, responses((status = 200, body = Moved), (status = 404), (status = 409)))]
pub async fn move_session(State(engine): State<Arc<Engine>>, Path(id): Path<String>, Json(body): Json<MoveBody>) -> Result<Json<Moved>, ApiError> {
    Ok(Json(Moved { moved: engine.move_session(&id, &body.workspace_id)? }))
}