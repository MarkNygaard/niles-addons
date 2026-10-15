//! A stand-in Bambu Lab X1C, for trying the plugin without a printer.
//!
//! It is what the plugin talks to on a real printer: an MQTT broker on
//! port 8883 with TLS and a self-signed certificate, user `bblp` and an
//! access code, with the printer itself on `device/<serial>/report` and
//! `device/<serial>/request`. It runs a print from PREPARE through
//! RUNNING to FINISH (or FAILED), sends partial reports as the state
//! changes, a full one when asked with `pushall`, and obeys pause,
//! resume, stop and the chamber light.
//!
//! `niles plugin dev` refuses loopback, so point the plugin at this
//! machine's LAN address, not 127.0.0.1. See the README.

use rumqttd::local::{LinkError, LinkRx, LinkTx};
use rumqttd::{
    Broker, Config, ConnectionSettings, Notification, RouterConfig, ServerSettings, TlsConfig,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const USAGE: &str = "\
fake-printer: a stand-in Bambu Lab X1C on this machine

  --bind ADDR          where to listen [0.0.0.0:8883]
  --serial SERIAL      the printer's serial number [00M09A350100001]
  --code CODE          the LAN access code [12345678]
  --job NAME           the print's name [Benchy]
  --start-after SECS   idle this long before printing [5]
  --speed SECS         seconds per percent of the print [1]
  --fail-at PERCENT    fail the print at this percent
  --repeat             start another print 20 s after one ends
  --full               send every field in every report, as an X1C does,
                       rather than only what changed, as a P1 does
  --cert-dir DIR       where the certificate is kept [.fake-printer]
  --new-cert           make a new certificate, as a firmware update can";

struct Args {
    bind: SocketAddr,
    serial: String,
    code: String,
    job: String,
    start_after: u64,
    speed: f64,
    fail_at: Option<f64>,
    repeat: bool,
    full: bool,
    cert_dir: PathBuf,
    new_cert: bool,
}

fn args() -> Result<Args, String> {
    let mut args = Args {
        bind: "0.0.0.0:8883".parse().unwrap(),
        serial: "00M09A350100001".into(),
        code: "12345678".into(),
        job: "Benchy".into(),
        start_after: 5,
        speed: 1.0,
        fail_at: None,
        repeat: false,
        full: false,
        cert_dir: ".fake-printer".into(),
        new_cert: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        let number = |text: String| text.parse::<f64>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--bind" => args.bind = value()?.parse().map_err(|e| format!("--bind: {e}"))?,
            "--serial" => args.serial = value()?,
            "--code" => args.code = value()?,
            "--job" => args.job = value()?,
            "--start-after" => args.start_after = number(value()?)? as u64,
            "--speed" => args.speed = number(value()?)?.max(0.05),
            "--fail-at" => args.fail_at = Some(number(value()?)?),
            "--repeat" => args.repeat = true,
            "--full" => args.full = true,
            "--cert-dir" => args.cert_dir = value()?.into(),
            "--new-cert" => args.new_cert = true,
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown flag {other}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

fn main() {
    let args = match args() {
        Ok(args) => args,
        Err(why) => {
            eprintln!("{why}");
            std::process::exit(2);
        }
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (cert, key) = certificate(&args).unwrap_or_else(|e| {
        eprintln!("certificate: {e}");
        std::process::exit(1);
    });

    let mut broker = Broker::new(config(&args, cert, key));
    let (tx, rx) = broker.link("printer").expect("a link into the broker");
    std::thread::spawn(move || {
        if let Err(e) = broker.start() {
            eprintln!("broker: {e}");
            std::process::exit(1);
        }
    });
    println!(
        "Fake X1C {} on {} (user bblp, access code {}). Ctrl-C to stop.",
        args.serial, args.bind, args.code
    );
    Printer::new(args, tx, rx).run();
}

/// The certificate and key, as PEM files, kept between runs so the
/// plugin's pin holds; `--new-cert` replaces them.
fn certificate(args: &Args) -> Result<(PathBuf, PathBuf), String> {
    let cert = args.cert_dir.join("cert.pem");
    let key = args.cert_dir.join("key.pem");
    if args.new_cert || !cert.is_file() || !key.is_file() {
        std::fs::create_dir_all(&args.cert_dir).map_err(|e| e.to_string())?;
        let made = rcgen::generate_simple_self_signed(vec![args.serial.clone()])
            .map_err(|e| e.to_string())?;
        std::fs::write(&cert, made.cert.pem()).map_err(|e| e.to_string())?;
        std::fs::write(&key, made.signing_key.serialize_pem()).map_err(|e| e.to_string())?;
        println!("A new certificate in {}", args.cert_dir.display());
    }
    Ok((cert, key))
}

fn config(args: &Args, cert: PathBuf, key: PathBuf) -> Config {
    let server = ServerSettings {
        name: "printer".into(),
        listen: args.bind,
        tls: Some(TlsConfig::Rustls {
            capath: None,
            certpath: cert.display().to_string(),
            keypath: key.display().to_string(),
        }),
        next_connection_delay_ms: 1,
        connections: ConnectionSettings {
            connection_timeout_ms: 60_000,
            max_payload_size: 1024 * 1024,
            max_inflight_count: 100,
            auth: Some(HashMap::from([("bblp".to_string(), args.code.clone())])),
            external_auth: None,
            dynamic_filters: true,
        },
    };
    Config {
        id: 0,
        router: RouterConfig {
            max_connections: 10,
            max_outgoing_packet_count: 200,
            max_segment_size: 10 * 1024 * 1024,
            max_segment_count: 10,
            custom_segment: None,
            initialized_filters: None,
            shared_subscriptions_strategy: Default::default(),
        },
        v4: Some(HashMap::from([("printer".to_string(), server)])),
        v5: None,
        ws: None,
        cluster: None,
        console: None,
        bridge: None,
        prometheus: None,
        metrics: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Prepare,
    Running,
    Pause,
    Finish,
    Failed,
}

impl State {
    fn code(self) -> &'static str {
        match self {
            State::Idle => "IDLE",
            State::Prepare => "PREPARE",
            State::Running => "RUNNING",
            State::Pause => "PAUSE",
            State::Finish => "FINISH",
            State::Failed => "FAILED",
        }
    }

    fn active(self) -> bool {
        matches!(self, State::Prepare | State::Running | State::Pause)
    }
}

const TOTAL_LAYERS: f64 = 210.0;
/// Minutes the whole print is meant to take, for `mc_remaining_time`.
const PRINT_MINUTES: f64 = 38.0;
const STOPPED: u64 = 0x0300_400C;

struct Printer {
    args: Args,
    tx: LinkTx,
    rx: LinkRx,
    state: State,
    /// Seconds in the current state.
    since: f64,
    percent: f64,
    nozzle: f64,
    nozzle_target: f64,
    bed: f64,
    bed_target: f64,
    chamber: f64,
    light: bool,
    print_error: u64,
    hms: Vec<Value>,
    job: String,
    /// What the last report said, to send only what changed.
    sent: Map<String, Value>,
    sequence: u64,
    ticks: u64,
}

impl Printer {
    fn new(args: Args, tx: LinkTx, rx: LinkRx) -> Printer {
        Printer {
            args,
            tx,
            rx,
            state: State::Idle,
            since: 0.0,
            percent: 0.0,
            nozzle: 24.0,
            nozzle_target: 0.0,
            bed: 23.0,
            bed_target: 0.0,
            chamber: 25.0,
            light: false,
            print_error: 0,
            hms: Vec::new(),
            job: String::new(),
            sent: Map::new(),
            sequence: 0,
            ticks: 0,
        }
    }

    fn report_topic(&self) -> String {
        format!("device/{}/report", self.args.serial)
    }

    fn run(mut self) {
        self.tx
            .subscribe(format!("device/{}/request", self.args.serial))
            .expect("subscribe to requests");
        let mut next_tick = Instant::now() + Duration::from_secs(1);
        loop {
            match self.rx.recv_deadline(next_tick) {
                Ok(Some(Notification::Forward(forward))) => {
                    let payload = forward.publish.payload.to_vec();
                    self.request(&payload);
                }
                Ok(_) => {}
                Err(LinkError::RecvTimeout(_)) => {}
                Err(e) => {
                    eprintln!("link: {e}");
                    return;
                }
            }
            if Instant::now() >= next_tick {
                next_tick += Duration::from_secs(1);
                self.tick();
                let full = self.args.full;
                self.report(full);
            }
        }
    }

    fn go(&mut self, state: State) {
        println!("{} -> {}", self.state.code(), state.code());
        self.state = state;
        self.since = 0.0;
    }

    fn start(&mut self) {
        self.job = format!("{}.3mf", self.args.job);
        self.percent = 0.0;
        self.print_error = 0;
        self.hms.clear();
        self.nozzle_target = 220.0;
        self.bed_target = 55.0;
        self.light = true;
        self.go(State::Prepare);
    }

    fn end(&mut self, state: State) {
        self.nozzle_target = 0.0;
        self.bed_target = 0.0;
        self.go(state);
    }

    fn tick(&mut self) {
        self.ticks += 1;
        self.since += 1.0;
        let wobble = ((self.ticks * 7919) % 11) as f64 / 20.0 - 0.25;
        let toward = |now: f64, target: f64, rate: f64| {
            let goal = if target > 0.0 { target } else { 24.0 };
            now + (goal - now) * rate
        };
        self.nozzle = toward(self.nozzle, self.nozzle_target, 0.3) + wobble;
        self.bed = toward(self.bed, self.bed_target, 0.2) + wobble / 2.0;
        let chamber_goal = if self.bed_target > 0.0 { 38.0 } else { 25.0 };
        self.chamber += (chamber_goal - self.chamber) * 0.02;
        match self.state {
            State::Idle if self.since >= self.args.start_after as f64 => self.start(),
            State::Prepare
                if self.since >= 6.0
                    && (self.nozzle - self.nozzle_target).abs() < 4.0
                    && (self.bed - self.bed_target).abs() < 3.0 =>
            {
                self.go(State::Running)
            }
            State::Running => {
                self.percent = (self.percent + 1.0 / self.args.speed).min(100.0);
                if self.args.fail_at.is_some_and(|at| self.percent >= at) {
                    self.print_error = 0x0300_8003;
                    self.hms = vec![json!({"attr": 0x0C00_0300u32, "code": 0x0002_0001u32})];
                    self.end(State::Failed);
                } else if self.percent >= 100.0 {
                    self.end(State::Finish);
                }
            }
            State::Finish | State::Failed if self.args.repeat && self.since >= 20.0 => {
                self.go(State::Idle)
            }
            _ => {}
        }
    }

    /// Every field, the way an X1C sends them, with some it sends that
    /// the plugin has no use for.
    fn fields(&self) -> Map<String, Value> {
        let remaining = ((100.0 - self.percent) / 100.0 * PRINT_MINUTES).ceil();
        let fan = |on: bool, steps: u32| if on { steps.to_string() } else { "0".into() };
        let printing = self.state == State::Running;
        let print = json!({
            "gcode_state": self.state.code(),
            "subtask_name": self.job,
            "gcode_file": if self.job.is_empty() { String::new() } else { "/data/Metadata/plate_1.gcode".into() },
            "mc_percent": self.percent.floor() as u64,
            "mc_remaining_time": if self.state.active() { remaining as u64 } else { 0 },
            "layer_num": (self.percent / 100.0 * TOTAL_LAYERS).floor() as u64,
            "total_layer_num": if self.job.is_empty() { 0 } else { TOTAL_LAYERS as u64 },
            "nozzle_temper": round(self.nozzle),
            "nozzle_target_temper": self.nozzle_target,
            "bed_temper": round(self.bed),
            "bed_target_temper": self.bed_target,
            "chamber_temper": round(self.chamber),
            "print_error": self.print_error,
            "hms": self.hms,
            "lights_report": [
                {"node": "chamber_light", "mode": if self.light { "on" } else { "off" }},
                {"node": "work_light", "mode": "flashing"}
            ],
            "cooling_fan_speed": fan(printing, 15),
            "big_fan1_speed": fan(printing, 10),
            "big_fan2_speed": fan(self.bed_target > 0.0, 6),
            "heatbreak_fan_speed": fan(self.nozzle > 50.0, 15),
            "spd_lvl": 2,
            "stg_cur": if printing { 0 } else { -1 },
            "print_type": if self.job.is_empty() { "idle" } else { "local" },
            "wifi_signal": "-52dBm",
            "ams": {"ams": [], "tray_now": "255"},
            "ipcam": {"ipcam_dev": "1", "timelapse": "disable"},
            "upgrade_state": {"status": "IDLE", "new_version_state": 2},
            "some_field_from_newer_firmware": {"nested": [1, 2, 3]}
        });
        print.as_object().unwrap().clone()
    }

    fn report(&mut self, full: bool) {
        let fields = self.fields();
        let changed: Map<String, Value> = if full {
            fields.clone()
        } else {
            fields
                .iter()
                .filter(|(k, v)| self.sent.get(*k) != Some(v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        self.sent = fields;
        if changed.is_empty() {
            return;
        }
        let mut print = changed;
        self.sequence += 1;
        print.insert("command".into(), "push_status".into());
        print.insert("msg".into(), json!(if full { 0 } else { 1 }));
        print.insert("sequence_id".into(), self.sequence.to_string().into());
        self.publish(json!({ "print": print }));
        // Now and then something the plugin has never heard of.
        if self.ticks.is_multiple_of(15) {
            self.publish(json!({"mc_print": {"command": "push_info", "param": "[BBL] fake log line", "sequence_id": "0"}}));
        }
    }

    fn publish(&mut self, message: Value) {
        let topic = self.report_topic();
        if let Err(e) = self.tx.publish(topic, message.to_string().into_bytes()) {
            eprintln!("publish: {e}");
        }
    }

    fn request(&mut self, payload: &[u8]) {
        let Ok(message) = serde_json::from_slice::<Value>(payload) else {
            println!(
                "request that is not JSON: {}",
                String::from_utf8_lossy(payload)
            );
            return;
        };
        println!("request {message}");
        let command = |section: &str| {
            message
                .get(section)
                .and_then(|s| s.get("command"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        if command("pushing").as_deref() == Some("pushall") {
            self.report(true);
            return;
        }
        if let Some(command) = command("print") {
            let ok = match command.as_str() {
                "pause" if matches!(self.state, State::Prepare | State::Running) => {
                    self.go(State::Pause);
                    true
                }
                "resume" if self.state == State::Pause => {
                    self.go(State::Running);
                    true
                }
                "stop" if self.state.active() => {
                    self.print_error = STOPPED;
                    self.end(State::Failed);
                    true
                }
                _ => false,
            };
            let result = if ok { "success" } else { "failed" };
            self.publish(json!({"print": {"command": command, "param": "", "reason": result, "result": result, "sequence_id": "0"}}));
            self.report(false);
            return;
        }
        if command("system").as_deref() == Some("ledctrl") {
            let system = &message["system"];
            let mut answer = system.clone();
            if system["led_node"] == "chamber_light" {
                self.light = system["led_mode"] != "off";
                answer["result"] = "success".into();
            } else {
                answer["result"] = "failed".into();
            }
            self.publish(json!({ "system": answer }));
            self.report(false);
            return;
        }
        println!("  (not something this fake printer does)");
    }
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
