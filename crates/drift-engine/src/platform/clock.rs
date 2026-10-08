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
    // SAFETY: time accepts a null output pointer and localtime_r receives live input and output storage.
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
        .map_or(0, |duration| duration.as_secs());
    civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0))
}

/// Howard Hinnant's algorithm; avoids pulling a date crate in for one line.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);

    let march_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * march_month + 2) / 5 + 1) as u32;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);

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
