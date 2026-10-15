//! A Bambu Lab X1C for Niles.
//!
//! The printer runs its own MQTT broker on the LAN (port 8883, TLS with
//! a certificate nobody vouches for, user `bblp`, the LAN access code as
//! password). Niles holds the connection; this plugin subscribes to the
//! printer's reports, merges them into one picture, and turns it into
//! two devices in the chosen room:
//!
//! - `<room>/printer`, a sensor whose readings are the job's progress,
//!   time left, layers and temperatures, and which reports what the job
//!   does (`started`, `paused`, `resumed`, `finished`, `failed`,
//!   `cancelled`) as actions, for rules;
//! - `<room>/printer_light`, the chamber light, switchable, and kept out
//!   of the room's lights so "lights off" never darkens a print.
//!
//! It offers tools for the printer's status, pausing, resuming and
//! stopping a print and the light, a "3D printer" page and a widget, and
//! says when a print finishes or fails, if asked to.

// The bindings wit-bindgen generates for the interface's wider records
// and functions take more arguments than clippy likes.
#![allow(clippy::too_many_arguments)]

wit_bindgen::generate!({
    world: "plugin",
    path: "wit",
});

mod clock;
mod command;
mod model;
mod speech;
mod view;

use model::{Action, JobState, Report, Snapshot};
use niles::integration::mqtt::{self, ConnectOptions, Qos};
use niles::integration::types::{Device, DeviceClass, HostError, LightCapabilities, Reading};
use niles::integration::{config, devices, log, notify, storage, timers, ui};
use serde_json::{Value, json};
use speech::capitalised;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const BROKER: &str = "printer";
const PAGE: &str = "printer";
const WIDGET: &str = "printer_now";

const TICK: &str = "tick";
const RETRY: &str = "retry";
const REDRAW: &str = "redraw";
const CHECK: &str = "check_command";

/// The last `gcode_state` seen, so a restart neither misses an ending
/// nor reports one twice.
const LAST_STATE: &str = "last_state";
/// The essentials of the last report, for the page while the printer is away.
const LAST_KNOWN: &str = "last_known";
/// When the printer last refused to be paused, resumed or stopped: in
/// cloud mode it takes those only from Bambu's own apps.
const REFUSED: &str = "control_refused";

/// Ask for a full report when the printer has been quiet this long.
const QUIET_SECONDS: i64 = 300;
/// After `failed`, try again after this long, doubling up to an hour.
const FIRST_RETRY: u32 = 300;
const LAST_RETRY: u32 = 3_600;
/// At most one redraw of the page and widget this often.
const REDRAW_SECONDS: i64 = 5;
/// A second tap on Stop counts this long after the first.
const STOP_ARMED_SECONDS: i64 = 15;
/// The light switch shows what was asked for this long, waiting for the printer.
const LIGHT_PENDING_SECONDS: i64 = 15;
/// Two announcements at least this far apart.
const ANNOUNCE_GAP_SECONDS: i64 = 60;
/// A command with neither an answer nor an effect by then was not taken.
/// Pausing waits for the current move, so not too short.
const COMMAND_SECONDS: u32 = 30;
/// How long the page says the light command was refused.
const LIGHT_REFUSED_SECONDS: i64 = 120;

/// What to tell somebody whose command the printer refused.
pub const CLOUD_MODE: &str = "The printer didn't accept that: in cloud mode Bambu only lets its \
    own apps control it. Use the Bambu app, or turn on Developer mode (LAN-only) on the printer.";

struct BambuLab;

export!(BambuLab);

/// The settings, read once in `init`: saving them restarts the plugin.
pub struct Settings {
    pub address: String,
    pub serial: String,
    pub room: String,
    pub announce_room: Option<String>,
    pub zone: jiff::tz::TimeZone,
}

impl Settings {
    fn read() -> Result<Settings, String> {
        let address = text("address").ok_or("The printer's address is not set.")?;
        let serial = text("serial").ok_or("The printer's serial number is not set.")?;
        let room = text("room").ok_or("The printer's room is not set.")?;
        if config::secret("access_code").is_none_or(|c| c.trim().is_empty()) {
            return Err("The LAN access code is not set.".into());
        }
        if serial.contains(['/', '+', '#']) {
            return Err("The serial number has characters a serial number never has.".into());
        }
        Ok(Settings {
            address,
            serial: serial.to_uppercase(),
            room,
            announce_room: text("announce_room"),
            zone: clock::zone(&config::time_zone()),
        })
    }

    pub fn printer(&self) -> String {
        format!("{}/printer", self.room)
    }

    pub fn light(&self) -> String {
        format!("{}/printer_light", self.room)
    }

    fn devices(&self) -> [String; 2] {
        [self.printer(), self.light()]
    }

    fn report_topic(&self) -> String {
        format!("device/{}/report", self.serial)
    }

    fn request_topic(&self) -> String {
        format!("device/{}/request", self.serial)
    }
}

/// Who asked for a command, which decides how a refusal is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// Voice or a chat assistant, through a tool: said aloud.
    Assistant,
    /// The page, which redraws with the note.
    Page,
}

/// A pause, resume or stop sent and not yet settled.
struct Pending {
    which: command::Job,
    from: JobState,
    origin: Origin,
    /// The printer answered that it took it.
    accepted: bool,
}

/// The connection to the printer, as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Connecting,
    Connected,
    /// Dropped, or could not be made; Niles is trying again.
    Offline(String),
    /// Niles stopped trying; this plugin tries again at `retry_at`.
    Failed {
        why: String,
        retry_at: i64,
    },
    /// The printer showed another certificate. Waits for the household.
    CertificateChanged {
        trusted: String,
        presented: String,
    },
}

pub struct App {
    pub settings: Settings,
    conn: Option<u32>,
    pub link: Link,
    report: Report,
    pub last_message: Option<i64>,
    last_state: Option<JobState>,
    /// From storage: what the printer last said, and when.
    pub last_known: Option<(Snapshot, i64)>,
    saved: String,
    reported: HashMap<&'static str, f64>,
    pub light: Option<bool>,
    pub light_pending: Option<(bool, i64)>,
    pub stop_armed_until: Option<i64>,
    retry_delay: u32,
    redraw_key: String,
    redraw_last: i64,
    redraw_pending: bool,
    last_announce: i64,
    last_pushall: i64,
    pending: Option<Pending>,
    /// When the printer refused to be controlled, remembered until someone
    /// asks to try again, so nobody is offered buttons that fail.
    pub control_refused: Option<i64>,
    pub light_refused_at: Option<i64>,
}

impl App {
    fn new(settings: Settings) -> App {
        App {
            settings,
            conn: None,
            link: Link::Connecting,
            report: Report::default(),
            last_message: None,
            last_state: None,
            last_known: None,
            saved: String::new(),
            reported: HashMap::new(),
            light: None,
            light_pending: None,
            stop_armed_until: None,
            retry_delay: FIRST_RETRY,
            redraw_key: String::new(),
            redraw_last: 0,
            redraw_pending: false,
            last_announce: 0,
            last_pushall: 0,
            pending: None,
            control_refused: None,
            light_refused_at: None,
        }
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        (!self.report.is_empty()).then(|| self.report.snapshot())
    }

    /// The chamber light as the page should show it: what was just asked
    /// for, until the printer says.
    pub fn light_shown(&self, now: i64) -> bool {
        match self.light_pending {
            Some((on, at)) if now - at < LIGHT_PENDING_SECONDS => on,
            _ => self.light.unwrap_or(false),
        }
    }

    pub fn light_refused(&self, now: i64) -> bool {
        self.light_refused_at
            .is_some_and(|at| now - at < LIGHT_REFUSED_SECONDS)
    }

    pub fn stop_armed(&self, now: i64) -> bool {
        self.stop_armed_until.is_some_and(|until| now < until)
    }

    /// Why a command cannot go to the printer right now, for a person.
    fn unreachable(&self) -> String {
        match &self.link {
            Link::Connecting | Link::Connected => "The printer is not connected yet.".into(),
            Link::Offline(_) => "The printer is not answering. It may be switched off.".into(),
            Link::Failed { why, .. } => format!("Niles could not connect to the printer: {why}"),
            Link::CertificateChanged { .. } => "The printer's certificate has changed, so Niles \
                stopped talking to it until someone trusts the new one on the 3D printer page."
                .into(),
        }
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.borrow_mut().as_mut().map(f))
}

impl Guest for BambuLab {
    fn init() -> Result<(), String> {
        let settings = Settings::read()?;
        register_devices(&settings)?;
        let mut app = App::new(settings);
        app.last_state = storage::get(LAST_STATE).map(|s| JobState::parse(&s));
        app.last_known = storage::get(LAST_KNOWN).and_then(|text| {
            let saved: Value = serde_json::from_str(&text).ok()?;
            let mut report = Report::default();
            report.merge(&saved);
            Some((report.snapshot(), saved.get("at")?.as_i64()?))
        });
        app.control_refused = storage::get(REFUSED).and_then(|at| at.parse().ok());
        connect(&mut app).map_err(|why| format!("Niles cannot connect to the printer: {why}"))?;
        timers::every(TICK, 60).map_err(|e| format!("Could not set a timer: {}", host(&e)))?;
        APP.with(|slot| *slot.borrow_mut() = Some(app));
        Ok(())
    }

    fn tools() -> Vec<ToolDef> {
        let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
        vec![
            ToolDef {
                name: "printer_status".into(),
                description: "The Bambu Lab 3D printer right now: what it is printing, \
                    whether it is printing, paused, finished or failed, percent done, \
                    layer, minutes left and the clock time it will be done, the nozzle, \
                    bed and chamber temperatures in degrees Celsius, the chamber light, \
                    and any error the printer reports. Use for \"how is the print going\", \
                    \"when will the print be done\", \"is the printer finished\", \"how hot \
                    is the nozzle\"."
                    .into(),
                schema: none.to_string(),
            },
            ToolDef {
                name: "pause_print".into(),
                description: "Pause the print the 3D printer is running. It can be resumed \
                    with bambulab_resume_print. Use for \"pause the print\", \"hold the printer\"."
                    .into(),
                schema: none.to_string(),
            },
            ToolDef {
                name: "resume_print".into(),
                description: "Resume a paused print on the 3D printer. Use for \"resume the \
                    print\", \"carry on printing\", \"unpause the printer\"."
                    .into(),
                schema: none.to_string(),
            },
            ToolDef {
                name: "stop_print".into(),
                description: "Stop the 3D printer's print for good. It cannot be resumed \
                    and the part is lost, so confirm with the user before calling, then \
                    call with confirm set to true. Use for \"stop the print\", \"cancel \
                    the print\", \"abort the printer\". To stop for a while, use bambulab_pause_print."
                    .into(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "confirm": {
                            "type": "boolean",
                            "description": "True once the user has confirmed stopping the print."
                        }
                    },
                    "additionalProperties": false
                })
                .to_string(),
            },
            ToolDef {
                name: "set_printer_light".into(),
                description: "Turn the light inside the 3D printer's chamber on or off. \
                    Use for \"turn on the printer light\", \"switch off the light in the \
                    printer\". The printer's camera and failure detection see better with \
                    it on during a print."
                    .into(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "on": {"type": "boolean", "description": "True for on, false for off."}
                    },
                    "required": ["on"],
                    "additionalProperties": false
                })
                .to_string(),
            },
        ]
    }

    fn call_tool(name: String, args_json: String) -> Result<String, String> {
        let args: Value = serde_json::from_str(&args_json).unwrap_or(Value::Null);
        let now = now();
        with_app(|app| match name.as_str() {
            "printer_status" => {
                if app.link != Link::Connected {
                    return Err(app.unreachable());
                }
                let snapshot = app.snapshot().unwrap_or_default();
                Ok(speech::status(&snapshot, now, &app.settings.zone).to_string())
            }
            "pause_print" => job_command(app, command::Job::Pause, Origin::Assistant)
                .map(|job| format!("Asked the printer to pause {job}.")),
            "resume_print" => job_command(app, command::Job::Resume, Origin::Assistant)
                .map(|job| format!("Asked the printer to resume {job}.")),
            "stop_print" => {
                if args.get("confirm").and_then(Value::as_bool) != Some(true) {
                    let job = app
                        .snapshot()
                        .and_then(|s| s.job)
                        .unwrap_or("the print".into());
                    return Err(format!(
                        "Not stopped. Stopping ends {job} for good and cannot be undone, so it \
                         needs confirm set to true."
                    ));
                }
                job_command(app, command::Job::Stop, Origin::Assistant)
                    .map(|job| format!("Asked the printer to stop {job}. It cannot be resumed."))
            }
            "set_printer_light" => {
                let on = args
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Say whether the light should be on or off.")?;
                switch_light(app, on, now)?;
                Ok(format!(
                    "Turning the chamber light {}.",
                    if on { "on" } else { "off" }
                ))
            }
            other => Err(format!("There is no tool called {other}.")),
        })
        .unwrap_or_else(|| Err("The printer is not set up.".into()))
    }

    fn set_device(id: String, desired: DeviceState) -> Result<(), String> {
        let now = now();
        with_app(|app| {
            if id != app.settings.light() {
                return Err("Only the printer's chamber light can be switched.".into());
            }
            // The chamber light only turns on and off; brightness and
            // colour are not something it does.
            match desired.on {
                Some(on) => switch_light(app, on, now),
                None => Ok(()),
            }
        })
        .unwrap_or_else(|| Err("The printer is not set up.".into()))
    }

    fn on_timer(id: String) {
        let now = now();
        with_app(|app| match id.as_str() {
            TICK => tick(app, now),
            RETRY => {
                if app.conn.is_none() {
                    log::log(log::Level::Info, "Trying the printer again.");
                    if let Err(why) = connect(app) {
                        failed(app, why, now);
                    }
                    redraw(app, now);
                }
            }
            REDRAW => {
                app.redraw_pending = false;
                app.redraw_last = now;
                invalidate();
            }
            CHECK => check_command(app, now),
            _ => {}
        });
    }

    fn on_event(_event: Event) {}

    fn on_mqtt_message(conn: u32, _topic: String, payload: Vec<u8>) {
        let Ok(message) = serde_json::from_slice::<Value>(&payload) else {
            return;
        };
        let now = now();
        with_app(|app| {
            if app.conn != Some(conn) {
                return;
            }
            app.last_message = Some(now);
            answers(app, &message, now);
            if let Some(on) = Report::light_in(&message) {
                app.light_pending = None;
                if app.light != Some(on) {
                    app.light = Some(on);
                    let mut state = empty_state();
                    state.on = Some(on);
                    if let Err(e) = devices::report_state(&app.settings.light(), &state) {
                        warn(&format!("Could not report the chamber light: {}", host(&e)));
                    }
                }
            }
            if app.report.merge(&message) {
                apply(app, now);
            }
            redraw(app, now);
        });
    }

    fn on_mqtt_status(conn: u32, status: ConnectionStatus) {
        let now = now();
        with_app(|app| {
            if app.conn != Some(conn) {
                return;
            }
            match status {
                ConnectionStatus::Connected => {
                    app.link = Link::Connected;
                    app.retry_delay = FIRST_RETRY;
                    set_available(app, true);
                    pushall(app, now);
                }
                ConnectionStatus::Disconnected(why) => {
                    app.link = Link::Offline(why);
                    set_available(app, false);
                }
                ConnectionStatus::Failed(why) => {
                    mqtt::disconnect(conn);
                    app.conn = None;
                    failed(app, why, now);
                }
                ConnectionStatus::CertificateChanged(change) => {
                    warn(&format!(
                        "The printer at {} shows a different certificate (key {}) from the one \
                         trusted before ({}). Nothing was sent to it. If the printer was updated \
                         or reset, trust the new one on the 3D printer page.",
                        app.settings.address, change.presented, change.trusted
                    ));
                    app.link = Link::CertificateChanged {
                        trusted: change.trusted,
                        presented: change.presented,
                    };
                    set_available(app, false);
                }
            }
            // The connection is news for the widget too: no waiting.
            app.redraw_last = 0;
            redraw(app, now);
        });
    }

    fn render(surface_id: String, _context: RenderContext) -> Result<UiTree, String> {
        let now = now();
        with_app(|app| match surface_id.as_str() {
            PAGE => Ok(view::page(app, now)),
            WIDGET => Ok(view::widget(app, now)),
            other => Err(format!("There is no page called {other}.")),
        })
        .unwrap_or_else(|| Err("The printer is not set up yet.".into()))
    }

    fn on_action(
        surface_id: String,
        control_id: String,
        value: ActionValue,
        _context: RenderContext,
    ) -> Result<ActionResult, String> {
        let now = now();
        let done = |toast: Option<&str>| {
            Ok(ActionResult {
                toast: toast.map(str::to_string),
                rerender: true,
            })
        };
        with_app(|app| match (surface_id.as_str(), control_id.as_str()) {
            (PAGE, "pause") => {
                job_command(app, command::Job::Pause, Origin::Page)?;
                done(Some("Pausing the print"))
            }
            (PAGE, "resume") => {
                job_command(app, command::Job::Resume, Origin::Page)?;
                done(Some("Resuming the print"))
            }
            (PAGE, "stop") => {
                app.stop_armed_until = Some(now + STOP_ARMED_SECONDS);
                done(None)
            }
            (PAGE, "stop_confirm") => {
                if !app.stop_armed(now) {
                    return Err("Press Stop again first.".into());
                }
                app.stop_armed_until = None;
                job_command(app, command::Job::Stop, Origin::Page)?;
                done(Some("Stopping the print"))
            }
            (PAGE, "stop_cancel") => {
                app.stop_armed_until = None;
                done(None)
            }
            (PAGE, "light") => {
                let ActionValue::Toggled(on) = value else {
                    return Err("The light is a switch.".into());
                };
                switch_light(app, on, now)?;
                done(None)
            }
            (PAGE, "trust_certificate") => {
                let Link::CertificateChanged { presented, .. } = &app.link else {
                    return Err("The certificate has not changed.".into());
                };
                let conn = app.conn.ok_or("There is no connection to trust it on.")?;
                log::log(
                    log::Level::Info,
                    &format!("Trusting the printer's new certificate ({presented}), as asked on the page."),
                );
                mqtt::accept_certificate(conn).map_err(|e| host(&e))?;
                app.link = Link::Connecting;
                app.redraw_last = 0;
                redraw(app, now);
                done(Some("Trusting the new certificate"))
            }
            (PAGE, "control_retry") => {
                app.control_refused = None;
                storage::delete(REFUSED);
                log::log(
                    log::Level::Info,
                    "Offering pause, resume and stop again, as asked on the page.",
                );
                app.redraw_last = 0;
                redraw(app, now);
                done(None)
            }
            (PAGE, "retry") => {
                if app.conn.is_some() {
                    return done(None);
                }
                timers::cancel(RETRY);
                connect(app)?;
                app.redraw_last = 0;
                redraw(app, now);
                done(Some("Connecting to the printer"))
            }
            _ => Err("That does nothing.".into()),
        })
        .unwrap_or_else(|| Err("The printer is not set up.".into()))
    }

    fn stop() {
        with_app(save_last_known);
    }
}

fn register_devices(settings: &Settings) -> Result<(), String> {
    let device = |id: String, class: DeviceClass, controllable: bool| Device {
        id,
        class,
        light: LightCapabilities {
            color_temp: false,
            rgb: false,
        },
        controllable,
        state: empty_state(),
    };
    for d in [
        device(settings.printer(), DeviceClass::Sensor, false),
        device(settings.light(), DeviceClass::Light, true),
    ] {
        devices::upsert(&d).map_err(|e| format!("Could not add {}: {}", d.id, host(&e)))?;
        // Unavailable until the printer answers.
        let _ = devices::set_available(&d.id, false);
    }
    // The camera and failure detection need the chamber light mid-print:
    // "turn off the office lights", scenes, presence and the light curve
    // leave it alone. By name it still switches.
    if let Err(e) = devices::set_room_light(&settings.light(), false) {
        warn(&format!(
            "Could not keep the chamber light out of the room's lights: {}",
            host(&e)
        ));
    }
    // Devices left from another room setting.
    let ours = settings.devices();
    for id in devices::registered() {
        if !ours.contains(&id) {
            let _ = devices::remove(&id);
        }
    }
    Ok(())
}

fn connect(app: &mut App) -> Result<(), String> {
    let conn = mqtt::connect(
        BROKER,
        &ConnectOptions {
            client_id: None,
            username: Some("bblp".into()),
            password: config::secret("access_code").map(|c| c.trim().to_string()),
            keep_alive: None,
            clean_session: None,
        },
    )
    .map_err(|e| host(&e))?;
    // Not latest-only: reports can be partial, and each one counts.
    if let Err(e) = mqtt::subscribe(conn, &app.settings.report_topic(), Qos::AtMostOnce, false) {
        mqtt::disconnect(conn);
        return Err(host(&e));
    }
    app.conn = Some(conn);
    app.link = Link::Connecting;
    Ok(())
}

fn failed(app: &mut App, why: String, now: i64) {
    let delay = app.retry_delay;
    app.retry_delay = (delay * 2).min(LAST_RETRY);
    warn(&format!(
        "Niles stopped trying the printer: {why}. Trying again in {} minutes.",
        delay / 60
    ));
    let _ = timers::after(RETRY, delay);
    app.link = Link::Failed {
        why,
        retry_at: now + i64::from(delay),
    };
    set_available(app, false);
}

fn set_available(app: &App, available: bool) {
    for id in app.settings.devices() {
        let _ = devices::set_available(&id, available);
    }
}

fn pushall(app: &mut App, now: i64) {
    app.last_pushall = now;
    if let Err(why) = publish(app, &command::pushall(), Qos::AtMostOnce) {
        warn(&format!(
            "Could not ask the printer for a full report: {why}"
        ));
    }
}

fn publish(app: &App, message: &Value, qos: Qos) -> Result<(), String> {
    let conn = match (app.conn, &app.link) {
        (Some(conn), Link::Connected) => conn,
        _ => return Err(app.unreachable()),
    };
    mqtt::publish(
        conn,
        &app.settings.request_topic(),
        message.to_string().as_bytes(),
        qos,
        false,
    )
    .map_err(|e| match e {
        HostError::Unavailable(_) => app.unreachable(),
        HostError::Limit(_) => "Too many commands at once. Try again in a moment.".into(),
        other => {
            warn(&format!("Could not send to the printer: {}", host(&other)));
            "Niles could not send that to the printer.".into()
        }
    })
}

/// Pause, resume or stop, if the print is in a state for it and the
/// printer has not refused before. Returns the job's name. Whether the
/// printer takes it is known only later: see [`answers`].
fn job_command(app: &mut App, which: command::Job, origin: Origin) -> Result<String, String> {
    if app.link != Link::Connected {
        return Err(app.unreachable());
    }
    let snapshot = app.snapshot().unwrap_or_default();
    let state = snapshot.state.clone().unwrap_or(JobState::Idle);
    let job = snapshot.job.clone().unwrap_or_else(|| "the print".into());
    match which {
        command::Job::Pause if state == JobState::Pause => {
            return Err(format!("{} is already paused.", capitalised(&job)));
        }
        command::Job::Pause | command::Job::Stop if !state.is_active() => {
            return Err("Nothing is printing.".into());
        }
        command::Job::Resume if state != JobState::Pause => {
            return Err(if state.is_active() {
                format!("{} is not paused.", capitalised(&job))
            } else {
                "Nothing is printing.".into()
            });
        }
        _ => {}
    }
    if app.control_refused.is_some() {
        return Err(CLOUD_MODE.into());
    }
    publish(app, &command::job(which), Qos::AtLeastOnce)?;
    log::log(
        log::Level::Info,
        &format!("Asked the printer to {} {job}.", which.verb()),
    );
    app.pending = Some(Pending {
        which,
        from: state,
        origin,
        accepted: false,
    });
    let _ = timers::after(CHECK, COMMAND_SECONDS);
    Ok(job)
}

/// The printer's answers to what was sent: a refusal on the report topic,
/// or the health message it raises for a command it could not verify.
fn answers(app: &mut App, message: &Value, now: i64) {
    if let Some(pending) = &mut app.pending {
        match model::reply_to(message, "print", pending.which.verb()) {
            Some(reply) if reply.accepted => {
                pending.accepted = true;
                if app.control_refused.take().is_some() {
                    storage::delete(REFUSED);
                    log::log(
                        log::Level::Info,
                        "The printer takes commands from Niles again.",
                    );
                }
            }
            Some(reply) => {
                let detail = format!(
                    "it answered {}{}",
                    reply.reason.as_deref().unwrap_or("that it failed"),
                    reply
                        .err_code
                        .map(|c| format!(" (error {c})"))
                        .unwrap_or_default()
                );
                refused(app, &detail, now);
                return;
            }
            None => {}
        }
        if model::verification_failed(message) {
            refused(
                app,
                "it reported HMS 0500_0500_0001_0007, command verification failed",
                now,
            );
            return;
        }
    }
    if app.light_pending.is_some()
        && let Some(reply) = model::reply_to(message, "system", "ledctrl")
        && !reply.accepted
    {
        warn(&format!(
            "The printer refused the chamber light command: {}.",
            reply.reason.as_deref().unwrap_or("no reason given")
        ));
        app.light_pending = None;
        app.light_refused_at = Some(now);
        app.redraw_last = 0;
    }
}

/// The printer did not take a pause, resume or stop: remember it, so the
/// page and the tools say so instead of offering it again, and tell
/// whoever asked.
fn refused(app: &mut App, detail: &str, now: i64) {
    let Some(pending) = app.pending.take() else {
        return;
    };
    timers::cancel(CHECK);
    warn(&format!(
        "The printer did not {} the print: {detail}. In cloud mode it takes control only from \
         Bambu's own apps; Developer mode (LAN-only) lifts that.",
        pending.which.verb()
    ));
    app.control_refused = Some(now);
    if let Err(e) = storage::set(REFUSED, &now.to_string()) {
        warn(&format!("Could not remember the refusal: {}", host(&e)));
    }
    if pending.origin == Origin::Assistant
        && let Err(e) = notify::say(CLOUD_MODE, notify::Priority::Normal, None)
    {
        warn(&format!(
            "Could not say that the printer refused: {}",
            host(&e)
        ));
    }
    app.redraw_last = 0;
    redraw(app, now);
}

/// A command's time is up: if the printer neither answered nor did it,
/// it did not take it.
fn check_command(app: &mut App, now: i64) {
    let Some(pending) = &app.pending else { return };
    let state = app
        .snapshot()
        .and_then(|s| s.state)
        .unwrap_or(JobState::Idle);
    if !pending.accepted && state == pending.from {
        refused(
            app,
            &format!("no answer and no change in {COMMAND_SECONDS} s"),
            now,
        );
    } else {
        app.pending = None;
    }
}

fn switch_light(app: &mut App, on: bool, now: i64) -> Result<(), String> {
    publish(app, &command::chamber_light(on), Qos::AtLeastOnce)?;
    app.light_pending = Some((on, now));
    Ok(())
}

/// After a report: actions, readings, storage.
fn apply(app: &mut App, now: i64) {
    let snapshot = app.report.snapshot();
    if let Some(state) = &snapshot.state
        && app.last_state.as_ref() != Some(state)
    {
        let action = model::transition(app.last_state.as_ref(), state, snapshot.print_error);
        app.last_state = Some(state.clone());
        // The command did what it asked; nothing to watch for.
        if app.pending.as_ref().is_some_and(|p| p.from != *state) {
            app.pending = None;
            timers::cancel(CHECK);
        }
        if let Err(e) = storage::set(LAST_STATE, state.code()) {
            warn(&format!(
                "Could not store the printer's state: {}",
                host(&e)
            ));
        }
        save_last_known(app);
        if !state.is_active() {
            app.stop_armed_until = None;
        }
        if let Some(action) = action {
            did(app, action, &snapshot, now);
        }
    }
    report_readings(app, &snapshot);
}

fn did(app: &mut App, action: Action, snapshot: &Snapshot, now: i64) {
    let job = snapshot.job.as_deref().unwrap_or("the print");
    log::log(
        log::Level::Info,
        &format!("The printer {}: {job}.", action.name()),
    );
    if let Err(e) = devices::report_action(&app.settings.printer(), action.name()) {
        warn(&format!("Could not report {}: {}", action.name(), host(&e)));
    }
    let (Some(room), Some(message)) = (
        app.settings.announce_room.clone(),
        speech::announcement(action, snapshot),
    ) else {
        return;
    };
    if now - app.last_announce < ANNOUNCE_GAP_SECONDS {
        log::log(
            log::Level::Info,
            &format!("Not announcing, too soon after the last: {message}"),
        );
        return;
    }
    match notify::say(&message, notify::Priority::Normal, Some(&room)) {
        Ok(()) => app.last_announce = now,
        Err(e) => warn(&format!("Could not announce \"{message}\": {}", host(&e))),
    }
}

/// Report the readings that changed since the last report.
fn report_readings(app: &mut App, s: &Snapshot) {
    let whole = |v: Option<f64>| v.map(f64::round);
    let readings: [(&'static str, Option<f64>, Option<&str>); 9] = [
        ("progress", s.percent, Some("%")),
        ("remaining", s.remaining_min, Some("min")),
        ("layer", s.layer, None),
        ("total_layers", s.total_layers, None),
        ("nozzle_temp", whole(s.nozzle), Some("°C")),
        ("nozzle_target", whole(s.nozzle_target), Some("°C")),
        ("bed_temp", whole(s.bed), Some("°C")),
        ("bed_target", whole(s.bed_target), Some("°C")),
        ("chamber_temp", whole(s.chamber), Some("°C")),
    ];
    let mut state = empty_state();
    for (name, value, unit) in readings {
        let Some(value) = value else { continue };
        if app.reported.get(name) == Some(&value) {
            continue;
        }
        app.reported.insert(name, value);
        state.readings.push(Reading {
            name: name.into(),
            value,
            unit: unit.map(str::to_string),
        });
    }
    if !state.readings.is_empty()
        && let Err(e) = devices::report_state(&app.settings.printer(), &state)
    {
        warn(&format!(
            "Could not report the printer's readings: {}",
            host(&e)
        ));
    }
}

fn tick(app: &mut App, now: i64) {
    if app.link == Link::Connected
        && now - app.last_message.unwrap_or(0) > QUIET_SECONDS
        && now - app.last_pushall > QUIET_SECONDS
    {
        pushall(app, now);
    }
    save_last_known(app);
    redraw(app, now);
}

/// Keep what the page needs to say where things stood, should the
/// printer be away after a restart. Written only when it changed.
fn save_last_known(app: &mut App) {
    let Some(s) = app.snapshot() else { return };
    let mut print = serde_json::Map::new();
    if let Some(state) = &s.state {
        print.insert("gcode_state".into(), state.code().into());
    }
    if let Some(job) = &s.job {
        print.insert("subtask_name".into(), job.as_str().into());
    }
    for (key, value) in [
        ("mc_percent", s.percent),
        ("mc_remaining_time", s.remaining_min),
        ("layer_num", s.layer),
        ("total_layer_num", s.total_layers),
        ("print_error", s.print_error.map(|e| e as f64)),
    ] {
        if let Some(v) = value {
            print.insert(key.into(), json!(v));
        }
    }
    let essentials = Value::Object(print);
    let key = essentials.to_string();
    if key == app.saved {
        return;
    }
    app.saved = key;
    let at = app.last_message.unwrap_or_else(now);
    let saved = json!({"print": essentials, "at": at});
    if let Err(e) = storage::set(LAST_KNOWN, &saved.to_string()) {
        warn(&format!(
            "Could not store the printer's last report: {}",
            host(&e)
        ));
    }
}

/// Redraw the page and widget when something they show changed, at
/// most every few seconds: the printer reports every second or so while
/// it prints, and temperatures wander.
fn redraw(app: &mut App, now: i64) {
    let s = app.snapshot().unwrap_or_default();
    let whole = |v: Option<f64>| v.map(|v| v.round() as i64);
    let key = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        app.link,
        s.state,
        s.job,
        s.percent,
        s.remaining_min,
        s.layer,
        app.light,
        (app.control_refused, app.light_refused(now)),
        s.print_error,
        s.hms,
        whole(s.nozzle),
        whole(s.bed),
        whole(s.chamber),
        whole(s.nozzle_target),
    );
    if key == app.redraw_key {
        return;
    }
    app.redraw_key = key;
    if now - app.redraw_last >= REDRAW_SECONDS {
        app.redraw_last = now;
        invalidate();
    } else if !app.redraw_pending {
        app.redraw_pending = true;
        let _ = timers::after(REDRAW, REDRAW_SECONDS as u32);
    }
}

fn invalidate() {
    ui::invalidate(PAGE);
    ui::invalidate(WIDGET);
}

fn text(key: &str) -> Option<String> {
    match config::get(key) {
        Some(config::SettingValue::Text(text)) if !text.trim().is_empty() => {
            Some(text.trim().to_string())
        }
        _ => None,
    }
}

fn host(e: &HostError) -> String {
    match e {
        HostError::NotPermitted(why)
        | HostError::Invalid(why)
        | HostError::Limit(why)
        | HostError::Unavailable(why) => why.clone(),
    }
}

fn warn(message: &str) {
    log::log(log::Level::Warn, message);
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// wit-bindgen does not derive `Default` for records.
fn empty_state() -> DeviceState {
    DeviceState {
        on: None,
        brightness: None,
        color_temp_kelvin: None,
        rgb: None,
        temperature_celsius: None,
        humidity_percent: None,
        battery_percent: None,
        open: None,
        motion: None,
        illuminance_lux: None,
        position: None,
        readings: Vec::new(),
    }
}
