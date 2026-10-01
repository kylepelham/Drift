//! Answers to questions the model asked without waiting: saved as their own prompt, once, and a turn
//! started for them only while the session has not been stopped since the question was asked.

use std::sync::Arc;

use super::turn::{Admission, Prompt, TurnError};
use super::types::{Clarified, Part};
use crate::question::{Answers, Request};
use crate::Engine;

#[derive(Debug, PartialEq)]
pub enum AnswerError {
    NotPending,
    /// The question was already answered differently.
    Conflict,
    Turn(TurnError),
}

impl From<TurnError> for AnswerError {
    fn from(error: TurnError) -> Self {
        Self::Turn(error)
    }
}

impl Engine {
    /// Answers or declines a question. A blocking one hands the answer to its waiting call; an async
    /// one is saved as a prompt before its card closes, so a failed save leaves it answerable.
    pub async fn answer_question(self: &Arc<Self>, request_id: &str, answers: Answers) -> Result<(), AnswerError> {
        let Some(request) = self.questions.lookup(request_id) else {
            return match (self.questions.answered(request_id), answers) {
                (Some(saved), Some(again)) if saved == again => Ok(()),
                (Some(_), _) => Err(AnswerError::Conflict),
                (None, _) => Err(AnswerError::NotPending),
            };
        };
        if !request.is_async {
            return self.questions.reply(&self.hub, request_id, answers).map_err(|_| AnswerError::NotPending);
        }
        // Dismissed: the card closes and nothing is said or started.
        let Some(answers) = answers else {
            self.questions.settle_async(&self.hub, &request, None);
            return Ok(());
        };
        let prompt = Prompt { parts: vec![answer_part(&request, &answers)], model: None, thinking_budget: None, submission_id: Some(format!("answer:{}", request.id)) };
        self.deliver_answer(&request, prompt).await?;
        self.questions.settle_async(&self.hub, &request, Some(answers));
        Ok(())
    }

    /// Joins the running turn, or starts one; after a Stop since the question, or with the session busy, it is only saved.
    async fn deliver_answer(self: &Arc<Self>, request: &Request, prompt: Prompt) -> Result<(), AnswerError> {
        if let Some(scope) = self.scope_at(&request.session_id, request.generation) {
            match self.admit(&request.session_id, prompt.clone(), Admission { parent: Some(&scope), ..Admission::default() }).await {
                Ok(_) => return Ok(()),
                Err(TurnError::Stopped | TurnError::Busy) => {}
                Err(TurnError::SubmissionReused) => return Err(AnswerError::Conflict),
                Err(error) => return Err(error.into()),
            }
        }
        self.save_without_turn(&request.session_id, prompt)
    }

    /// Writes the prompt into the conversation for its next turn to read, starting nothing.
    fn save_without_turn(&self, session_id: &str, prompt: Prompt) -> Result<(), AnswerError> {
        let hash = super::turn::payload_hash(&prompt);
        let submission = prompt.submission_id.as_deref().map(|id| (id, hash.as_str()));
        if let Some((id, hash)) = submission {
            if self.replayed_receipt(id, session_id, hash)?.is_some() {
                return Ok(());
            }
        }
        let session = self.store.session(session_id).map_err(TurnError::from)?.ok_or(TurnError::NoSession)?;
        let model = session.model.ok_or(TurnError::NoModel)?;
        let admitted = self.admit_fenced(session_id, &model, prompt.parts, submission, None, None)?;
        self.announce(session_id, admitted);
        Ok(())
    }
}

fn answer_part(request: &Request, answers: &[Vec<String>]) -> Part {
    let items = request
        .questions
        .iter()
        .zip(answers.iter().chain(std::iter::repeat(&Vec::new())))
        .map(|(question, chosen)| Clarified { header: question.header.clone(), question: question.question.clone(), answers: chosen.clone() })
        .collect();
    Part::Clarification { request_id: request.id.clone(), items }
}

#[cfg(test)]
mod tests;
