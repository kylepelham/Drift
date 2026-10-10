use crate::llm::Chunk;
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct Generation {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Generation {
    pub(super) fn observe(&mut self, chunk: &Chunk) {
        let content = match chunk {
            Chunk::TextStart | Chunk::ReasoningStart | Chunk::ToolUseStart { .. } => true,
            Chunk::TextDelta(text)
            | Chunk::ReasoningDelta(text)
            | Chunk::ToolInputDelta(text)
            | Chunk::ReasoningRedacted(text) => !text.is_empty(),
            Chunk::ReasoningSignature(_) => self.first.is_some(),
            _ => false,
        };
        if content {
            let now = Instant::now();
            self.first.get_or_insert(now);
            self.last = Some(now);
        }
    }

    pub(super) fn milliseconds(&self) -> Option<u64> {
        let elapsed = self.last?.duration_since(self.first?).as_millis();
        u64::try_from(elapsed).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StopReason;
    use crate::session::types::Usage;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn request_waits_and_trailing_usage_do_not_count_as_generation() {
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
    async fn reasoning_and_tool_arguments_share_one_response_clock_but_tool_waits_do_not() {
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
        tokio::time::advance(Duration::from_secs(600)).await;
        assert_eq!(generation.milliseconds(), Some(3_000));

        let mut next = Generation::default();
        next.observe(&Chunk::TextStart);
        tokio::time::advance(Duration::from_secs(2)).await;
        next.observe(&Chunk::TextDelta("done".into()));
        assert_eq!(next.milliseconds(), Some(2_000));
    }

    #[tokio::test(start_paused = true)]
    async fn metadata_and_empty_deltas_do_not_invent_a_generation_window() {
        let mut generation = Generation::default();
        generation.observe(&Chunk::TextDelta(String::new()));
        generation.observe(&Chunk::ReasoningSignature("signature".into()));
        generation.observe(&Chunk::PartSignature("signature".into()));
        generation.observe(&Chunk::Usage(Usage::default()));
        assert_eq!(generation.milliseconds(), None);
    }
}
