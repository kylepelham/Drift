//! Today's date where the user is, which is what a model is told "today" means.

/// The local date as `YYYY-MM-DD`; the UTC date only if the system will not say.
pub fn local_date() -> String {
    let (year, month, day) = local().unwrap_or_else(utc);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(windows)]
fn local() -> Option<(i64, u32, u32)> {
    // SAFETY: GetLocalTime only writes the struct it is given.
    let now = unsafe {
        let mut now = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut now);
        now
    };
    Some((i64::from(now.wYear), u32::from(now.wMonth), u32::from(now.wDay)))
}

#[cfg(unix)]
fn local() -> Option<(i64, u32, u32)> {
    // SAFETY: time(NULL) reads the clock; localtime_r writes only the struct it is given.
    let mut now = unsafe { std::mem::zeroed::<libc::tm>() };
    if unsafe { libc::localtime_r(&libc::time(std::ptr::null_mut()), &mut now) }.is_null() {
        return None;
    }
    Some((
        i64::from(now.tm_year) + 1900,
        u32::try_from(now.tm_mon + 1).ok()?,
        u32::try_from(now.tm_mday).ok()?,
    ))
}

#[cfg(not(any(windows, unix)))]
fn local() -> Option<(i64, u32, u32)> {
    None
}

fn utc() -> (i64, u32, u32) {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0))
}

/// Howard Hinnant's algorithm; avoids pulling a date crate in for one line.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_is_the_local_one_and_well_formed() {
        let date = local_date();
        assert!(
            date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-',
            "{date}"
        );
        assert!(local().is_some(), "the system gives a local date");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}
