//! Time-ordered ids: creation order is row order, even for rows made in the same millisecond.

use std::sync::atomic::{AtomicU64, Ordering};

/// Last issued timestamp in the low 52 bits and a counter above it; never moves backwards.
static LAST: AtomicU64 = AtomicU64::new(0);
const COUNTER_BITS: u32 = 12;

pub fn new(prefix: &str) -> String {
    let now = (now_ms() as u64) << COUNTER_BITS;
    let stamp = LAST
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(if now > last { now } else { last + 1 }))
        .map(|last| if now > last { now } else { last + 1 })
        .unwrap_or(now);
    format!("{prefix}_{stamp:016x}{}", crate::random_hex(4))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_sort_by_creation_even_within_a_millisecond() {
        let ids: Vec<String> = (0..50).map(|_| super::new("ses")).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        assert_eq!(ids[0].len(), "ses_".len() + 24);
    }
}
