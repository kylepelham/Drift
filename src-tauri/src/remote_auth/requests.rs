//! Devices asking to connect. Each waits for this computer's user to accept or decline it, while both screens show
//! the code that proves no other device sits between them (`drift_pairing`).

use drift_pairing::Secret;
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How long a request waits for an answer.
pub(crate) const REQUEST_TTL: Duration = Duration::from_secs(120);
/// Requests waiting at once; any more are refused until one is answered.
const MAX_WAITING: usize = 3;

#[derive(Debug, PartialEq, thiserror::Error)]
pub(crate) enum RequestError {
    #[error("Too many devices are asking to connect. Try again in a minute.")]
    TooMany,
    #[error("That request has expired. Try connecting again.")]
    Unknown,
}

/// Where a request stands, as its device waits to hear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Answer {
    Waiting,
    Accepted,
    Declined,
    /// Cancelled, expired, or replaced by a newer request from the same address.
    Gone,
}

struct Request {
    name: String,
    address: IpAddr,
    secret: Secret,
    /// Set once the device sent its own secret, which is when the request is shown.
    code: Option<String>,
    expires: Instant,
    answer: watch::Sender<Answer>,
}

/// A request shown to this computer's user.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Prompt {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) code: String,
    pub(crate) address: String,
    pub(crate) expires_at: i64,
}

#[derive(Default)]
pub(crate) struct Requests {
    waiting: HashMap<String, Request>,
}

impl Requests {
    /// A new request: its id, and the commitment to this computer's secret. A newer request from an address replaces
    /// its last.
    pub(crate) fn open(
        &mut self,
        name: String,
        address: IpAddr,
        secret: Secret,
        now: Instant,
    ) -> Result<(String, [u8; 32]), RequestError> {
        self.prune(now);
        let replaced: Vec<String> = self
            .waiting
            .iter()
            .filter(|(_, request)| request.address == address)
            .map(|(id, _)| id.clone())
            .collect();
        for id in replaced {
            self.end(&id, Answer::Gone);
        }
        if self.waiting.len() >= MAX_WAITING {
            return Err(RequestError::TooMany);
        }

        let id = super::random_hex(16);
        let (answer, _) = watch::channel(Answer::Waiting);
        self.waiting.insert(
            id.clone(),
            Request {
                name,
                address,
                secret,
                code: None,
                expires: now + REQUEST_TTL,
                answer,
            },
        );

        Ok((id, drift_pairing::commitment(&secret)))
    }

    /// Takes the device's secret, once, and reveals this computer's; the request is shown from now on.
    pub(crate) fn reveal(
        &mut self,
        id: &str,
        device: &Secret,
        authority: &[u8; 32],
        now: Instant,
    ) -> Result<Secret, RequestError> {
        self.prune(now);
        let request = self.waiting.get_mut(id).ok_or(RequestError::Unknown)?;
        if request.code.is_some() {
            return Err(RequestError::Unknown);
        }
        request.code = Some(drift_pairing::matching_code(authority, device, &request.secret));

        Ok(request.secret)
    }

    /// What the device waits on; `None` once the request is gone.
    pub(crate) fn watch(&self, id: &str) -> Option<watch::Receiver<Answer>> {
        Some(self.waiting.get(id)?.answer.subscribe())
    }

    /// The user's answer to a shown request; the address it came from, so declines count toward its lockout.
    pub(crate) fn answer(&mut self, id: &str, accept: bool) -> Option<IpAddr> {
        let request = self.waiting.get(id).filter(|request| request.code.is_some())?;
        let address = request.address;
        request
            .answer
            .send_replace(if accept { Answer::Accepted } else { Answer::Declined });

        Some(address)
    }

    /// Ends an answered request for its device; the name to list an accepted device by.
    pub(crate) fn settle(&mut self, id: &str) -> Option<(Answer, String)> {
        let answer = *self.waiting.get(id)?.answer.borrow();
        if answer == Answer::Waiting {
            return None;
        }

        self.waiting.remove(id).map(|request| (answer, request.name))
    }

    pub(crate) fn cancel(&mut self, id: &str) -> bool {
        self.end(id, Answer::Gone)
    }

    /// Requests the user is asked about now, oldest first.
    pub(crate) fn prompts(&mut self, now: Instant) -> Vec<Prompt> {
        self.prune(now);
        let mut prompts: Vec<(Instant, Prompt)> = self
            .waiting
            .iter()
            .filter(|(_, request)| *request.answer.borrow() == Answer::Waiting)
            .filter_map(|(id, request)| {
                let left = request.expires.saturating_duration_since(now);
                let prompt = Prompt {
                    id: id.clone(),
                    name: request.name.clone(),
                    code: request.code.clone()?,
                    address: request.address.to_string(),
                    expires_at: super::now_ms() + left.as_millis() as i64,
                };
                Some((request.expires, prompt))
            })
            .collect();
        prompts.sort_by_key(|(expires, _)| *expires);

        prompts.into_iter().map(|(_, prompt)| prompt).collect()
    }

    fn end(&mut self, id: &str, answer: Answer) -> bool {
        let Some(request) = self.waiting.remove(id) else {
            return false;
        };
        request.answer.send_replace(answer);

        true
    }

    fn prune(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .waiting
            .iter()
            .filter(|(_, request)| request.expires <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.end(&id, Answer::Gone);
        }
    }
}
