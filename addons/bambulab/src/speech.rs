//! What the plugin says: the status the voice model reads out, and the
//! announcements. Written to be spoken, so units are words and numbers
//! are rounded.

use crate::clock::clock_from;
use crate::model::{Action, CANCELLED, JobState, Snapshot, VERIFICATION_FAILED, error_text};
use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};

/// "1 hour 5 minutes", "45 minutes", "2 hours", "less than a minute".
pub fn duration(minutes: f64) -> String {
    let minutes = minutes.max(0.0).round() as u64;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    let unit = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match (hours, minutes) {
        (0, 0) => "less than a minute".into(),
        (0, m) => unit(m, "minute", "minutes"),
        (h, 0) => unit(h, "hour", "hours"),
        (h, m) => format!(
            "{} {}",
            unit(h, "hour", "hours"),
            unit(m, "minute", "minutes")
        ),
    }
}

/// "1 h 5 min", for the page.
pub fn short_duration(minutes: f64) -> String {
    let minutes = minutes.max(0.0).round() as u64;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// When the print should be done, in Unix seconds, if one is under way.
pub fn finishes_at(s: &Snapshot, now: i64) -> Option<i64> {
    let state = s.state.as_ref()?;
    if !state.is_active() {
        return None;
    }
    let remaining = s.remaining_min.filter(|m| *m > 0.0)?;
    Some(now + (remaining * 60.0).round() as i64)
}

fn degrees(value: f64) -> String {
    format!("{}", value.round() as i64)
}

fn job(s: &Snapshot) -> String {
    s.job.clone().unwrap_or_else(|| "the print".into())
}

pub fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn temperatures(s: &Snapshot) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = s.nozzle {
        parts.push(format!("nozzle {} degrees", degrees(n)));
    }
    if let Some(b) = s.bed {
        parts.push(format!("bed {}", degrees(b)));
    }
    if let Some(c) = s.chamber {
        parts.push(format!("chamber {}", degrees(c)));
    }
    (!parts.is_empty()).then(|| capitalised(&parts.join(", ")) + ".")
}

fn layers(s: &Snapshot) -> Option<String> {
    match (s.layer, s.total_layers) {
        (Some(l), Some(t)) if t > 0.0 => Some(format!("layer {} of {}", l as u64, t as u64)),
        _ => None,
    }
}

fn percent(s: &Snapshot) -> String {
    format!("{} percent", s.percent.unwrap_or(0.0).round() as i64)
}

/// The problems the printer reports, as a sentence, if any.
pub fn problems(s: &Snapshot) -> Option<String> {
    let mut codes = Vec::new();
    if let Some(e) = s.print_error.filter(|e| *e != 0 && *e != CANCELLED) {
        codes.push(format!("error {}", error_text(e)));
    }
    for h in &s.hms {
        if *h == VERIFICATION_FAILED {
            codes.push(format!(
                "that it refused a command it could not verify (HMS {})",
                h.code_text()
            ));
        } else {
            codes.push(format!("HMS {} ({})", h.code_text(), h.severity()));
        }
    }
    match codes.len() {
        0 => None,
        1 => Some(format!("The printer reports {}.", codes[0])),
        _ => Some(format!("The printer reports {}.", codes.join(", "))),
    }
}

/// One or two sentences on where the printer stands.
pub fn summary(s: &Snapshot, now: i64, zone: &TimeZone) -> String {
    let Some(state) = &s.state else {
        return "The printer is connected but has not said what it is doing yet.".into();
    };
    let mut out = match state {
        JobState::Running => {
            let mut parts = vec![format!("Printing {}: {} done", job(s), percent(s))];
            parts.extend(layers(s));
            if let Some(remaining) = s.remaining_min.filter(|m| *m > 0.0) {
                parts.push(format!("about {} left", duration(remaining)));
            }
            if let Some(at) = finishes_at(s, now) {
                parts.push(format!("done around {}", clock_from(zone, now, at)));
            }
            parts.join(", ") + "."
        }
        JobState::Prepare | JobState::Slicing => {
            let mut text = format!("Getting ready to print {}", job(s));
            if let (Some(n), Some(t)) = (s.nozzle, s.nozzle_target.filter(|t| *t > 0.0)) {
                text += &format!(
                    ": heating the nozzle to {} degrees, now at {}",
                    degrees(t),
                    degrees(n)
                );
            }
            text + "."
        }
        JobState::Pause => {
            let mut text = format!("{} is paused at {}", capitalised(&job(s)), percent(s));
            if let Some(l) = layers(s) {
                text += &format!(", {l}");
            }
            text + "."
        }
        JobState::Finish => match &s.job {
            Some(job) => format!("The printer is idle. The last print, {job}, finished."),
            None => "The printer is idle. The last print finished.".into(),
        },
        JobState::Failed if s.print_error == Some(CANCELLED) => format!(
            "The printer is idle. {} was stopped at {}.",
            capitalised(&job(s)),
            percent(s)
        ),
        JobState::Failed => format!("{} failed at {}.", capitalised(&job(s)), percent(s)),
        JobState::Idle => "The printer is idle.".into(),
        JobState::Other(_) => format!("The printer says {}.", state.label().to_lowercase()),
    };
    for extra in [temperatures(s), problems(s)].into_iter().flatten() {
        out.push(' ');
        out += &extra;
    }
    out
}

/// What `printer_status` returns: the summary, and the facts behind it
/// for the model to pick from.
pub fn status(s: &Snapshot, now: i64, zone: &TimeZone) -> Value {
    let mut facts = Map::new();
    let mut put = |key: &str, value: Value| {
        facts.insert(key.into(), value);
    };
    put("summary", summary(s, now, zone).into());
    if let Some(state) = &s.state {
        put("state", state.label().to_lowercase().into());
    }
    if let Some(job) = &s.job {
        put("job", job.as_str().into());
    }
    let active = s.state.as_ref().is_some_and(JobState::is_active);
    if active || s.state == Some(JobState::Failed) {
        if let Some(p) = s.percent {
            put("progress_percent", json!(p.round() as i64));
        }
        if let (Some(l), Some(t)) = (s.layer, s.total_layers) {
            put("layer", json!(l as u64));
            put("total_layers", json!(t as u64));
        }
    }
    if active {
        if let Some(m) = s.remaining_min {
            put("remaining_minutes", json!(m.round() as i64));
        }
        if let Some(at) = finishes_at(s, now) {
            put("finishes_at", clock_from(zone, now, at).into());
        }
    }
    for (key, value) in [
        ("nozzle_celsius", s.nozzle),
        ("nozzle_target_celsius", s.nozzle_target),
        ("bed_celsius", s.bed),
        ("bed_target_celsius", s.bed_target),
        ("chamber_celsius", s.chamber),
    ] {
        if let Some(v) = value {
            put(key, json!(v.round() as i64));
        }
    }
    if let Some(light) = s.chamber_light {
        put("chamber_light", if light { "on" } else { "off" }.into());
    }
    if let Some(p) = problems(s) {
        put("problems", p.into());
    }
    Value::Object(facts)
}

/// What to say when a print ends, if anything. Somebody who stopped a
/// print knows; nothing is said for that.
pub fn announcement(action: Action, s: &Snapshot) -> Option<String> {
    match action {
        Action::Finished => Some(match &s.job {
            Some(job) => format!("The 3D printer has finished {job}."),
            None => "The 3D printer has finished printing.".into(),
        }),
        Action::Failed => Some(format!(
            "The 3D printer has stopped: {} failed at {}.",
            job(s),
            percent(s)
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Hms;

    const NOW: i64 = 1_792_065_600; // 2026-10-15 12:00 UTC, 14:00 in Copenhagen

    fn printing() -> Snapshot {
        Snapshot {
            state: Some(JobState::Running),
            job: Some("Benchy".into()),
            percent: Some(42.0),
            remaining_min: Some(65.0),
            layer: Some(87.0),
            total_layers: Some(210.0),
            nozzle: Some(219.6),
            nozzle_target: Some(220.0),
            bed: Some(55.2),
            bed_target: Some(55.0),
            chamber: Some(38.4),
            print_error: Some(0),
            ..Snapshot::default()
        }
    }

    fn cph() -> TimeZone {
        crate::clock::zone("Europe/Copenhagen")
    }

    #[test]
    fn durations_read_aloud() {
        assert_eq!(duration(0.2), "less than a minute");
        assert_eq!(duration(1.0), "1 minute");
        assert_eq!(duration(45.0), "45 minutes");
        assert_eq!(duration(60.0), "1 hour");
        assert_eq!(duration(65.0), "1 hour 5 minutes");
        assert_eq!(duration(121.0), "2 hours 1 minute");
        assert_eq!(short_duration(65.0), "1 h 5 min");
    }

    #[test]
    fn a_running_print_says_how_far_and_when_it_is_done() {
        assert_eq!(
            summary(&printing(), NOW, &cph()),
            "Printing Benchy: 42 percent done, layer 87 of 210, about 1 hour 5 minutes left, \
             done around 15:05. Nozzle 220 degrees, bed 55, chamber 38."
        );
    }

    #[test]
    fn status_carries_the_facts() {
        let status = status(&printing(), NOW, &cph());
        assert_eq!(status["state"], "printing");
        assert_eq!(status["progress_percent"], 42);
        assert_eq!(status["remaining_minutes"], 65);
        assert_eq!(status["finishes_at"], "15:05");
        assert_eq!(status["layer"], 87);
        assert_eq!(status["nozzle_celsius"], 220);
        assert!(status.get("problems").is_none());
    }

    #[test]
    fn an_idle_printer_has_no_eta() {
        let mut s = printing();
        s.state = Some(JobState::Finish);
        s.remaining_min = Some(0.0);
        let status = status(&s, NOW, &cph());
        assert!(status.get("finishes_at").is_none());
        assert!(status.get("remaining_minutes").is_none());
        assert!(
            status["summary"]
                .as_str()
                .unwrap()
                .starts_with("The printer is idle. The last print, Benchy, finished.")
        );
    }

    #[test]
    fn paused_failed_and_stopped_prints_say_so() {
        let mut s = printing();
        s.state = Some(JobState::Pause);
        assert!(
            summary(&s, NOW, &cph())
                .starts_with("Benchy is paused at 42 percent, layer 87 of 210.")
        );
        s.state = Some(JobState::Failed);
        s.print_error = Some(0x0300_8003);
        let text = summary(&s, NOW, &cph());
        assert!(text.starts_with("Benchy failed at 42 percent."), "{text}");
        assert!(
            text.ends_with("The printer reports error 0300_8003."),
            "{text}"
        );
        s.print_error = Some(CANCELLED);
        assert!(
            summary(&s, NOW, &cph())
                .starts_with("The printer is idle. Benchy was stopped at 42 percent.")
        );
    }

    #[test]
    fn health_messages_are_listed() {
        let mut s = printing();
        s.hms = vec![Hms {
            attr: 0x0300_0100,
            code: 0x0002_0001,
        }];
        assert_eq!(
            problems(&s).as_deref(),
            Some("The printer reports HMS 0300_0100_0002_0001 (serious).")
        );
    }

    #[test]
    fn a_refused_command_is_not_called_fatal() {
        let mut s = printing();
        s.hms = vec![VERIFICATION_FAILED];
        assert_eq!(
            problems(&s).as_deref(),
            Some(
                "The printer reports that it refused a command it could not verify (HMS 0500_0500_0001_0007)."
            )
        );
    }

    #[test]
    fn preparing_says_what_it_heats() {
        let mut s = printing();
        s.state = Some(JobState::Prepare);
        s.nozzle = Some(140.2);
        assert!(summary(&s, NOW, &cph()).starts_with(
            "Getting ready to print Benchy: heating the nozzle to 220 degrees, now at 140."
        ));
    }

    #[test]
    fn a_printer_that_has_said_nothing_says_so() {
        assert!(summary(&Snapshot::default(), NOW, &cph()).contains("has not said"));
    }

    #[test]
    fn only_endings_are_announced() {
        let s = printing();
        assert_eq!(
            announcement(Action::Finished, &s).as_deref(),
            Some("The 3D printer has finished Benchy.")
        );
        assert_eq!(
            announcement(Action::Failed, &s).as_deref(),
            Some("The 3D printer has stopped: Benchy failed at 42 percent.")
        );
        assert_eq!(announcement(Action::Cancelled, &s), None);
        assert_eq!(announcement(Action::Started, &s), None);
    }
}
