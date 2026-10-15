//! What the printer reports, merged into one picture.
//!
//! The printer publishes JSON on `device/<serial>/report`. Its state is
//! under `print`: an X1C usually sends all of it, a P1 only what changed,
//! and either answers a `pushall` with everything. Each report is merged
//! key by key into what is known, so a partial one changes only what it
//! carries, and keys this plugin does not know are kept and ignored.

use serde_json::{Map, Value};

/// `print_error` when somebody stopped the print, rather than it failing.
pub const CANCELLED: u64 = 0x0300_400C;

/// Keys under `print` that belong to the message, not to the printer's state.
const ENVELOPE: [&str; 6] = ["command", "sequence_id", "msg", "result", "reason", "param"];

/// `gcode_state`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Idle,
    Slicing,
    Prepare,
    Running,
    Pause,
    Finish,
    Failed,
    Other(String),
}

impl JobState {
    pub fn parse(text: &str) -> JobState {
        match text.trim().to_ascii_uppercase().as_str() {
            "IDLE" | "INIT" => JobState::Idle,
            "SLICING" => JobState::Slicing,
            "PREPARE" => JobState::Prepare,
            "RUNNING" => JobState::Running,
            "PAUSE" => JobState::Pause,
            "FINISH" => JobState::Finish,
            "FAILED" => JobState::Failed,
            other => JobState::Other(other.to_string()),
        }
    }

    /// As the printer says it, for storage.
    pub fn code(&self) -> &str {
        match self {
            JobState::Idle => "IDLE",
            JobState::Slicing => "SLICING",
            JobState::Prepare => "PREPARE",
            JobState::Running => "RUNNING",
            JobState::Pause => "PAUSE",
            JobState::Finish => "FINISH",
            JobState::Failed => "FAILED",
            JobState::Other(other) => other,
        }
    }

    /// A print is under way, paused or not.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            JobState::Slicing | JobState::Prepare | JobState::Running | JobState::Pause
        )
    }

    pub fn label(&self) -> String {
        match self {
            JobState::Idle => "Idle".into(),
            JobState::Slicing => "Slicing".into(),
            JobState::Prepare => "Preparing".into(),
            JobState::Running => "Printing".into(),
            JobState::Pause => "Paused".into(),
            JobState::Finish => "Finished".into(),
            JobState::Failed => "Failed".into(),
            JobState::Other(other) => {
                let lower = other.to_lowercase();
                let mut chars = lower.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_else(|| "Unknown".into())
            }
        }
    }
}

/// What the printer did, reported as an action on the job device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Started,
    Paused,
    Resumed,
    Finished,
    Failed,
    /// Stopped by somebody, from the printer, the app or here.
    Cancelled,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Started => "started",
            Action::Paused => "paused",
            Action::Resumed => "resumed",
            Action::Finished => "finished",
            Action::Failed => "failed",
            Action::Cancelled => "cancelled",
        }
    }
}

/// The action a change of `gcode_state` amounts to, if any. Nothing
/// without a previous state: a first report says where things stand,
/// not that anything happened.
pub fn transition(prev: Option<&JobState>, next: &JobState, error: Option<u64>) -> Option<Action> {
    let prev = prev?;
    if prev == next {
        return None;
    }
    match next {
        JobState::Slicing | JobState::Prepare | JobState::Running if !prev.is_active() => {
            Some(Action::Started)
        }
        JobState::Running if *prev == JobState::Pause => Some(Action::Resumed),
        JobState::Pause if prev.is_active() => Some(Action::Paused),
        JobState::Finish if prev.is_active() => Some(Action::Finished),
        JobState::Failed if prev.is_active() => Some(if error == Some(CANCELLED) {
            Action::Cancelled
        } else {
            Action::Failed
        }),
        _ => None,
    }
}

/// One entry of `hms`, the printer's health messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hms {
    pub attr: u64,
    pub code: u64,
}

impl Hms {
    /// As Bambu's wiki and app write it: `0300_0100_0001_0007`.
    pub fn code_text(&self) -> String {
        format!(
            "{:04X}_{:04X}_{:04X}_{:04X}",
            self.attr >> 16,
            self.attr & 0xFFFF,
            self.code >> 16,
            self.code & 0xFFFF
        )
    }

    /// From the top half of `code`, as Home Assistant's ha-bambulab reads it.
    pub fn severity(&self) -> &'static str {
        match self.code >> 16 {
            1 => "fatal",
            2 => "serious",
            3 => "common",
            4 => "info",
            _ => "unknown",
        }
    }
}

/// `print_error` as Bambu writes it: `0300_400C`.
pub fn error_text(code: u64) -> String {
    let hex = format!("{code:08X}");
    format!("{}_{}", &hex[..4], &hex[4..])
}

/// The fields this plugin uses, from the merged report. Every one is
/// optional: a printer that has not said it yet, or never does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub state: Option<JobState>,
    pub job: Option<String>,
    pub percent: Option<f64>,
    pub remaining_min: Option<f64>,
    pub layer: Option<f64>,
    pub total_layers: Option<f64>,
    pub nozzle: Option<f64>,
    pub nozzle_target: Option<f64>,
    pub bed: Option<f64>,
    pub bed_target: Option<f64>,
    pub chamber: Option<f64>,
    pub print_error: Option<u64>,
    pub hms: Vec<Hms>,
    pub chamber_light: Option<bool>,
    /// Percent, from the printer's 0-15 steps.
    pub part_fan: Option<f64>,
    pub aux_fan: Option<f64>,
    pub chamber_fan: Option<f64>,
}

impl Snapshot {
    /// An error worth telling somebody about: `print_error` that is not
    /// a cancel, or a health message.
    pub fn has_error(&self) -> bool {
        self.print_error.is_some_and(|e| e != 0 && e != CANCELLED) || !self.hms.is_empty()
    }
}

/// Everything the printer has said under `print`, merged.
#[derive(Clone, Debug, Default)]
pub struct Report {
    print: Map<String, Value>,
}

impl Report {
    /// Merge one message. True if it carried any printer state.
    pub fn merge(&mut self, message: &Value) -> bool {
        let Some(print) = message.get("print").and_then(Value::as_object) else {
            return false;
        };
        let mut changed = false;
        for (key, value) in print {
            if ENVELOPE.contains(&key.as_str()) {
                continue;
            }
            self.print.insert(key.clone(), value.clone());
            changed = true;
        }
        changed
    }

    pub fn is_empty(&self) -> bool {
        self.print.is_empty()
    }

    /// The chamber light as a message reports it: in `lights_report`,
    /// or as the answer to a `ledctrl` command.
    pub fn light_in(message: &Value) -> Option<bool> {
        if let Some(system) = message.get("system")
            && system.get("command").and_then(Value::as_str) == Some("ledctrl")
            && system.get("led_node").and_then(Value::as_str) == Some("chamber_light")
            && system.get("result").and_then(Value::as_str) == Some("success")
        {
            return system.get("led_mode").and_then(Value::as_str).map(is_on);
        }
        chamber_light(message.get("print")?.get("lights_report")?)
    }

    pub fn snapshot(&self) -> Snapshot {
        let get = |key: &str| self.print.get(key);
        let number = |key: &str| get(key).and_then(number);
        Snapshot {
            state: get("gcode_state")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(JobState::parse),
            job: get("subtask_name")
                .and_then(Value::as_str)
                .map(job_name)
                .filter(|s| !s.is_empty()),
            percent: number("mc_percent"),
            remaining_min: number("mc_remaining_time"),
            layer: number("layer_num"),
            total_layers: number("total_layer_num"),
            nozzle: number("nozzle_temper"),
            nozzle_target: number("nozzle_target_temper"),
            bed: number("bed_temper"),
            bed_target: number("bed_target_temper"),
            chamber: number("chamber_temper"),
            print_error: number("print_error").map(|e| e as u64),
            hms: get("hms")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|h| {
                            Some(Hms {
                                attr: number_in(h, "attr")? as u64,
                                code: number_in(h, "code")? as u64,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            chamber_light: get("lights_report").and_then(chamber_light),
            part_fan: number("cooling_fan_speed").map(fan_percent),
            aux_fan: number("big_fan1_speed").map(fan_percent),
            chamber_fan: number("big_fan2_speed").map(fan_percent),
        }
    }
}

fn is_on(mode: &str) -> bool {
    mode != "off"
}

fn chamber_light(lights: &Value) -> Option<bool> {
    lights
        .as_array()?
        .iter()
        .find(|l| l.get("node").and_then(Value::as_str) == Some("chamber_light"))?
        .get("mode")
        .and_then(Value::as_str)
        .map(is_on)
}

/// A number, whether the printer sent it as one or as a string (fan
/// speeds come as `"15"`).
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n: &f64| n.is_finite())
}

fn number_in(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(number)
}

/// Fans report 0-15; shown in steps of ten percent, as the printer's
/// own screen does.
fn fan_percent(raw: f64) -> f64 {
    ((raw.clamp(0.0, 15.0) / 15.0 * 10.0).round()) * 10.0
}

/// `subtask_name` is the file's name, sometimes with its extension.
fn job_name(name: &str) -> String {
    let name = name.trim();
    for ext in [".gcode.3mf", ".3mf", ".gcode"] {
        if name.len() > ext.len() && name.to_ascii_lowercase().ends_with(ext) {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn merged(messages: &[Value]) -> Snapshot {
        let mut report = Report::default();
        for m in messages {
            report.merge(m);
        }
        report.snapshot()
    }

    #[test]
    fn a_full_report_reads_every_field() {
        let s = merged(&[json!({"print": {
            "gcode_state": "RUNNING", "subtask_name": "Benchy.3mf",
            "mc_percent": 42, "mc_remaining_time": 65,
            "layer_num": 87, "total_layer_num": 210,
            "nozzle_temper": 219.8, "nozzle_target_temper": 220,
            "bed_temper": 54.9, "bed_target_temper": 55.0, "chamber_temper": 38,
            "print_error": 0, "hms": [],
            "lights_report": [{"node": "chamber_light", "mode": "on"}, {"node": "work_light", "mode": "flashing"}],
            "cooling_fan_speed": "15", "big_fan1_speed": "10", "big_fan2_speed": "0",
            "command": "push_status", "msg": 0, "sequence_id": "2021"
        }})]);
        assert_eq!(s.state, Some(JobState::Running));
        assert_eq!(s.job.as_deref(), Some("Benchy"));
        assert_eq!(s.percent, Some(42.0));
        assert_eq!(s.remaining_min, Some(65.0));
        assert_eq!((s.layer, s.total_layers), (Some(87.0), Some(210.0)));
        assert_eq!((s.nozzle, s.nozzle_target), (Some(219.8), Some(220.0)));
        assert_eq!(
            (s.bed, s.bed_target, s.chamber),
            (Some(54.9), Some(55.0), Some(38.0))
        );
        assert_eq!(s.chamber_light, Some(true));
        assert_eq!(
            (s.part_fan, s.aux_fan, s.chamber_fan),
            (Some(100.0), Some(70.0), Some(0.0))
        );
        assert!(!s.has_error());
    }

    #[test]
    fn partial_reports_change_only_what_they_carry() {
        let s = merged(&[
            json!({"print": {"gcode_state": "RUNNING", "mc_percent": 10, "nozzle_temper": 220.0}}),
            json!({"print": {"mc_percent": 11}}),
            json!({"print": {"nozzle_temper": 221.5, "command": "push_status"}}),
        ]);
        assert_eq!(s.state, Some(JobState::Running));
        assert_eq!(s.percent, Some(11.0));
        assert_eq!(s.nozzle, Some(221.5));
    }

    #[test]
    fn unknown_fields_and_other_messages_are_ignored() {
        let mut report = Report::default();
        assert!(!report.merge(&json!({"info": {"command": "get_version"}})));
        assert!(!report.merge(
            &json!({"print": {"command": "pause", "result": "success", "sequence_id": "0"}})
        ));
        assert!(report.is_empty());
        assert!(report.merge(&json!({"print": {"some_new_field": {"x": 1}, "mc_percent": "7"}})));
        assert_eq!(report.snapshot().percent, Some(7.0));
    }

    #[test]
    fn wrong_types_read_as_missing() {
        let s = merged(&[
            json!({"print": {"mc_percent": null, "gcode_state": 3, "hms": "none", "bed_temper": "warm"}}),
        ]);
        assert_eq!(s, Snapshot::default());
    }

    #[test]
    fn transitions_become_actions() {
        use JobState::*;
        let t = |prev: JobState, next: JobState| transition(Some(&prev), &next, Some(0));
        assert_eq!(t(Idle, Prepare), Some(Action::Started));
        assert_eq!(t(Finish, Running), Some(Action::Started));
        assert_eq!(t(Prepare, Running), None);
        assert_eq!(t(Running, Pause), Some(Action::Paused));
        assert_eq!(t(Pause, Running), Some(Action::Resumed));
        assert_eq!(t(Running, Finish), Some(Action::Finished));
        assert_eq!(t(Running, Failed), Some(Action::Failed));
        assert_eq!(t(Pause, Failed), Some(Action::Failed));
        assert_eq!(t(Finish, Idle), None);
        assert_eq!(
            t(Idle, Finish),
            None,
            "a printer that boots into FINISH did not just finish"
        );
        assert_eq!(t(Running, Running), None);
        assert_eq!(transition(None, &Running, None), None);
    }

    #[test]
    fn a_stop_is_a_cancel_not_a_failure() {
        assert_eq!(
            transition(Some(&JobState::Running), &JobState::Failed, Some(CANCELLED)),
            Some(Action::Cancelled)
        );
    }

    #[test]
    fn errors_are_written_as_bambu_writes_them() {
        assert_eq!(error_text(CANCELLED), "0300_400C");
        let hms = Hms {
            attr: 0x0300_0100,
            code: 0x0001_0007,
        };
        assert_eq!(hms.code_text(), "0300_0100_0001_0007");
        assert_eq!(hms.severity(), "fatal");
        let s = merged(&[json!({"print": {"hms": [{"attr": 50331904, "code": 65543}]}})]);
        assert_eq!(s.hms, vec![hms]);
        assert!(s.has_error());
    }

    #[test]
    fn the_light_is_read_from_reports_and_command_answers() {
        assert_eq!(
            Report::light_in(
                &json!({"print": {"lights_report": [{"node": "chamber_light", "mode": "off"}]}})
            ),
            Some(false)
        );
        assert_eq!(
            Report::light_in(
                &json!({"system": {"command": "ledctrl", "led_node": "chamber_light", "led_mode": "on", "result": "success"}})
            ),
            Some(true)
        );
        assert_eq!(
            Report::light_in(
                &json!({"system": {"command": "ledctrl", "led_node": "chamber_light", "led_mode": "on", "result": "failed"}})
            ),
            None
        );
    }

    #[test]
    fn job_names_lose_their_extension() {
        assert_eq!(job_name("Benchy.gcode.3mf"), "Benchy");
        assert_eq!(job_name("plate_1.gcode"), "plate_1");
        assert_eq!(job_name(".3mf"), ".3mf");
        assert_eq!(job_name("Cable clip"), "Cable clip");
    }

    #[test]
    fn unknown_states_keep_their_name() {
        let state = JobState::parse("offline");
        assert_eq!(state.code(), "OFFLINE");
        assert_eq!(state.label(), "Offline");
        assert!(!state.is_active());
    }
}
