//! Wall-clock times in the house's time zone.
//!
//! The sandbox has a clock but no time zones, and Niles does not tell a
//! plugin which zone the house is in, so the household picks one in the
//! `time_zone` setting and the rules for it live here: a standard
//! offset and, where there is one, the summer-time rule.

/// When summer time applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Summer {
    None,
    /// The EU's: last Sunday of March to last Sunday of October, 01:00 UTC.
    Eu,
    /// The US's: second Sunday of March to first Sunday of November, 02:00 local.
    Us,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Minutes east of UTC in winter.
    standard: i64,
    summer: Summer,
}

impl Zone {
    pub const UTC: Zone = Zone {
        standard: 0,
        summer: Summer::None,
    };

    /// The zone for a `time_zone` setting's value. Unknown values are UTC.
    pub fn from_setting(value: &str) -> Zone {
        let (standard, summer) = match value {
            "europe_west" => (0, Summer::Eu),
            "europe_central" => (60, Summer::Eu),
            "europe_east" => (120, Summer::Eu),
            "us_eastern" => (-300, Summer::Us),
            "us_central" => (-360, Summer::Us),
            "us_mountain" => (-420, Summer::Us),
            "us_arizona" => (-420, Summer::None),
            "us_pacific" => (-480, Summer::Us),
            "india" => (330, Summer::None),
            "china" => (480, Summer::None),
            "japan" => (540, Summer::None),
            _ => (0, Summer::None),
        };
        Zone { standard, summer }
    }

    /// Minutes east of UTC at `unix` seconds.
    fn offset_at(&self, unix: i64) -> i64 {
        let (year, _, _) = civil_from_days(unix.div_euclid(86_400));
        let summer = match self.summer {
            Summer::None => false,
            Summer::Eu => {
                let start = last_sunday(year, 3) * 86_400 + 3_600;
                let end = last_sunday(year, 10) * 86_400 + 3_600;
                (start..end).contains(&unix)
            }
            Summer::Us => {
                let standard = self.standard * 60;
                let start = nth_sunday(year, 3, 2) * 86_400 + 7_200 - standard;
                let end = nth_sunday(year, 11, 1) * 86_400 + 7_200 - (standard + 3_600);
                (start..end).contains(&unix)
            }
        };
        self.standard + if summer { 60 } else { 0 }
    }

    /// The local date and time at `unix` seconds.
    pub fn local(&self, unix: i64) -> Local {
        let local = unix + self.offset_at(unix) * 60;
        let day = local.div_euclid(86_400);
        let seconds = local.rem_euclid(86_400);
        Local {
            day,
            hour: (seconds / 3_600) as u32,
            minute: (seconds % 3_600 / 60) as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    /// Days since 1970-01-01, local.
    pub day: i64,
    pub hour: u32,
    pub minute: u32,
}

impl Local {
    pub fn clock(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

/// "14:35", "14:35 tomorrow" or "14:35 on 17 October", for a moment
/// `at` seen from `now`.
pub fn clock_from(zone: Zone, now: i64, at: i64) -> String {
    let (now, at) = (zone.local(now), zone.local(at));
    match at.day - now.day {
        0 => at.clock(),
        1 => format!("{} tomorrow", at.clock()),
        _ => {
            let (_, month, day) = civil_from_days(at.day);
            format!("{} on {day} {}", at.clock(), MONTHS[month as usize - 1])
        }
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Howard Hinnant's days_from_civil.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Howard Hinnant's civil_from_days: (year, month 1-12, day 1-31).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 0 is Sunday.
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

fn last_sunday(year: i64, month: i64) -> i64 {
    let last = days_from_civil(year, month + 1, 1) - 1;
    last - weekday(last)
}

fn nth_sunday(year: i64, month: i64, n: i64) -> i64 {
    let first = days_from_civil(year, month, 1);
    first + (7 - weekday(first)) % 7 + 7 * (n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> i64 {
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60
    }

    #[test]
    fn civil_round_trips() {
        for days in [-1, 0, 59, 10_957, 20_741, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn copenhagen_is_two_hours_ahead_in_summer_and_one_in_winter() {
        let cph = Zone::from_setting("europe_central");
        assert_eq!(cph.local(unix(2026, 7, 1, 12, 0)).clock(), "14:00");
        assert_eq!(cph.local(unix(2026, 12, 1, 12, 0)).clock(), "13:00");
    }

    #[test]
    fn eu_summer_time_changes_at_one_in_the_morning_utc() {
        let cph = Zone::from_setting("europe_central");
        // 2026: last Sunday of March is the 29th, of October the 25th.
        assert_eq!(cph.local(unix(2026, 3, 29, 0, 59)).clock(), "01:59");
        assert_eq!(cph.local(unix(2026, 3, 29, 1, 0)).clock(), "03:00");
        assert_eq!(cph.local(unix(2026, 10, 25, 0, 59)).clock(), "02:59");
        assert_eq!(cph.local(unix(2026, 10, 25, 1, 0)).clock(), "02:00");
    }

    #[test]
    fn us_summer_time_changes_at_two_in_the_morning_local() {
        let ny = Zone::from_setting("us_eastern");
        // 2026: second Sunday of March is the 8th, first of November the 1st.
        assert_eq!(ny.local(unix(2026, 3, 8, 6, 59)).clock(), "01:59");
        assert_eq!(ny.local(unix(2026, 3, 8, 7, 0)).clock(), "03:00");
        assert_eq!(ny.local(unix(2026, 11, 1, 5, 59)).clock(), "01:59");
        assert_eq!(ny.local(unix(2026, 11, 1, 6, 0)).clock(), "01:00");
    }

    #[test]
    fn zones_without_summer_time_stay_put() {
        let india = Zone::from_setting("india");
        assert_eq!(india.local(unix(2026, 7, 1, 12, 0)).clock(), "17:30");
        assert_eq!(Zone::from_setting("nonsense"), Zone::UTC);
    }

    #[test]
    fn clock_says_tomorrow_past_midnight() {
        let cph = Zone::from_setting("europe_central");
        let now = unix(2026, 10, 15, 20, 0);
        assert_eq!(clock_from(cph, now, now + 30 * 60), "22:30");
        assert_eq!(clock_from(cph, now, now + 5 * 3_600), "03:00 tomorrow");
        assert_eq!(
            clock_from(cph, now, now + 50 * 3_600),
            "00:00 on 18 October"
        );
    }
}
