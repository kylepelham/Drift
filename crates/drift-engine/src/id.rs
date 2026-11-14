//! Time-ordered ids: creation order is row order, even for rows made in the same millisecond.

use std::sync::atomic::{AtomicU64, Ordering};

/// Last issued timestamp and counter; the timestamp occupies the high bits and never moves backwards.
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

/// Whether `id` has the shape ids are made in, a prefix, `_` and the rest in letters and digits, so it is safe
/// as one folder name; one read from a restored backup may not be.
pub fn well_formed(id: &str) -> bool {
    let plain = |part: &str| !part.is_empty() && part.chars().all(|char| char.is_ascii_alphanumeric() || char == '_');

    id.len() <= 64
        && id
            .split_once('_')
            .is_some_and(|(prefix, rest)| plain(prefix) && plain(rest))
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

    #[test]
    fn only_an_id_of_the_made_shape_is_well_formed() {
        assert!(super::well_formed(&super::new("ses")));
        assert!(super::well_formed("ses_someone_else"));
        for odd in [
            "",
            "ses",
            "_x",
            "ses_",
            "../../x",
            "ses_../x",
            "C:\\x",
            "ses_a/b",
            "ses_a.b",
            "ses_\u{e9}",
        ] {
            assert!(!super::well_formed(odd), "{odd}");
        }
        assert!(!super::well_formed(&format!("ses_{}", "a".repeat(64))));
    }
}
