//! How long a model spent generating one response, on a monotonic clock: from its first content block to
//! its last content event. Waiting for the request to start, usage and stop frames, and anything that
//! happens after the response (tools, retries) are not counted.

use tokio::time::Instant;

use crate::llm::Chunk;

#[derive(Default)]
pub(super) struct Generation {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Generation {
    pub(super) fn observe(&mut self, chunk: &Chunk) {
        if !generated(chunk, self.first.is_some()) {
            return;
        }

        let now = Instant::now();
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    pub(super) fn milliseconds(&self) -> Option<u64> {
        let elapsed = self.last?.duration_since(self.first?).as_millis();
        u64::try_from(elapsed).ok()
    }
}

/// Whether the chunk is something the model generated; a signature only extends a response already going.
fn generated(chunk: &Chunk, started: bool) -> bool {
    match chunk {
        Chunk::TextStart | Chunk::ReasoningStart | Chunk::ToolUseStart { .. } => true,
        Chunk::TextDelta(text)
        | Chunk::ReasoningDelta(text)
        | Chunk::ToolInputDelta(text)
        | Chunk::ReasoningRedacted(text) => !text.is_empty(),
        Chunk::ReasoningSignature(_) => started,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::llm::StopReason;
    use crate::session::types::Usage;

    #[tokio::test(start_paused = true)]
    async fn waiting_for_the_response_and_its_trailing_frames_are_not_generation() {
        let mut generation = Generation::default();

        tokio::time::advance(Duration::from_secs(90)).await;
        generation.observe(&Chunk::Usage(Usage::default()));
        generation.observe(&Chunk::TextStart);

        tokio::time::advance(Duration::from_secs(3)).await;
        generation.observe(&Chunk::TextDelta("answer".into()));

        tokio::time::advance(Duration::from_secs(120)).await;
        generation.observe(&Chunk::BlockStop);
        generation.observe(&Chunk::Usage(Usage {
            output: 60,
            ..Usage::default()
        }));
        generation.observe(&Chunk::Stop(StopReason::EndTurn));

        assert_eq!(generation.milliseconds(), Some(3_000));
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_and_tool_arguments_count_but_the_tool_run_after_does_not() {
        let mut generation = Generation::default();

        generation.observe(&Chunk::ReasoningStart);
        tokio::time::advance(Duration::from_secs(2)).await;
        generation.observe(&Chunk::ReasoningDelta("thinking".into()));
        generation.observe(&Chunk::BlockStop);

        generation.observe(&Chunk::ToolUseStart {
            id: "call".into(),
            name: "bash".into(),
        });
        tokio::time::advance(Duration::from_secs(1)).await;
        generation.observe(&Chunk::ToolInputDelta("{}".into()));
        generation.observe(&Chunk::Stop(StopReason::ToolUse));

        // The shell command runs here, outside the response.
        tokio::time::advance(Duration::from_secs(600)).await;

        assert_eq!(generation.milliseconds(), Some(3_000));
    }

    #[tokio::test(start_paused = true)]
    async fn metadata_and_empty_deltas_alone_are_not_a_response() {
        let mut generation = Generation::default();

        generation.observe(&Chunk::TextDelta(String::new()));
        generation.observe(&Chunk::ReasoningSignature("signature".into()));
        generation.observe(&Chunk::PartSignature("signature".into()));
        generation.observe(&Chunk::Usage(Usage::default()));

        assert_eq!(generation.milliseconds(), None);
    }
}
