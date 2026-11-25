//! Localized human-readable dates, times and sizes for the desktop.

use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::{Locale, TextKey, gui_keys as gui, preferences::TimeFormat};

const MONTHS: [TextKey; 12] = [
    gui::GUI_DATE_JANUARY,
    gui::GUI_DATE_FEBRUARY,
    gui::GUI_DATE_MARCH,
    gui::GUI_DATE_APRIL,
    gui::GUI_DATE_MAY,
    gui::GUI_DATE_JUNE,
    gui::GUI_DATE_JULY,
    gui::GUI_DATE_AUGUST,
    gui::GUI_DATE_SEPTEMBER,
    gui::GUI_DATE_OCTOBER,
    gui::GUI_DATE_NOVEMBER,
    gui::GUI_DATE_DECEMBER,
];

/// Format a Unix timestamp as a Gregorian date and clock time in UTC.
pub fn date_time_utc(locale: Locale, format: TimeFormat, unix_seconds: i64) -> Option<String> {
    let instant = OffsetDateTime::from_unix_timestamp(unix_seconds).ok()?;
    Some(format_date_time(locale, format, instant))
}

/// Format a Docker RFC 3339 date as a human-readable UTC date and clock time.
pub fn rfc3339_utc(locale: Locale, format: TimeFormat, value: &str) -> Option<String> {
    let instant = parse_rfc3339_utc(value)?;
    Some(format_date_time(locale, format, instant))
}

/// Format an RFC 3339 timestamp as a UTC clock label at minute precision.
pub fn rfc3339_clock_utc(locale: Locale, format: TimeFormat, value: &str) -> Option<String> {
    let instant = parse_rfc3339_utc(value)?;
    Some(clock(
        locale,
        format,
        instant.hour(),
        instant.minute(),
        None,
    ))
}

fn parse_rfc3339_utc(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .checked_to_offset(UtcOffset::UTC)
}

fn format_date_time(locale: Locale, format: TimeFormat, instant: OffsetDateTime) -> String {
    let (year, month, day) = instant.to_calendar_date();
    let month = MONTHS[usize::from(u8::from(month) - 1)].text(locale);
    let clock = clock(
        locale,
        format,
        instant.hour(),
        instant.minute(),
        Some(instant.second()),
    );
    match locale {
        Locale::EnglishUs => format!("{month} {day}, {year}, {clock} UTC"),
        Locale::Arabic => format!("{day} {month} {year}، {clock} UTC"),
    }
}

/// Format the time of day of a UTC Unix timestamp in milliseconds.
pub fn time_utc(locale: Locale, format: TimeFormat, unix_millis: i64) -> String {
    let seconds = unix_millis.div_euclid(1_000).rem_euclid(86_400);
    let hour = (seconds / 3_600) as u8;
    let minute = (seconds / 60 % 60) as u8;
    let second = (seconds % 60) as u8;
    format!("{} UTC", clock(locale, format, hour, minute, Some(second)))
}

/// Return the locale's labels for the two halves of a 12-hour clock.
pub fn day_periods(locale: Locale) -> (&'static str, &'static str) {
    (
        gui::GUI_CLOCK_AM.text(locale),
        gui::GUI_CLOCK_PM.text(locale),
    )
}

fn clock(locale: Locale, format: TimeFormat, hour: u8, minute: u8, second: Option<u8>) -> String {
    use std::fmt::Write as _;
    let (hour, period) = match format {
        TimeFormat::TwentyFourHour => (hour, None),
        TimeFormat::TwelveHour => {
            let (am, pm) = day_periods(locale);
            ((hour + 11) % 12 + 1, Some(if hour < 12 { am } else { pm }))
        }
    };
    let mut text = format!("{hour:02}:{minute:02}");
    if let Some(second) = second {
        let _ = write!(text, ":{second:02}");
    }
    if let Some(period) = period {
        let _ = write!(text, " {period}");
    }
    text
}

/// Select the unit scale and precision appropriate to the displayed measurement.
#[derive(Clone, Copy)]
pub enum SizeFormat {
    /// SI units with one decimal place for scaled values.
    Decimal,
    /// SI units with extra precision for image and layer metadata.
    DecimalDetail,
    /// IEC units, using the standard IEC symbols for scaled values.
    Binary,
}

/// Format bytes in the requested units with Western digits.
pub fn bytes(locale: Locale, format: SizeFormat, bytes: u64) -> String {
    let (radix, scale) = match format {
        SizeFormat::Decimal | SizeFormat::DecimalDetail => (1_000_u64, 3),
        SizeFormat::Binary => (1_024_u64, 4),
    };
    let exponent = (1..=scale)
        .take_while(|&power| bytes >= radix.pow(power))
        .last()
        .unwrap_or(0);
    if exponent == 0 {
        return format!("{bytes} {}", gui::GUI_DESKTOP_B.text(locale));
    }

    let unit = radix.pow(exponent);
    let value = bytes as f64 / unit as f64;
    let index = (exponent - 1) as usize;
    match format {
        SizeFormat::Decimal | SizeFormat::DecimalDetail => {
            let units = [
                gui::GUI_DESKTOP_KB,
                gui::GUI_DESKTOP_MB,
                gui::GUI_DESKTOP_GB,
            ];
            let precision = match format {
                SizeFormat::Decimal => 1,
                _ if index == 0 => 0,
                _ => 2,
            };
            format!("{value:.precision$} {}", units[index].text(locale))
        }
        SizeFormat::Binary => {
            let precision = usize::from(!bytes.is_multiple_of(unit));
            format!("{value:.precision$} {}", binary_unit(locale, index + 1))
        }
    }
}

/// Format a finite, nonnegative byte rate in IEC units per second.
pub fn bytes_per_second(locale: Locale, rate: f64) -> Option<String> {
    if !rate.is_finite() || rate < 0. {
        return None;
    }
    let units = [1., 1_024., 1_048_576., 1_073_741_824., 1_099_511_627_776.];
    let exponent = units.iter().rposition(|&unit| rate >= unit).unwrap_or(0);
    let value = rate / units[exponent];
    let suffix = gui::GUI_SIZE_PER_SECOND.text(locale);
    Some(format!(
        "{value:.1} {}{suffix}",
        binary_unit(locale, exponent)
    ))
}

fn binary_unit(locale: Locale, exponent: usize) -> &'static str {
    [gui::GUI_DESKTOP_B.text(locale), "KiB", "MiB", "GiB", "TiB"][exponent]
}

#[cfg(test)]
mod tests {
    use super::*;
    use Locale::{Arabic, EnglishUs};
    use TimeFormat::{TwelveHour, TwentyFourHour};

    #[test]
    fn date_order_and_clock_labels_follow_the_selected_language() {
        let source = "2024-02-29T17:30:45+02:00";
        assert_eq!(
            rfc3339_utc(EnglishUs, TwelveHour, source).as_deref(),
            Some("February 29, 2024, 03:30:45 PM UTC"),
        );
        assert_eq!(
            rfc3339_utc(Arabic, TwelveHour, source).as_deref(),
            Some("29 فبراير 2024، 03:30:45 م UTC"),
        );
        assert_eq!(
            date_time_utc(Arabic, TwentyFourHour, 0).as_deref(),
            Some("1 يناير 1970، 00:00:00 UTC"),
        );
        assert_eq!(time_utc(Arabic, TwelveHour, 0), "12:00:00 ص UTC");
        assert_eq!(time_utc(EnglishUs, TwentyFourHour, -1), "23:59:59 UTC");
        assert!(date_time_utc(EnglishUs, TwentyFourHour, i64::MAX).is_none());
        assert!(rfc3339_utc(Arabic, TwelveHour, "invalid").is_none());
    }

    #[test]
    fn settings_and_stats_clocks_use_western_digits() {
        assert_eq!(gui::GUI_SETTINGS_TIME_FORMAT_TWELVE.text(Arabic), "12 ساعة");
        assert_eq!(
            gui::GUI_SETTINGS_TIME_FORMAT_TWENTY_FOUR.text(Arabic),
            "24 ساعة"
        );
        let sample = "2024-02-29T17:30:45+02:00";
        assert_eq!(
            rfc3339_clock_utc(Arabic, TwelveHour, sample).as_deref(),
            Some("03:30 م")
        );
        assert_eq!(
            rfc3339_clock_utc(EnglishUs, TwentyFourHour, sample).as_deref(),
            Some("15:30")
        );
        assert!(rfc3339_clock_utc(Arabic, TwelveHour, "invalid").is_none());
    }

    #[test]
    fn byte_rates_localize_units_and_keep_fractional_precision() {
        assert_eq!(
            bytes_per_second(Arabic, 4.25).as_deref(),
            Some("4.2 بايت/ث")
        );
        assert_eq!(
            bytes_per_second(EnglishUs, 1_536.).as_deref(),
            Some("1.5 KiB/s")
        );
        assert_eq!(
            bytes_per_second(Arabic, 1_536.).as_deref(),
            Some("1.5 KiB/ث")
        );
        assert!(bytes_per_second(Arabic, f64::NAN).is_none());
    }

    #[test]
    fn sizes_use_localized_labels_western_digits_and_correct_unit_boundaries() {
        use SizeFormat::{Binary, Decimal, DecimalDetail};
        assert_eq!(bytes(EnglishUs, Decimal, 999), "999 B");
        assert_eq!(bytes(Arabic, Decimal, 999), "999 بايت");
        assert_eq!(bytes(Arabic, Decimal, 1_250_000), "1.2 م.ب");
        assert_eq!(bytes(EnglishUs, DecimalDetail, 1_250_000), "1.25 MB");
        assert_eq!(bytes(Arabic, Binary, 1_536), "1.5 KiB");
        assert_eq!(bytes(EnglishUs, Binary, 1_024), "1 KiB");
    }
}
