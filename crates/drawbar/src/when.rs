//! Moments as the user's clock shows them: in the time zone of the OS on the desktop,
//! and of the browser on the web.

/// The minutes east of UTC the user's clock read at `ms` milliseconds after the Unix
/// epoch, where the platform says.
#[cfg(target_arch = "wasm32")]
pub fn local_offset(ms: u64) -> Option<i32> {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let behind = date.get_timezone_offset();
    behind.is_finite().then(|| -(behind as i32))
}

/// The minutes east of UTC the user's clock read at `ms` milliseconds after the Unix
/// epoch, where the platform says.
#[cfg(unix)]
pub fn local_offset(ms: u64) -> Option<i32> {
    let seconds = libc::time_t::try_from(ms / 1000).ok()?;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: `localtime_r` reads only `seconds`, and fills `tm` whole where it returns
    // it.
    let filled = unsafe { libc::localtime_r(&seconds, tm.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }
    // SAFETY: filled above.
    let tm = unsafe { tm.assume_init() };
    i32::try_from(tm.tm_gmtoff / 60).ok()
}

/// The minutes east of UTC the user's clock read at `ms` milliseconds after the Unix
/// epoch, where the platform says.
#[cfg(windows)]
pub fn local_offset(ms: u64) -> Option<i32> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    // A FILETIME counts 100 ns from 1601-01-01 UTC.
    const EPOCH: u64 = 116_444_736_000_000_000;
    let ticks = ms.checked_mul(10_000)?.checked_add(EPOCH)?;
    let split = |ticks: u64| FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let joined =
        |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    let zero = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    let (mut utc, mut local) = (zero, zero);
    let mut back = split(0);
    // SAFETY: each call reads and writes only the values passed, and a null zone names
    // the one the OS is set to.
    let converted = unsafe {
        FileTimeToSystemTime(&split(ticks), &mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
            && SystemTimeToFileTime(&local, &mut back) != 0
    };
    if !converted {
        return None;
    }
    let ahead = i64::try_from(joined(back)).ok()? - i64::try_from(ticks).ok()?;
    i32::try_from(ahead.div_euclid(600_000_000)).ok()
}

/// When `from` to `to`, in milliseconds after the Unix epoch, was, as "on 23 Oct 2026,
/// 14:05", "on 23 Oct 2026, 14:05 to 16:30" or "between … and …" says it, in the zone
/// `offset` gives each moment, in minutes east of UTC. In UTC, and saying so, where it
/// gives none.
pub fn span(from: u64, to: u64, offset: impl Fn(u64) -> Option<i32>) -> String {
    let (zones, utc) = match (offset(from), offset(to)) {
        (Some(early), Some(late)) => ((early, late), ""),
        _ => ((0, 0), " UTC"),
    };
    let ((first, at), (last, until)) = (moment(from, zones.0), moment(to, zones.1));
    match (first == last, at == until) {
        (true, true) => format!("on {first}, {at}{utc}"),
        (true, false) => format!("on {first}, {at} to {until}{utc}"),
        (false, _) => format!("between {first}, {at} and {last}, {until}{utc}"),
    }
}

/// The day and the time of day of `ms` milliseconds after the Unix epoch, `offset`
/// minutes east of UTC, as "23 Oct 2026" and "14:05" say them.
fn moment(ms: u64, offset: i32) -> (String, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // u64::MAX ms is under 2^49 minutes, so the cast is exact.
    let minutes = (ms / 60_000) as i64 + i64::from(offset);
    let (days, of_day) = (minutes.div_euclid(1_440), minutes.rem_euclid(1_440));
    // Days to a civil date, as Howard Hinnant's `civil_from_days` counts them.
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        format!("{day} {} {year}", MONTHS[(month - 1) as usize]),
        format!("{:02}:{:02}", of_day / 60, of_day % 60),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 23 Oct 2026, 23:30 UTC.
    const LATE: u64 = 1_792_798_200_000;

    #[test]
    fn a_moment_is_said_in_the_zone_its_offset_gives() {
        assert_eq!(span(LATE, LATE, |_| Some(0)), "on 23 Oct 2026, 23:30");
        assert_eq!(span(LATE, LATE, |_| Some(60)), "on 24 Oct 2026, 00:30");
        assert_eq!(span(LATE, LATE, |_| Some(-300)), "on 23 Oct 2026, 18:30");
        assert_eq!(span(LATE, LATE, |_| Some(330)), "on 24 Oct 2026, 05:00");
    }

    #[test]
    fn a_span_names_its_day_once_where_it_ends_on_the_day_it_began() {
        let earlier = LATE - 2 * 3_600_000;
        assert_eq!(
            span(earlier, LATE, |_| Some(0)),
            "on 23 Oct 2026, 21:30 to 23:30"
        );
        assert_eq!(
            span(earlier, LATE, |_| Some(60)),
            "between 23 Oct 2026, 22:30 and 24 Oct 2026, 00:30"
        );
    }

    #[test]
    fn each_end_of_a_span_takes_its_own_offset() {
        let summer = 1_792_713_600_000;
        let winter = summer + 14 * 86_400_000;
        let zone = |ms| Some(if ms < summer + 86_400_000 { 120 } else { 60 });
        assert_eq!(
            span(summer, winter, zone),
            "between 23 Oct 2026, 02:00 and 6 Nov 2026, 01:00"
        );
    }

    #[test]
    fn a_span_with_no_zone_is_said_in_utc() {
        assert_eq!(span(LATE, LATE, |_| None), "on 23 Oct 2026, 23:30 UTC");
    }

    #[test]
    fn the_platform_gives_an_offset_within_fourteen_hours() {
        let offset = local_offset(LATE);
        assert!(
            offset.is_some_and(|minutes| minutes.abs() <= 14 * 60),
            "{offset:?}"
        );
    }

    #[test]
    fn days_are_counted_across_leap_years_and_centuries() {
        let day = |ms| moment(ms, 0).0;
        assert_eq!(day(0), "1 Jan 1970");
        assert_eq!(day(951_868_800_000), "1 Mar 2000");
        assert_eq!(day(1_709_251_199_000), "29 Feb 2024");
    }
}
