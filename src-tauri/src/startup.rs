//! Monotonic startup milestones, available when launching Drift with stderr captured.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub(crate) fn mark(phase: &str) {
    let start = START.get_or_init(Instant::now);
    eprintln!("drift startup: {phase} {}ms", start.elapsed().as_millis());
}
