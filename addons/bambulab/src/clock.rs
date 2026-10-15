//! Wall-clock times in the house's time zone.
//!
//! Niles tells the plugin the house's zone by its IANA name
//! (`config::time_zone`). The sandbox has no zone files, so the rules
//! come from the tz database jiff bundles into the component.

use jiff::Timestamp;
use jiff::tz::TimeZone;

/// The zone Niles names, or UTC if the name is not one the bundled
/// database knows.
pub fn zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

/// "14:35", "14:35 tomorrow" or "14:35 on 17 October", for a moment
/// `at` seen from `now`, both in Unix seconds.
pub fn clock_from(zone: &TimeZone, now: i64, at: i64) -> String {
    let local = |unix: i64| {
        Timestamp::from_second(unix)
            .unwrap_or(Timestamp::UNIX_EPOCH)
            .to_zoned(zone.clone())
    };
    let (now, at) = (local(now), local(at));
    let clock = format!("{:02}:{:02}", at.hour(), at.minute());
    if at.date() == now.date() {
        clock
    } else if now.date().tomorrow().ok() == Some(at.date()) {
        format!("{clock} tomorrow")
    } else {
        format!(
            "{clock} on {} {}",
            at.day(),
            MONTHS[at.month() as usize - 1]
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-15 12:00 UTC.
    const NOON: i64 = 1_792_065_600;

    #[test]
    fn copenhagen_follows_summer_time() {
        let cph = zone("Europe/Copenhagen");
        assert_eq!(clock_from(&cph, NOON, NOON), "14:00");
        // 2026-12-01 12:00 UTC: winter, one hour ahead.
        let december = 1_796_126_400;
        assert_eq!(clock_from(&cph, december, december), "13:00");
    }

    #[test]
    fn eu_summer_time_ends_at_one_in_the_morning_utc() {
        let cph = zone("Europe/Copenhagen");
        // 2026-10-25 00:59 and 01:00 UTC.
        let before = 1_792_889_940;
        assert_eq!(clock_from(&cph, before, before), "02:59");
        assert_eq!(clock_from(&cph, before, before + 60), "02:00");
    }

    #[test]
    fn half_hour_zones_and_unknown_names() {
        assert_eq!(clock_from(&zone("Asia/Kolkata"), NOON, NOON), "17:30");
        assert_eq!(clock_from(&zone("Not/AZone"), NOON, NOON), "12:00");
    }

    #[test]
    fn clock_says_tomorrow_past_midnight() {
        let cph = zone("Europe/Copenhagen");
        let evening = NOON + 8 * 3_600; // 22:00 in Copenhagen
        assert_eq!(clock_from(&cph, evening, evening + 30 * 60), "22:30");
        assert_eq!(
            clock_from(&cph, evening, evening + 5 * 3_600),
            "03:00 tomorrow"
        );
        assert_eq!(
            clock_from(&cph, evening, evening + 50 * 3_600),
            "00:00 on 18 October"
        );
    }
}
