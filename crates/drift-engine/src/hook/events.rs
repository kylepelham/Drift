use std::sync::Arc;

use super::{SessionEvent, SessionKind};
use crate::event::{Event, SessionStatus};

/// Session events as the hub publishes them, handed to the hooks until the hub closes.
pub async fn relay_session_events(engine: Arc<crate::Engine>) {
    let mut receiver = engine.hub.attach(None).rx;

    loop {
        let envelope = match receiver.recv().await {
            Ok(envelope) => envelope,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        if engine.hooks.is_empty() {
            continue;
        }

        let event = match envelope.event {
            Event::SessionCreated { session } => session_event(&engine, &session, SessionKind::Created),
            Event::SessionUpdated { session } => session_event(&engine, &session, SessionKind::Updated),
            Event::SessionDeleted { session_id } => SessionEvent {
                id: session_id,
                workspace: String::new(),
                title: String::new(),
                agent: String::new(),
                kind: SessionKind::Deleted,
            },
            Event::SessionStatusChanged { session_id, status } => {
                let kind = if status == SessionStatus::Running {
                    SessionKind::Running
                } else {
                    SessionKind::Idle
                };
                let Some(session) = engine.store.session(&session_id).ok().flatten() else {
                    continue;
                };

                session_event(&engine, &session, kind)
            }
            _ => continue,
        };

        engine.hooks.session(&event).await;
    }
}

pub(crate) fn session_event(
    engine: &crate::Engine,
    session: &crate::session::types::Session,
    kind: SessionKind,
) -> SessionEvent {
    let workspace = engine
        .store
        .workspace(&session.workspace_id)
        .ok()
        .flatten()
        .map(|workspace| workspace.path)
        .unwrap_or_default();

    SessionEvent {
        id: session.id.clone(),
        workspace,
        title: session.title.clone(),
        agent: session.agent.clone(),
        kind,
    }
}
