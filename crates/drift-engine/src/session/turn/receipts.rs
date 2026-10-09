use sha2::Digest;

use super::*;

impl Engine {
    /// The receipt of a prompt that already landed under the same submission id.
    pub(in crate::session) fn receipt_for(&self, session_id: &str, message_id: &str) -> Result<Receipt, TurnError> {
        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let message = self.store.message(message_id)?.ok_or(TurnError::NoSession)?;

        Ok(Receipt { session, message })
    }

    pub(in crate::session) fn announce(&self, session_id: &str, admitted: Admitted) -> Receipt {
        let Admitted {
            message,
            parts,
            session,
            discarded,
        } = admitted;
        for message_id in discarded {
            self.hub.publish(Event::MessageRemoved {
                session_id: session_id.into(),
                message_id,
            });
        }

        self.hub.publish(Event::MessageCreated {
            message: message.clone(),
        });
        for row in parts {
            self.hub.publish(Event::PartCreated { part: row });
        }
        self.hub.publish(Event::SessionUpdated {
            session: session.clone(),
        });

        Receipt { session, message }
    }

    /// A known submission id replays its receipt from storage, so a retry after a restart is still one prompt.
    pub(in crate::session) fn replayed_receipt(
        &self,
        id: &str,
        session_id: &str,
        payload_hash: &str,
    ) -> Result<Option<Receipt>, TurnError> {
        let Some(found) = self.store.submission(id)? else {
            return Ok(None);
        };
        if found.session_id != session_id || found.payload_hash != payload_hash {
            return Err(TurnError::SubmissionReused);
        }

        let session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let message = self.store.message(&found.message_id)?.ok_or(TurnError::NoSession)?;

        Ok(Some(Receipt { session, message }))
    }
}

/// Identity of a prompt for replay checks: the same id must carry the same parts and model.
pub(in crate::session) fn payload_hash(prompt: &Prompt) -> String {
    // Wrapping distinguishes an absent variant from a variant explicitly cleared to the default.
    let variant = prompt
        .variant
        .as_ref()
        .map(|chosen| serde_json::json!({ "chosen": chosen }));
    let body = serde_json::json!({
        "parts": prompt.parts,
        "model": prompt.model,
        "variant": variant,
        "agent": prompt.agent,
    });
    let digest = sha2::Sha256::digest(body.to_string().as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hash, "{byte:02x}");
    }

    hash
}
