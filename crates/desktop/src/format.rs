//! Pure presentation helpers: local time, initials, avatar colors, sizes.

use crate::reactors::UNKNOWN_NAME;
use std::time::{SystemTime, UNIX_EPOCH};

const US: i64 = 1_000_000;
const WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];
const AVATAR_COLORS: [u32; 7] = [
    0xc4585f, 0xd08a4f, 0x8a79c9, 0x5fa84a, 0x4aa8ab, 0x4a8dc0, 0xc9608c,
];

/// Source of "now" and the local UTC offset; fixed in tests.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    fixed: Option<(i64, i32)>,
}

impl Clock {
    pub fn system() -> Self {
        Self { fixed: None }
    }

    #[cfg(test)]
    pub fn fixed(now_us: i64, utc_offset_secs: i32) -> Self {
        Self {
            fixed: Some((now_us, utc_offset_secs)),
        }
    }

    pub fn now_us(self) -> i64 {
        match self.fixed {
            Some((now, _)) => now,
            None => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_micros() as i64),
        }
    }

    fn utc_offset(self, ts_us: i64) -> i32 {
        match self.fixed {
            Some((_, offset)) => offset,
            None => system_utc_offset(ts_us.div_euclid(US)),
        }
    }
}

/// Mirrors `struct tm` of macOS and glibc; only `tm_gmtoff` is read.
#[repr(C)]
struct Tm {
    ints: [i32; 9],
    tm_gmtoff: std::ffi::c_long,
    tm_zone: *const std::ffi::c_char,
}

unsafe extern "C" {
    fn localtime_r(time: *const std::ffi::c_long, out: *mut Tm) -> *mut Tm;
}

fn system_utc_offset(secs: i64) -> i32 {
    let time = secs as std::ffi::c_long;
    // SAFETY: `Tm` is plain data that `localtime_r` fully initializes on success; a null result leaves it unread.
    unsafe {
        let mut tm: Tm = std::mem::zeroed();
        if localtime_r(&time, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i32
    }
}

struct Civil {
    day: i64,
    secs: u32,
    year: i64,
    month: u32,
    dom: u32,
    weekday: usize,
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dom = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, dom)
}

fn civil(ts_us: i64, offset: i32) -> Civil {
    let local = ts_us.div_euclid(US) + i64::from(offset);
    let day = local.div_euclid(86_400);
    let (year, month, dom) = civil_from_days(day);
    Civil {
        day,
        secs: local.rem_euclid(86_400) as u32,
        year,
        month,
        dom,
        // 1970-01-01 was a Thursday; Monday is 0.
        weekday: (day + 3).rem_euclid(7) as usize,
    }
}

fn local(ts_us: i64, clock: Clock) -> Civil {
    civil(ts_us, clock.utc_offset(ts_us))
}

/// Local calendar day number; equal for timestamps on the same local day.
pub fn day_key(ts_us: i64, clock: Clock) -> i64 {
    local(ts_us, clock).day
}

/// `HH:MM` in local time.
pub fn clock_time(ts_us: i64, clock: Clock) -> String {
    let c = local(ts_us, clock);
    format!("{:02}:{:02}", c.secs / 3600, c.secs / 60 % 60)
}

/// `HH:MM:SS.mmm` in local time, for log lines.
pub fn log_time(ts_us: i64, clock: Clock) -> String {
    let c = local(ts_us, clock);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        c.secs / 3600,
        c.secs / 60 % 60,
        c.secs % 60,
        ts_us.rem_euclid(US) / 1000
    )
}

/// `YYYYMMDD-HHMMSS` in UTC; sorts in time order, for file names.
pub fn utc_stamp(ts_us: i64) -> String {
    let c = civil(ts_us, 0);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        c.year,
        c.month,
        c.dom,
        c.secs / 3600,
        c.secs / 60 % 60,
        c.secs % 60
    )
}

/// Chat row time: today `HH:MM`, within the last week the weekday, otherwise `DD.MM.YY`.
pub fn chat_time(ts_us: i64, clock: Clock) -> String {
    let c = local(ts_us, clock);
    let age = local(clock.now_us(), clock).day - c.day;
    match age {
        0 => format!("{:02}:{:02}", c.secs / 3600, c.secs / 60 % 60),
        1..=6 => WEEKDAYS[c.weekday].to_string(),
        _ => format!("{:02}.{:02}.{:02}", c.dom, c.month, c.year.rem_euclid(100)),
    }
}

/// Date separator label inside a timeline.
pub fn day_label(ts_us: i64, clock: Clock) -> String {
    let c = local(ts_us, clock);
    let now = local(clock.now_us(), clock);
    match now.day - c.day {
        0 => "Сегодня".into(),
        1 => "Вчера".into(),
        _ if c.year == now.year => format!("{} {}", c.dom, MONTHS[c.month as usize - 1]),
        _ => format!("{} {} {}", c.dom, MONTHS[c.month as usize - 1], c.year),
    }
}

/// `ДД.ММ.ГГГГ ЧЧ:ММ` in local time, for the header of copied messages.
pub fn date_time(ts_us: i64, clock: Clock) -> String {
    let c = local(ts_us, clock);
    format!(
        "{:02}.{:02}.{:04} {:02}:{:02}",
        c.dom,
        c.month,
        c.year,
        c.secs / 3600,
        c.secs / 60 % 60
    )
}

/// Header line of a copied message: `{author}, {date_time}`; a missing or blank author reads «Неизвестный».
pub fn copy_header(author: Option<&str>, ts_us: i64, clock: Clock) -> String {
    let author = author
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(UNKNOWN_NAME);
    format!("{author}, {}", date_time(ts_us, clock))
}

/// First letters of the first and last word, uppercased.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let last = words.next_back().and_then(|w| w.chars().next());
    match (first, last) {
        (Some(a), Some(b)) => a.to_uppercase().chain(b.to_uppercase()).collect(),
        (Some(a), None) => a.to_uppercase().collect(),
        _ => "?".into(),
    }
}

/// Stable avatar color for a server id (FNV-1a).
pub fn avatar_color(key: &str) -> u32 {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    AVATAR_COLORS[(hash % AVATAR_COLORS.len() as u64) as usize]
}

/// Human file size with a decimal comma, e.g. `1,5 МБ`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} Б")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Playback duration: `m:ss` below one hour, `h:mm:ss` from one hour; whole seconds, rounded down.
pub fn duration_label(ms: u64) -> String {
    let secs = ms / 1000;
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The line under a video's name, e.g. `0:42 · 12 МБ`: the duration and the size, a missing part left
/// out, an empty string when both are missing.
pub fn duration_size_line(duration_ms: Option<u64>, size: Option<u64>) -> String {
    let parts = [duration_ms.map(duration_label), size.map(human_size)];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// Scales `width`x`height` down to fit inside `max`x`max` keeping the aspect ratio; never scales up.
/// Unknown dimensions give a square.
pub fn fit_thumbnail(width: u32, height: u32, max: f32) -> (f32, f32) {
    if width == 0 || height == 0 {
        return (max * 0.6, max * 0.6);
    }
    let (w, h) = (width as f32, height as f32);
    let scale = (max / w).min(max / h).min(1.0);
    (w * scale, h * scale)
}

/// Single-line preview: whitespace runs, including line breaks, collapse to one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A count of messages with the Russian plural of the noun: 1 сообщение, 2-4 сообщения, 5-20 сообщений,
/// 21 сообщение, 22-24 сообщения, 111-114 сообщений.
pub fn messages_count(n: usize) -> String {
    let noun = match (n % 10, n % 100) {
        (_, 11..=14) => "сообщений",
        (1, _) => "сообщение",
        (2..=4, _) => "сообщения",
        _ => "сообщений",
    };
    format!("{n} {noun}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn ts(y: i64, m: i64, d: i64, h: i64, mi: i64) -> i64 {
        (days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60) * US
    }

    #[test]
    fn civil_round_trip_and_weekday() {
        for day in [-1, 0, 59, 365, 10_957, 20_411, 25_000] {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m as i64, d as i64), day);
        }
        // 2026-11-19 is a Thursday.
        assert_eq!(civil(ts(2026, 11, 19, 12, 0), 0).weekday, 3);
        assert_eq!(civil(0, 0).weekday, 3);
    }

    #[test]
    fn chat_time_buckets_use_local_offset() {
        let clock = Clock::fixed(ts(2026, 11, 19, 20, 0), 3 * 3600);
        // 21:30 UTC is already the next local day, 00:30; clock "now" is 23:00 local on the 19th.
        assert_eq!(chat_time(ts(2026, 11, 19, 9, 5), clock), "12:05");
        assert_eq!(chat_time(ts(2026, 11, 18, 9, 5), clock), "Ср");
        assert_eq!(chat_time(ts(2026, 11, 13, 9, 5), clock), "Пт");
        assert_eq!(chat_time(ts(2026, 11, 12, 9, 5), clock), "12.11.26");
        assert_eq!(chat_time(ts(2025, 1, 2, 9, 5), clock), "02.01.25");
        assert_eq!(clock_time(ts(2026, 11, 19, 21, 30), clock), "00:30");
        assert_ne!(
            day_key(ts(2026, 11, 19, 20, 59), clock),
            day_key(ts(2026, 11, 19, 21, 0), clock)
        );
    }

    #[test]
    fn day_labels() {
        let clock = Clock::fixed(ts(2026, 11, 19, 12, 0), 0);
        assert_eq!(day_label(ts(2026, 11, 19, 1, 0), clock), "Сегодня");
        assert_eq!(day_label(ts(2026, 11, 18, 23, 0), clock), "Вчера");
        assert_eq!(day_label(ts(2026, 3, 5, 0, 0), clock), "5 марта");
        assert_eq!(day_label(ts(2025, 12, 31, 0, 0), clock), "31 декабря 2025");
    }

    #[test]
    fn date_time_uses_the_clock_offset() {
        let clock = Clock::fixed(0, 3 * 3600);
        assert_eq!(date_time(ts(2026, 11, 19, 9, 5), clock), "19.11.2026 12:05");
        assert_eq!(
            date_time(ts(2026, 3, 5, 0, 0), Clock::fixed(0, 0)),
            "05.03.2026 00:00"
        );
    }

    #[test]
    fn date_time_at_midnight_and_year_boundaries() {
        let utc = Clock::fixed(0, 0);
        assert_eq!(date_time(ts(2026, 12, 31, 23, 59), utc), "31.12.2026 23:59");
        assert_eq!(date_time(ts(2027, 1, 1, 0, 0), utc), "01.01.2027 00:00");
        // 21:30 UTC on 31 December is 00:30 on 1 January at +3.
        assert_eq!(
            date_time(ts(2026, 12, 31, 21, 30), Clock::fixed(0, 3 * 3600)),
            "01.01.2027 00:30"
        );
        // 00:30 UTC on 1 January is still 31 December at -1.
        assert_eq!(
            date_time(ts(2027, 1, 1, 0, 30), Clock::fixed(0, -3600)),
            "31.12.2026 23:30"
        );
        assert_eq!(date_time(ts(2024, 2, 29, 12, 0), utc), "29.02.2024 12:00");
        assert_eq!(date_time(0, utc), "01.01.1970 00:00");
    }

    #[test]
    fn copy_header_names_the_author_or_an_unknown_one() {
        let clock = Clock::fixed(0, 3 * 3600);
        let at = ts(2026, 11, 19, 9, 5);
        assert_eq!(
            copy_header(Some("Анна Петрова"), at, clock),
            "Анна Петрова, 19.11.2026 12:05"
        );
        assert_eq!(
            copy_header(None, at, clock),
            "Неизвестный, 19.11.2026 12:05"
        );
        assert_eq!(
            copy_header(Some("  "), at, clock),
            "Неизвестный, 19.11.2026 12:05"
        );
    }

    #[test]
    fn initials_cases() {
        assert_eq!(initials("Анна Петрова"), "АП");
        assert_eq!(initials("  анна  мария  петрова "), "АП");
        assert_eq!(initials("Telemost"), "T");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn avatar_color_is_stable() {
        assert_eq!(avatar_color("0/0/abc"), avatar_color("0/0/abc"));
        let colors: std::collections::HashSet<_> = (0..64)
            .map(|i| avatar_color(&format!("chat-{i}")))
            .collect();
        assert!(colors.len() > 3);
    }

    #[test]
    fn sizes_and_thumbnails() {
        assert_eq!(human_size(0), "0 Б");
        assert_eq!(human_size(1023), "1023 Б");
        assert_eq!(human_size(1536), "1,5 КБ");
        assert_eq!(human_size(1 << 20), "1,0 МБ");
        assert_eq!(human_size(25 * (1 << 20)), "25 МБ");
        assert_eq!(fit_thumbnail(1600, 800, 320.0), (320.0, 160.0));
        assert_eq!(fit_thumbnail(800, 1600, 320.0), (160.0, 320.0));
        assert_eq!(fit_thumbnail(100, 50, 320.0), (100.0, 50.0));
        assert_eq!(fit_thumbnail(0, 50, 320.0), (192.0, 192.0));
    }

    #[test]
    fn duration_labels() {
        assert_eq!(duration_label(0), "0:00");
        assert_eq!(duration_label(5_000), "0:05");
        assert_eq!(duration_label(59_999), "0:59");
        assert_eq!(duration_label(60_000), "1:00");
        assert_eq!(duration_label(67_000), "1:07");
        assert_eq!(duration_label(723_400), "12:03");
        assert_eq!(duration_label(999), "0:00");
        assert_eq!(duration_label(3_599_999), "59:59");
        assert_eq!(duration_label(3_600_000), "1:00:00");
        assert_eq!(duration_label(3_723_000), "1:02:03");
        assert_eq!(duration_label(36_000_000), "10:00:00");
        assert_eq!(duration_label(u64::MAX), "5124095576030:25:51");
    }

    #[test]
    fn duration_size_lines() {
        assert_eq!(
            duration_size_line(Some(42_000), Some(12 * (1 << 20))),
            "0:42 · 12 МБ"
        );
        assert_eq!(
            duration_size_line(Some(67_000), Some(1536)),
            "1:07 · 1,5 КБ"
        );
        assert_eq!(duration_size_line(Some(5_000), Some(900)), "0:05 · 900 Б");
        assert_eq!(duration_size_line(Some(0), Some(0)), "0:00 · 0 Б");
        assert_eq!(duration_size_line(Some(42_000), None), "0:42");
        assert_eq!(duration_size_line(None, Some(12 * (1 << 20))), "12 МБ");
        assert_eq!(duration_size_line(None, None), "");
    }

    #[test]
    fn preview_collapses_breaks() {
        assert_eq!(one_line("a\n\n b   c\n"), "a b c");
    }

    #[test]
    fn system_offset_is_sane() {
        assert!(system_utc_offset(1_700_000_000).abs() <= 14 * 3600);
    }

    #[test]
    fn messages_count_has_the_russian_plural() {
        let label = messages_count;
        assert_eq!(label(1), "1 сообщение");
        for n in [2, 3, 4, 22, 24, 102] {
            assert_eq!(label(n), format!("{n} сообщения"));
        }
        for n in [0, 5, 10, 11, 14, 20, 25, 100, 111, 112, 113, 114, 1000] {
            assert_eq!(label(n), format!("{n} сообщений"));
        }
        for n in [21, 31, 101, 121] {
            assert_eq!(label(n), format!("{n} сообщение"));
        }
    }
}
