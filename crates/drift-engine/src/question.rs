//! Questions the model asks the user mid-turn; answered over the socket or HTTP like permissions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::event::{Event, Hub};
use crate::id;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Option_ {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Question {
    pub question: String,
    /// Short label shown as the card title.
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub options: Vec<Option_>,
    #[serde(default)]
    pub multiple: bool,
    /// Whether the user may type an answer that is not one of the options.
    #[serde(default = "default_true")]
    pub custom: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = QuestionRequest)]
pub struct Request {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub questions: Vec<Question>,
    pub created_at: i64,
    /// The turn went on without waiting; the answer arrives as its own prompt.
    #[serde(rename = "async", default)]
    pub is_async: bool,
    /// The session's Stop count when asked; a Stop since keeps the answer from starting a turn.
    #[serde(skip)]
    pub generation: i64,
}

/// One list of chosen labels per question; `None` means the user declined to answer.
pub type Answers = Option<Vec<Vec<String>>>;

/// Pending questions live in this process only: a restart drops the cards, never a saved answer.
#[derive(Default)]
pub struct Questions {
    /// A blocking question's call waits on its sender; an async one has none.
    pending: Mutex<Vec<(Request, Option<oneshot::Sender<Answers>>)>>,
    /// Held while a request is being answered or dismissed, so its decisions take turns.
    decisions: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Debug, PartialEq)]
pub struct NotPending;

impl Questions {
    pub async fn ask(&self, hub: &Hub, request: Request, abort: &CancellationToken) -> Answers {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().push((request.clone(), Some(tx)));
        hub.publish(Event::QuestionAsked {
            request: request.clone(),
        });
        let answers = tokio::select! {
            answers = rx => answers.ok().flatten(),
            () = abort.cancelled() => None,
        };
        self.pending
            .lock()
            .unwrap()
            .retain(|(pending, _)| pending.id != request.id);
        answers
    }

    /// Registers a question nobody waits on; its answer is handled by `Engine::answer_question`.
    pub fn ask_async(&self, hub: &Hub, request: Request) {
        self.pending.lock().unwrap().push((request.clone(), None));
        hub.publish(Event::QuestionAsked { request });
    }

    pub fn lookup(&self, request_id: &str) -> Option<Request> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .find(|(request, _)| request.id == request_id)
            .map(|(request, _)| request.clone())
    }

    /// Answers or declines a blocking question: its call gets the answer and goes on.
    pub fn reply(&self, hub: &Hub, request_id: &str, answers: Answers) -> Result<(), NotPending> {
        let mut pending = self.pending.lock().unwrap();
        let index = pending
            .iter()
            .position(|(request, tx)| request.id == request_id && tx.is_some())
            .ok_or(NotPending)?;
        let (request, tx) = pending.remove(index);
        drop(pending);
        if let Some(tx) = tx {
            let _ = tx.send(answers);
        }
        hub.publish(Event::QuestionReplied {
            request_id: request.id,
            session_id: request.session_id,
        });
        Ok(())
    }

    /// Closes an async question once its answer is saved (or it was dismissed).
    pub fn settle_async(&self, hub: &Hub, request: &Request) {
        self.pending
            .lock()
            .unwrap()
            .retain(|(pending, _)| pending.id != request.id);
        hub.publish(Event::QuestionReplied {
            request_id: request.id.clone(),
            session_id: request.session_id.clone(),
        });
    }

    /// The lock a decision on `request_id` holds; the same one for every caller while any holds it.
    pub fn decision(&self, request_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut decisions = self.decisions.lock().unwrap();
        // Locks nobody holds or waits on are dropped as new ones are made.
        decisions.retain(|_, lock| Arc::strong_count(lock) > 1);
        decisions.entry(request_id.into()).or_default().clone()
    }

    /// A deleted session's async questions go with it; blocking ones end with their turn.
    pub fn forget_session(&self, session_id: &str) {
        self.pending
            .lock()
            .unwrap()
            .retain(|(request, tx)| request.session_id != session_id || tx.is_some());
    }

    pub fn pending(&self) -> Vec<Request> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(request, _)| request.clone())
            .collect()
    }
}

pub fn new_request(session_id: &str, message_id: &str, call_id: &str, questions: Vec<Question>) -> Request {
    Request {
        id: id::new("q"),
        session_id: session_id.into(),
        message_id: message_id.into(),
        call_id: call_id.into(),
        questions,
        created_at: id::now_ms(),
        is_async: false,
        generation: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        new_request(
            "ses",
            "msg",
            "call",
            vec![Question {
                question: "Which?".into(),
                header: "Pick".into(),
                options: vec![],
                multiple: false,
                custom: true,
            }],
        )
    }

    #[tokio::test]
    async fn asks_and_delivers_answers() {
        let hub = Hub::new(8);
        let questions = Questions::default();
        let abort = CancellationToken::new();
        let request = request();
        let (answers, ()) = tokio::join!(questions.ask(&hub, request.clone(), &abort), async {
            tokio::task::yield_now().await;
            assert_eq!(questions.pending().len(), 1);
            questions
                .reply(&hub, &request.id, Some(vec![vec!["a".into()]]))
                .unwrap();
        });
        assert_eq!(answers, Some(vec![vec!["a".into()]]));
        assert!(questions.pending().is_empty());
        assert_eq!(questions.reply(&hub, "q_nope", None), Err(NotPending));
    }

    #[tokio::test]
    async fn abort_and_reject_give_no_answer() {
        let hub = Hub::new(8);
        let questions = Questions::default();
        let abort = CancellationToken::new();
        let (answers, ()) = tokio::join!(questions.ask(&hub, request(), &abort), async {
            tokio::task::yield_now().await;
            abort.cancel();
        });
        assert_eq!(answers, None);
    }
}
