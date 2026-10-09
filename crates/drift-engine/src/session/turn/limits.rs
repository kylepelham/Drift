use super::*;

/// Words that mark a shell command as waiting on purpose, the way polling does.
const WAITS: [&str; 5] = ["sleep", "start-sleep", "timeout", "wait", "watch"];

impl WrapUp {
    pub(super) fn instruction(self) -> String {
        let why = match self {
            WrapUp::Steps(_) => "This is the last step this turn allows".to_string(),
            WrapUp::Repeats(times) => format!("Your last {times} steps made the same calls and got the same results"),
        };

        format!(
            "<system-reminder>\n{why}, so tools are off for this reply. Answer with text only, no tool calls: \
             say that the turn stops here, summarise what you found and did, list what is still undone, \
             and say what should happen next.\n</system-reminder>"
        )
    }

    pub(super) fn pause_reason(self) -> String {
        match self {
            WrapUp::Steps(steps) => {
                format!("Paused after {steps} steps, this turn's limit. Send a message to carry on.")
            }
            WrapUp::Repeats(times) => format!(
                "Paused: the last {times} steps made the same calls and got the same results. \
                 Send a message to carry on or change course."
            ),
        }
    }
}

impl Repeats {
    /// `Some(times)` once the same step has come back as many times in a row as the limit allows.
    pub(super) fn record(&mut self, calls: Vec<CallTrace>, limits: &crate::config::Limits) -> Option<u32> {
        if calls.is_empty() {
            *self = Self::default();
            return None;
        }

        if calls == self.last {
            self.count += 1;
        } else {
            self.last = calls;
            self.count = 1;
        }
        let limit = if self.last.iter().any(waits) {
            limits.polls
        } else {
            limits.repeats
        };

        (self.count >= limit).then_some(self.count)
    }
}

pub(super) fn waits(call: &CallTrace) -> bool {
    let Ok(input) = serde_json::from_str::<serde_json::Value>(&call.input) else {
        return false;
    };

    let command = input["command"].as_str().unwrap_or_default().to_ascii_lowercase();
    let mut words = command.split(|character: char| !character.is_ascii_alphanumeric() && character != '-');

    call.name == "bash" && words.any(|word| WAITS.contains(&word))
}
