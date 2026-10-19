//! A server asking the user for input in the middle of a call (MCP elicitation), answered on the question card.
//! The card blocks the call, names the server, and is asked in the session whose call it is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use tokio_util::sync::CancellationToken;

use crate::question::{Option_, Question, new_request};
use crate::tool::Context;

mod form;

/// A call in flight on one connection: whose it is, so a server's question reaches the right session.
#[derive(Clone)]
struct Caller {
    id: u64,
    engine: Arc<crate::Engine>,
    server: String,
    session_id: String,
    message_id: String,
    call_id: String,
    abort: CancellationToken,
}

/// The calls in flight on one connection; a server's question goes to the newest.
#[derive(Default)]
pub(super) struct Callers {
    next: AtomicU64,
    list: Mutex<Vec<Caller>>,
}

/// Holds a call's place among its connection's callers until the call ends.
pub(super) struct Entered {
    callers: Arc<Callers>,
    id: u64,
}

impl Drop for Entered {
    fn drop(&mut self) {
        self.callers.list.lock().unwrap().retain(|caller| caller.id != self.id);
    }
}

impl Callers {
    pub(super) fn enter(self: &Arc<Self>, ctx: &Context, server: &str) -> Entered {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.list.lock().unwrap().push(Caller {
            id,
            engine: ctx.engine.clone(),
            server: server.to_string(),
            session_id: ctx.session_id.clone(),
            message_id: ctx.message_id.clone(),
            call_id: ctx.call_id.clone(),
            abort: ctx.abort.clone(),
        });

        Entered {
            callers: self.clone(),
            id,
        }
    }

    fn newest(&self) -> Option<Caller> {
        self.list.lock().unwrap().last().cloned()
    }
}

/// Asks the user and answers the server: accept with the content, decline on a dismissed card, cancel on Stop.
/// A question with no call to belong to is declined.
pub(super) async fn answer(callers: &Callers, request: ElicitRequestParams) -> ElicitResult {
    let Some(caller) = callers.newest() else {
        return ElicitResult::new(ElicitationAction::Decline);
    };

    match request {
        ElicitRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } => {
            let schema = serde_json::to_value(&requested_schema).unwrap_or_default();
            let order = requested_schema.property_order.clone();
            ask_form(&caller, &message, &form::fields(&schema, order)).await
        }
        ElicitRequestParams::UrlElicitationParams { message, url, .. } => ask_url(&caller, &message, &url).await,
        _ => ElicitResult::new(ElicitationAction::Decline),
    }
}

/// Asks until the answers fit the schema, the card is dismissed, or the turn stops.
async fn ask_form(caller: &Caller, message: &str, fields: &[form::Field]) -> ElicitResult {
    let mut problem: Option<String> = None;
    loop {
        let mut questions = form::questions(fields, &caller.server);
        let lead = match &problem {
            Some(problem) => format!("{problem}\n\n{message}"),
            None => message.to_string(),
        };
        questions[0].question = format!("{lead}\n\n{}", questions[0].question).trim().to_string();

        let answers = match ask(caller, questions).await {
            Ok(answers) => answers,
            Err(action) => return ElicitResult::new(action),
        };
        match form::content(fields, &answers) {
            Ok(content) => return ElicitResult::new(ElicitationAction::Accept).with_content(content),
            Err(why) => problem = Some(why),
        }
    }
}

async fn ask_url(caller: &Caller, message: &str, url: &str) -> ElicitResult {
    let question = Question {
        question: format!("{message}\n\nOpen {url} to go on."),
        header: caller.server.clone(),
        options: vec![Option_ {
            label: form::DONE.into(),
            description: String::new(),
        }],
        multiple: false,
        custom: false,
    };

    match ask(caller, vec![question]).await {
        Ok(_) => ElicitResult::new(ElicitationAction::Accept),
        Err(action) => ElicitResult::new(action),
    }
}

/// The card's answers, or what to tell the server instead: declined when dismissed, cancelled on Stop.
async fn ask(caller: &Caller, questions: Vec<Question>) -> Result<Vec<Vec<String>>, ElicitationAction> {
    let request = new_request(&caller.session_id, &caller.message_id, &caller.call_id, questions);
    let engine = &caller.engine;
    let answers = engine.questions.ask(&engine.hub, request, &caller.abort).await;
    if caller.abort.is_cancelled() {
        return Err(ElicitationAction::Cancel);
    }

    answers.ok_or(ElicitationAction::Decline)
}
