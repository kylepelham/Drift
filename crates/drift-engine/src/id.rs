//! Time-ordered ids: creation order is row order, even for rows made in the same millisecond.

use std::sync::atomic::{AtomicU64, Ordering};

/// Last issued timestamp in the low 52 bits and a counter above it; never moves backwards.
static LAST: AtomicU64 = AtomicU64::new(0);
const COUNTER_BITS: u32 = 12;

pub fn new(prefix: &str) -> String {
    format!("{prefix}_{:016x}{}", stamp(), crate::random_hex(4))
}

/// The next point in id order, for rows that must sort against ids without being one.
pub fn stamp() -> i64 {
    let now = (now_ms() as u64) << COUNTER_BITS;
    let mut last = LAST.load(Ordering::SeqCst);
    loop {
        let next = if now > last { now } else { last + 1 };
        match LAST.compare_exchange_weak(last, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next as i64,
            Err(seen) => last = seen,
        }
    }
}

/// The point in id order an id was made at.
pub fn stamp_of(id: &str) -> Option<i64> {
    let (_, rest) = id.split_once('_')?;
    u64::from_str_radix(rest.get(..16)?, 16).ok().map(|stamp| stamp as i64)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
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

    #[test]
    fn a_stamp_sorts_between_the_ids_made_around_it() {
        let before = super::new("msg");
        let stamp = super::stamp();
        let after = super::new("msg");
        assert!(super::stamp_of(&before).unwrap() < stamp && stamp < super::stamp_of(&after).unwrap());
    }
}
