//! Time-ordered ids: a millisecond prefix keeps rows sortable, random bytes keep them unique.

pub fn new(prefix: &str) -> String {
    format!("{prefix}_{:012x}{}", now_ms(), crate::random_hex(6))
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
    fn ids_sort_by_creation() {
        let first = super::new("ses");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = super::new("ses");
        assert!(first < second);
        assert_eq!(first.len(), "ses_".len() + 24);
    }
}
