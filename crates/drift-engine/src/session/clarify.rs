//! Answers to questions the model asked without waiting, saved as their own prompt (see docs/engine-rewrite.md).

use std::sync::Arc;

use super::turn::{Admission, Prompt, TurnError};
use super::types::{Clarified, Part};
use crate::Engine;
use crate::question::{Answers, Request};

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
    /// Saves an async answer before closing its question card, so failed saves leave the card answerable.
    pub async fn answer_question(self: &Arc<Self>, request_id: &str, answers: Answers) -> Result<(), AnswerError> {
        // One decision per request at a time: an answer and a dismissal never both go through.
        let decision = self.questions.decision(request_id);
        let _deciding = decision.lock().await;
        let Some(request) = self.questions.lookup(request_id) else {
            return self.already_answered(request_id, answers);
        };
        if !request.is_async {
            return self
                .questions
                .reply(&self.hub, request_id, answers)
                .map_err(|_| AnswerError::NotPending);
        }

        let Some(answers) = answers else {
            self.questions.settle_async(&self.hub, &request);
            return Ok(());
        };

        let prompt = Prompt {
            parts: vec![answer_part(&request, &answers)],
            model: None,
            variant: None,
            agent: None,
            submission_id: Some(format!("answer:{}", request.id)),
        };
        self.deliver_answer(&request, prompt).await?;
        self.questions.settle_async(&self.hub, &request);

        Ok(())
    }

    /// A question no longer pending, settled by its saved answer, which outlives the card and a restart.
    fn already_answered(&self, request_id: &str, answers: Answers) -> Result<(), AnswerError> {
        let Some(saved) = self
            .store
            .submission(&format!("answer:{request_id}"))
            .map_err(TurnError::from)?
        else {
            return Err(AnswerError::NotPending);
        };

        let transcript = self.store.transcript(&saved.session_id).map_err(TurnError::from)?;
        let parts = transcript
            .into_iter()
            .find(|m| m.info.id == saved.message_id)
            .map(|m| m.parts)
            .unwrap_or_default();
        let given = parts.into_iter().find_map(|row| match row.part {
            Part::Clarification { items, .. } => Some(items.into_iter().map(|item| item.answers).collect::<Vec<_>>()),
            _ => None,
        });

        match (given, answers) {
            (Some(given), Some(again)) if given == again => Ok(()),
            _ => Err(AnswerError::Conflict),
        }
    }

    /// Joins or starts a turn unless Stop or another job prevents admission; then the answer is only saved.
    async fn deliver_answer(self: &Arc<Self>, request: &Request, prompt: Prompt) -> Result<(), AnswerError> {
        if let Some(scope) = self.scope_at(&request.session_id, request.generation) {
            match self
                .admit(
                    &request.session_id,
                    prompt.clone(),
                    Admission {
                        parent: Some(&scope),
                        ..Admission::default()
                    },
                )
                .await
            {
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
        if let Some((id, hash)) = submission
            && self.replayed_receipt(id, session_id, hash)?.is_some()
        {
            return Ok(());
        }

        let session = self
            .store
            .session(session_id)
            .map_err(TurnError::from)?
            .ok_or(TurnError::NoSession)?;
        let model = session.model.ok_or(TurnError::NoModel)?;

        match self.admit_fenced(
            session_id,
            super::turn::FencedPrompt {
                pick: crate::store::Pick::model(&model),
                parts: prompt.parts,
                submission,
                abort: None,
                delivery: None,
            },
        ) {
            Ok(admitted) => {
                self.announce(session_id, admitted);
                Ok(())
            }
            Err(TurnError::Replayed(_)) => Ok(()),
            Err(TurnError::SubmissionReused) => Err(AnswerError::Conflict),
            Err(error) => Err(error.into()),
        }
    }
}

fn answer_part(request: &Request, answers: &[Vec<String>]) -> Part {
    let items = request
        .questions
        .iter()
        .zip(answers.iter().chain(std::iter::repeat(&Vec::new())))
        .map(|(question, chosen)| Clarified {
            header: question.header.clone(),
            question: question.question.clone(),
            answers: chosen.clone(),
        })
        .collect();

    Part::Clarification {
        request_id: request.id.clone(),
        items,
    }
}

#[cfg(test)]
mod tests;
