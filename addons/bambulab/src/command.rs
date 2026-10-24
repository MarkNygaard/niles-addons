//! The JSON the printer takes on `device/<serial>/request`, as Bambu
//! Studio and Home Assistant's ha-bambulab send it.

use serde_json::{Value, json};

/// Ask for every field, not just what changed.
pub fn pushall() -> Value {
    json!({"pushing": {"sequence_id": "0", "command": "pushall"}})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Pause,
    Resume,
    Stop,
}

impl Job {
    /// The printer's name for it, which is also the English verb.
    pub fn verb(self) -> &'static str {
        match self {
            Job::Pause => "pause",
            Job::Resume => "resume",
            Job::Stop => "stop",
        }
    }
}

pub fn job(command: Job) -> Value {
    json!({"print": {"sequence_id": "0", "command": command.verb(), "param": ""}})
}

/// The chamber light on or off. The timing fields only matter for
/// flashing, but the printer wants them there.
pub fn chamber_light(on: bool) -> Value {
    json!({"system": {
        "sequence_id": "0",
        "command": "ledctrl",
        "led_node": "chamber_light",
        "led_mode": if on { "on" } else { "off" },
        "led_on_time": 500,
        "led_off_time": 500,
        "loop_times": 0,
        "interval_time": 0
    }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_what_the_printer_expects() {
        assert_eq!(
            pushall().to_string(),
            r#"{"pushing":{"command":"pushall","sequence_id":"0"}}"#
        );
        assert_eq!(job(Job::Pause)["print"]["command"], "pause");
        assert_eq!(job(Job::Resume)["print"]["command"], "resume");
        assert_eq!(job(Job::Stop)["print"]["command"], "stop");
        let light = chamber_light(false);
        assert_eq!(light["system"]["command"], "ledctrl");
        assert_eq!(light["system"]["led_node"], "chamber_light");
        assert_eq!(light["system"]["led_mode"], "off");
        assert_eq!(chamber_light(true)["system"]["led_mode"], "on");
        for field in ["led_on_time", "led_off_time", "loop_times", "interval_time"] {
            assert!(light["system"].get(field).is_some(), "{field}");
        }
    }
}
