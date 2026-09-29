//! Questions the model asks the user mid-turn; answered over the socket or HTTP like permissions.

use std::sync::Mutex;

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
}

/// One list of chosen labels per question; `None` means the user declined to answer.
pub type Answers = Option<Vec<Vec<String>>>;

#[derive(Default)]
pub struct Questions {
    pending: Mutex<Vec<(Request, oneshot::Sender<Answers>)>>,
}

#[derive(Debug, PartialEq)]
pub struct NotPending;

impl Questions {
    pub async fn ask(&self, hub: &Hub, request: Request, abort: &CancellationToken) -> Answers {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().push((request.clone(), tx));
        hub.publish(Event::QuestionAsked { request: request.clone() });
        let answers = tokio::select! {
            answers = rx => answers.ok().flatten(),
            () = abort.cancelled() => None,
        };
        self.pending.lock().unwrap().retain(|(pending, _)| pending.id != request.id);
        answers
    }

    pub fn reply(&self, hub: &Hub, request_id: &str, answers: Answers) -> Result<(), NotPending> {
        let mut pending = self.pending.lock().unwrap();
        let index = pending.iter().position(|(request, _)| request.id == request_id).ok_or(NotPending)?;
        let (request, tx) = pending.remove(index);
        drop(pending);
        let _ = tx.send(answers);
        hub.publish(Event::QuestionReplied { request_id: request.id, session_id: request.session_id });
        Ok(())
    }

    pub fn pending(&self) -> Vec<Request> {
        self.pending.lock().unwrap().iter().map(|(request, _)| request.clone()).collect()
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        new_request("ses", "msg", "call", vec![Question { question: "Which?".into(), header: "Pick".into(), options: vec![], multiple: false, custom: true }])
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
            questions.reply(&hub, &request.id, Some(vec![vec!["a".into()]])).unwrap();
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
