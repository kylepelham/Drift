//! A provider's accounts: several sign-ins used in order, each renamed, marked cyber verified, moved or signed out
//! on its own.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use super::providers::{credentials_changed, credentials_error};
use crate::Engine;
use crate::llm::Credential;
use crate::llm::credentials::Account;
use crate::llm::limits::Limits;

/// One stored credential of a provider, in the order the engine uses them.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderAccount {
    pub id: String,
    /// What the user named it, else the email it signed in with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A subscription sign-in, which takes turns with the provider's others; else an API key.
    pub signed_in: bool,
    /// What it last reported of its subscription's usage; absent until it has answered a request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// The user says it is in its provider's cyber program; after a cyber refusal only such accounts are tried.
    pub cyber_verified: bool,
}

impl ProviderAccount {
    pub(super) fn of(engine: &Engine, account: Account) -> Self {
        let signed_in = matches!(engine.credentials.account(&account.key), Some(Credential::OAuth { .. }));

        Self {
            limits: engine.limits.get(&account.key),
            id: account.key,
            label: account.label,
            signed_in,
            cyber_verified: account.cyber_verified,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub(super) struct AccountOrderBody {
    /// Every account id, first used first.
    pub order: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountChangeBody {
    /// A new name; empty clears it. Left out, the name stays.
    #[serde(default)]
    pub label: Option<String>,
    /// Whether it is in its provider's cyber program. Left out, it stays as it is.
    #[serde(default)]
    pub cyber_verified: Option<bool>,
}

#[utoipa::path(put, path = "/providers/{id}/accounts", operation_id = "reorderProviderAccounts", request_body = AccountOrderBody, responses((status = 204), (status = 400)))]
pub(super) async fn reorder(
    State(engine): State<Arc<Engine>>,
    Path(id): Path<String>,
    Json(body): Json<AccountOrderBody>,
) -> Result<StatusCode, ApiError> {
    let reordered = engine
        .credentials
        .reorder_accounts(&id, &body.order)
        .map_err(credentials_error)?;
    if !reordered {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "name every account once",
        ));
    }

    credentials_changed(&engine, &id);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(patch, path = "/providers/{id}/accounts/{account}", operation_id = "changeProviderAccount", request_body = AccountChangeBody, responses((status = 204), (status = 400), (status = 404)))]
pub(super) async fn change(
    State(engine): State<Arc<Engine>>,
    Path((id, account)): Path<(String, String)>,
    Json(body): Json<AccountChangeBody>,
) -> Result<StatusCode, ApiError> {
    let credentials = &engine.credentials;
    if !credentials.accounts(&id).iter().any(|listed| listed.key == account) {
        return Err(ApiError::not_found("account"));
    }
    // Only Anthropic grants its program to an account; OpenAI's Daybreak is a model the user picks.
    if body.cyber_verified.is_some() && id != "anthropic" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "only Anthropic accounts can be cyber verified",
        ));
    }

    if let Some(label) = &body.label {
        credentials
            .rename_account(&id, &account, label)
            .map_err(credentials_error)?;
    }
    if let Some(verified) = body.cyber_verified {
        credentials
            .set_cyber_verified(&id, &account, verified)
            .map_err(credentials_error)?;
    }

    engine.hub.publish(crate::event::Event::CatalogUpdated {});
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/providers/{id}/accounts/{account}", operation_id = "removeProviderAccount", responses((status = 204), (status = 404)))]
pub(super) async fn remove(
    State(engine): State<Arc<Engine>>,
    Path((id, account)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    if !engine
        .credentials
        .remove_account(&id, &account)
        .map_err(credentials_error)?
    {
        return Err(ApiError::not_found("account"));
    }

    credentials_changed(&engine, &id);
    Ok(StatusCode::NO_CONTENT)
}
