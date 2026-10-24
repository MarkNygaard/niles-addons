//! The "3D printer" page and the widget, drawn from what the plugin
//! already holds.

use crate::clock::clock_from;
use crate::model::{CANCELLED, JobState, Snapshot, VERIFICATION_FAILED, error_text};
use crate::niles::integration::ui::{
    self, Badge, Button, ButtonStyle, ListItem, Node, NodeKind, SwitchControl, TextBlock, Tone,
};
use crate::speech::{self, short_duration};
use crate::{App, Link, UiTree};

/// The flat node list a `ui-tree` is. Node 0 is the root, and a child
/// is always added after its parent, as the interface requires.
struct Tree(Vec<Node>);

impl Tree {
    fn new() -> Self {
        Tree(vec![Node {
            kind: NodeKind::Stack,
            children: Vec::new(),
        }])
    }

    fn add(&mut self, parent: u32, kind: NodeKind) -> u32 {
        let index = self.0.len() as u32;
        self.0.push(Node {
            kind,
            children: Vec::new(),
        });
        self.0[parent as usize].children.push(index);
        index
    }

    fn done(self) -> UiTree {
        UiTree { nodes: self.0 }
    }
}

fn text(value: impl Into<String>) -> NodeKind {
    NodeKind::Text(TextBlock {
        value: value.into(),
        muted: false,
    })
}

fn muted(value: impl Into<String>) -> NodeKind {
    NodeKind::Text(TextBlock {
        value: value.into(),
        muted: true,
    })
}

fn badge(label: &str, tone: Tone) -> NodeKind {
    NodeKind::Badge(Badge {
        label: label.into(),
        tone,
    })
}

fn reading(label: &str, value: String, unit: Option<&str>) -> NodeKind {
    NodeKind::Reading(ui::Reading {
        label: label.into(),
        value,
        unit: unit.map(str::to_string),
    })
}

fn button(id: &str, label: &str, style: ButtonStyle) -> NodeKind {
    NodeKind::Button(Button {
        id: id.into(),
        label: label.into(),
        style,
    })
}

fn state_badge(s: &Snapshot) -> NodeKind {
    match &s.state {
        None => badge("Waiting", Tone::Neutral),
        Some(JobState::Failed) if s.print_error == Some(CANCELLED) => {
            badge("Stopped", Tone::Neutral)
        }
        Some(state) => badge(
            &state.label(),
            match state {
                JobState::Running | JobState::Prepare | JobState::Slicing | JobState::Finish => {
                    Tone::Good
                }
                JobState::Pause => Tone::Warning,
                JobState::Failed => Tone::Bad,
                JobState::Idle | JobState::Other(_) => Tone::Neutral,
            },
        ),
    }
}

fn degrees(value: Option<f64>) -> String {
    value.map_or_else(|| "–".into(), |v| format!("{}", v.round() as i64))
}

fn target_label(name: &str, target: Option<f64>) -> String {
    match target.filter(|t| *t > 0.0) {
        Some(t) => format!("{name} (to {})", t.round() as i64),
        None => name.into(),
    }
}

/// The connection when it is not up: a badge and a sentence. None once connected.
fn link_problem(app: &App, now: i64) -> Option<(NodeKind, String)> {
    let address = &app.settings.address;
    match &app.link {
        Link::Connected => None,
        Link::Connecting => Some((
            badge("Connecting", Tone::Neutral),
            format!("Connecting to the printer at {address}."),
        )),
        Link::Offline(why) => Some((
            badge("Offline", Tone::Warning),
            format!(
                "The printer at {address} is not answering ({why}). It may be switched off; \
                 Niles keeps trying."
            ),
        )),
        Link::Failed { why, retry_at } => Some((
            badge("Not connected", Tone::Bad),
            format!(
                "Niles could not connect to the printer at {address}: {why}. Check the address \
                 and the access code. Trying again at {}.",
                clock_from(&app.settings.zone, now, *retry_at)
            ),
        )),
        Link::CertificateChanged { .. } => Some((
            badge("Certificate changed", Tone::Bad),
            "The printer's certificate has changed.".into(),
        )),
    }
}

fn ago(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 60 => format!("{s} s"),
        s if s < 3_600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3_600),
        s => format!("{} days", s / 86_400),
    }
}

pub fn page(app: &App, now: i64) -> UiTree {
    let mut tree = Tree::new();

    if let Link::CertificateChanged { trusted, presented } = &app.link {
        let card = tree.add(
            0,
            NodeKind::Card(Some("The printer's certificate has changed".into())),
        );
        tree.add(
            card,
            text(format!(
                "Niles stopped talking to the printer at {} because it showed a different \
                 certificate from the one it trusted before, and sent it nothing, not even the \
                 access code. A firmware update or a factory reset does this; so would another \
                 device answering at that address. If you have just updated or reset the \
                 printer, trust the new certificate.",
                app.settings.address
            )),
        );
        tree.add(
            card,
            button(
                "trust_certificate",
                "Trust the new certificate",
                ButtonStyle::Primary,
            ),
        );
        tree.add(card, muted(format!("Trusted before: {trusted}")));
        tree.add(card, muted(format!("Shown now: {presented}")));
    } else if let Some((badge, sentence)) = link_problem(app, now) {
        let row = tree.add(0, NodeKind::Row);
        tree.add(row, badge);
        tree.add(row, text(sentence));
        if matches!(app.link, Link::Failed { .. }) {
            tree.add(0, button("retry", "Try again now", ButtonStyle::Secondary));
        }
    }

    if app.link != Link::Connected {
        if let Some((known, at)) = &app.last_known
            && known.state.is_some()
        {
            tree.add(
                0,
                muted(format!(
                    "Last heard {} ago: {}",
                    ago(now - at),
                    speech::summary(known, *at, &app.settings.zone)
                )),
            );
        }
        return tree.done();
    }

    let Some(s) = app.snapshot() else {
        tree.add(0, text("Waiting for the printer's first report."));
        return tree.done();
    };
    let state = s.state.clone().unwrap_or(JobState::Idle);

    let top = tree.add(0, NodeKind::Row);
    tree.add(
        top,
        NodeKind::Heading(s.job.clone().unwrap_or_else(|| "No print job".into())),
    );
    tree.add(top, state_badge(&s));

    if state.is_active() || state == JobState::Failed {
        let row = tree.add(0, NodeKind::Row);
        tree.add(
            row,
            reading(
                "Progress",
                format!("{}", s.percent.unwrap_or(0.0).round() as i64),
                Some("%"),
            ),
        );
        if state.is_active() {
            if let Some(m) = s.remaining_min {
                tree.add(row, reading("Remaining", short_duration(m), None));
            }
            if let Some(at) = speech::finishes_at(&s, now) {
                tree.add(
                    row,
                    reading("Done at", clock_from(&app.settings.zone, now, at), None),
                );
            }
        }
        if let (Some(l), Some(t)) = (s.layer, s.total_layers) {
            tree.add(
                row,
                reading("Layer", format!("{} / {}", l as u64, t as u64), None),
            );
        }
    }

    let problems: Vec<ListItem> = s
        .print_error
        .filter(|e| *e != 0 && *e != CANCELLED)
        .map(|e| ListItem {
            title: format!("Print error {}", error_text(e)),
            detail: None,
            trailing: None,
        })
        .into_iter()
        .chain(s.hms.iter().map(|h| {
            if *h == VERIFICATION_FAILED {
                ListItem {
                    title: "Command verification failed".into(),
                    detail: Some(
                        "The printer refused a command from outside Bambu's apps, as it does in cloud mode."
                            .into(),
                    ),
                    trailing: Some(format!("HMS {}", h.code_text())),
                }
            } else {
                ListItem {
                    title: format!("HMS {}", h.code_text()),
                    detail: Some(
                        "Look the code up in Bambu Lab's wiki or the Bambu Handy app.".into(),
                    ),
                    trailing: Some(h.severity().into()),
                }
            }
        }))
        .collect();
    if !problems.is_empty() {
        let card = tree.add(0, NodeKind::Card(Some("The printer reports".into())));
        tree.add(card, NodeKind::Items(problems));
    }

    if state.is_active() && app.control_refused.is_some() {
        // Calm, in place of buttons that would only fail again.
        tree.add(
            0,
            muted(
                "Pause, resume and stop are not offered: the printer refused them. In cloud mode Bambu only lets its own apps control it. Use the Bambu app, or turn on Developer mode (LAN-only) on the printer and try again.",
            ),
        );
        tree.add(
            0,
            button(
                "control_retry",
                "Try the controls again",
                ButtonStyle::Secondary,
            ),
        );
    } else if state.is_active() {
        let row = tree.add(0, NodeKind::Row);
        match state {
            JobState::Pause => tree.add(row, button("resume", "Resume", ButtonStyle::Primary)),
            _ => tree.add(row, button("pause", "Pause", ButtonStyle::Secondary)),
        };
        if !app.stop_armed(now) {
            tree.add(row, button("stop", "Stop", ButtonStyle::Destructive));
        } else {
            let card = tree.add(0, NodeKind::Card(Some("Stop the print?".into())));
            tree.add(
                card,
                text(format!(
                    "Stopping ends {} for good. It cannot be resumed.",
                    s.job.as_deref().unwrap_or("the print")
                )),
            );
            let row = tree.add(card, NodeKind::Row);
            tree.add(
                row,
                button(
                    "stop_confirm",
                    "Yes, stop the print",
                    ButtonStyle::Destructive,
                ),
            );
            tree.add(
                row,
                button("stop_cancel", "Keep printing", ButtonStyle::Secondary),
            );
        }
    }

    let card = tree.add(0, NodeKind::Card(Some("Temperatures".into())));
    let row = tree.add(card, NodeKind::Row);
    tree.add(
        row,
        reading(
            &target_label("Nozzle", s.nozzle_target),
            degrees(s.nozzle),
            Some("°C"),
        ),
    );
    tree.add(
        row,
        reading(
            &target_label("Bed", s.bed_target),
            degrees(s.bed),
            Some("°C"),
        ),
    );
    if s.chamber.is_some() {
        tree.add(row, reading("Chamber", degrees(s.chamber), Some("°C")));
    }

    tree.add(
        0,
        NodeKind::Switch(SwitchControl {
            id: "light".into(),
            label: "Chamber light".into(),
            value: app.light_shown(now),
        }),
    );
    if app.light_refused(now) {
        tree.add(
            0,
            muted(
                "The printer didn't accept the light command. In cloud mode Bambu may only let its own apps control it.",
            ),
        );
    }

    let mut footnote = Vec::new();
    let fans: Vec<String> = [
        ("part", s.part_fan),
        ("auxiliary", s.aux_fan),
        ("chamber", s.chamber_fan),
    ]
    .into_iter()
    .filter_map(|(name, v)| v.map(|v| format!("{name} {}%", v as i64)))
    .collect();
    if !fans.is_empty() {
        footnote.push(format!("Fans: {}.", fans.join(", ")));
    }
    if let Some(at) = app.last_message {
        footnote.push(format!("Last report {} ago.", ago(now - at)));
    }
    if !footnote.is_empty() {
        tree.add(0, muted(footnote.join(" ")));
    }
    tree.done()
}

/// The compact card for a room or the front page.
pub fn widget(app: &App, now: i64) -> UiTree {
    let mut tree = Tree::new();
    if let Some((badge, sentence)) = link_problem(app, now) {
        tree.add(0, badge);
        if matches!(app.link, Link::CertificateChanged { .. }) {
            tree.add(0, muted("Open the 3D printer page to trust the new one."));
        } else {
            tree.add(0, muted(sentence));
        }
        return tree.done();
    }
    let Some(s) = app.snapshot() else {
        tree.add(0, muted("Waiting for the printer's first report."));
        return tree.done();
    };
    let state = s.state.clone().unwrap_or(JobState::Idle);
    let top = tree.add(0, NodeKind::Row);
    tree.add(top, state_badge(&s));
    if let Some(job) = &s.job {
        tree.add(top, text(job.as_str()));
    }
    if state.is_active() {
        let row = tree.add(0, NodeKind::Row);
        tree.add(
            row,
            reading(
                "Progress",
                format!("{}", s.percent.unwrap_or(0.0).round() as i64),
                Some("%"),
            ),
        );
        match speech::finishes_at(&s, now) {
            Some(at) => tree.add(
                row,
                reading("Done at", clock_from(&app.settings.zone, now, at), None),
            ),
            None => tree.add(row, reading("Remaining", "–".into(), None)),
        };
    } else {
        tree.add(
            0,
            muted(format!(
                "Nozzle {} °C · bed {} °C{}",
                degrees(s.nozzle),
                degrees(s.bed),
                s.chamber
                    .map(|c| format!(" · chamber {} °C", c.round() as i64))
                    .unwrap_or_default()
            )),
        );
    }
    tree.done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;
    use serde_json::json;

    const NOW: i64 = 1_792_065_600;

    fn app(link: Link, messages: &[serde_json::Value]) -> App {
        let mut app = App::new(Settings {
            address: "192.168.1.40".into(),
            serial: "00M09A350100001".into(),
            room: "office".into(),
            announce_room: None,
            zone: crate::clock::zone("Europe/Copenhagen"),
        });
        app.link = link;
        for m in messages {
            app.report.merge(m);
        }
        app
    }

    fn printing() -> serde_json::Value {
        json!({"print": {
            "gcode_state": "RUNNING", "subtask_name": "Benchy", "mc_percent": 42,
            "mc_remaining_time": 65, "layer_num": 87, "total_layer_num": 210,
            "nozzle_temper": 220.1, "nozzle_target_temper": 220, "bed_temper": 55,
            "bed_target_temper": 55, "chamber_temper": 38,
            "lights_report": [{"node": "chamber_light", "mode": "on"}]
        }})
    }

    /// The controls' ids, and every text in the tree, in order.
    fn controls(tree: &UiTree) -> Vec<String> {
        tree.nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Button(b) => Some(b.id.clone()),
                NodeKind::Switch(s) => Some(s.id.clone()),
                _ => None,
            })
            .collect()
    }

    fn words(tree: &UiTree) -> String {
        tree.nodes
            .iter()
            .map(|n| match &n.kind {
                NodeKind::Heading(h) => h.clone(),
                NodeKind::Text(t) => t.value.clone(),
                NodeKind::Badge(b) => b.label.clone(),
                NodeKind::Reading(r) => format!("{}={}", r.label, r.value),
                NodeKind::Card(Some(title)) => title.clone(),
                NodeKind::Button(b) => b.label.clone(),
                _ => String::new(),
            })
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("|")
    }

    /// The interface's rules: children after parents, one parent each.
    fn well_formed(tree: &UiTree) {
        let mut parent = vec![None; tree.nodes.len()];
        for (i, node) in tree.nodes.iter().enumerate() {
            for &c in &node.children {
                assert!(c as usize > i);
                assert!(parent[c as usize].replace(i).is_none());
            }
        }
        assert!(parent.iter().skip(1).all(Option::is_some));
    }

    #[test]
    fn a_running_print_shows_progress_controls_and_light() {
        let tree = page(&app(Link::Connected, &[printing()]), NOW);
        well_formed(&tree);
        let words = words(&tree);
        for expected in [
            "Benchy",
            "Printing",
            "Progress=42",
            "Remaining=1 h 5 min",
            "Done at=15:05",
            "Layer=87 / 210",
            "Nozzle (to 220)=220",
        ] {
            assert!(words.contains(expected), "{expected} in {words}");
        }
        assert_eq!(controls(&tree), ["pause", "stop", "light"]);
    }

    #[test]
    fn stop_asks_first() {
        let mut app = app(Link::Connected, &[printing()]);
        app.stop_armed_until = Some(NOW + 10);
        let tree = page(&app, NOW);
        well_formed(&tree);
        assert_eq!(
            controls(&tree),
            ["pause", "stop_confirm", "stop_cancel", "light"]
        );
        assert_eq!(controls(&page(&app, NOW + 11)), ["pause", "stop", "light"]);
    }

    #[test]
    fn a_printer_that_refused_control_gets_a_note_not_buttons() {
        let mut app = app(Link::Connected, &[printing()]);
        app.control_refused = Some(NOW - 60);
        let tree = page(&app, NOW);
        well_formed(&tree);
        assert_eq!(controls(&tree), ["control_retry", "light"]);
        assert!(words(&tree).contains("In cloud mode Bambu only lets its own apps control it"));
    }

    #[test]
    fn a_refused_light_says_so_for_a_while() {
        let mut app = app(Link::Connected, &[printing()]);
        app.light_refused_at = Some(NOW - 10);
        assert!(words(&page(&app, NOW)).contains("didn't accept the light command"));
        assert!(!words(&page(&app, NOW + 600)).contains("didn't accept the light command"));
    }

    #[test]
    fn a_paused_print_offers_resume() {
        let mut paused = printing();
        paused["print"]["gcode_state"] = "PAUSE".into();
        assert_eq!(
            controls(&page(&app(Link::Connected, &[paused]), NOW)),
            ["resume", "stop", "light"]
        );
    }

    #[test]
    fn an_idle_printer_has_only_the_light() {
        let idle = json!({"print": {"gcode_state": "IDLE", "nozzle_temper": 25, "bed_temper": 24}});
        let tree = page(&app(Link::Connected, &[idle]), NOW);
        assert_eq!(controls(&tree), ["light"]);
        assert!(words(&tree).contains("No print job|Idle"));
    }

    #[test]
    fn a_changed_certificate_asks_to_trust_it() {
        let link = Link::CertificateChanged {
            trusted: "aa:bb".into(),
            presented: "cc:dd".into(),
        };
        let tree = page(&app(link.clone(), &[printing()]), NOW);
        well_formed(&tree);
        assert_eq!(controls(&tree), ["trust_certificate"]);
        assert!(words(&tree).contains("Shown now: cc:dd"));
        let widget = widget(&app(link, &[]), NOW);
        assert!(words(&widget).contains("Certificate changed"));
        assert!(controls(&widget).is_empty(), "never trusted from a widget");
    }

    #[test]
    fn connection_states_say_what_is_wrong() {
        let offline = page(&app(Link::Offline("connection refused".into()), &[]), NOW);
        assert!(
            words(&offline).contains(
                "Offline|The printer at 192.168.1.40 is not answering (connection refused)"
            )
        );
        let failed = page(
            &app(
                Link::Failed {
                    why: "the broker refused the username or password".into(),
                    retry_at: NOW + 300,
                },
                &[],
            ),
            NOW,
        );
        assert!(words(&failed).contains("Not connected"));
        assert!(words(&failed).contains("Trying again at 14:05"));
        assert_eq!(controls(&failed), ["retry"]);
        assert!(words(&page(&app(Link::Connecting, &[]), NOW)).contains("Connecting"));
        assert!(
            words(&page(&app(Link::Connected, &[]), NOW))
                .contains("Waiting for the printer's first report")
        );
    }

    #[test]
    fn the_last_report_is_shown_while_offline() {
        let mut app = app(Link::Offline("timed out".into()), &[]);
        let mut known = crate::model::Report::default();
        known.merge(&printing());
        app.last_known = Some((known.snapshot(), NOW - 600));
        assert!(
            words(&page(&app, NOW))
                .contains("Last heard 10 min ago: Printing Benchy: 42 percent done")
        );
    }

    #[test]
    fn a_failure_lists_its_errors() {
        let mut failed = printing();
        failed["print"]["gcode_state"] = "FAILED".into();
        failed["print"]["print_error"] = json!(0x0300_8003u32);
        failed["print"]["hms"] = json!([{"attr": 0x0300_0100u32, "code": 0x0002_0001u32}]);
        let tree = page(&app(Link::Connected, &[failed]), NOW);
        let words = words(&tree);
        assert!(words.contains("Failed"));
        assert!(words.contains("The printer reports"));
        assert_eq!(controls(&tree), ["light"]);
        assert!(
            tree.nodes
                .iter()
                .any(|n| matches!(&n.kind, NodeKind::Items(items) if items.len() == 2))
        );
    }

    #[test]
    fn the_widget_is_compact() {
        let tree = widget(&app(Link::Connected, &[printing()]), NOW);
        well_formed(&tree);
        assert_eq!(words(&tree), "Printing|Benchy|Progress=42|Done at=15:05");
        assert!(controls(&tree).is_empty());
    }
}
