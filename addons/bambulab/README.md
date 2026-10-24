# Bambu Lab for Niles

A [Niles](https://github.com/MarkNygaard/niles) integration for a Bambu Lab
X1C 3D printer, over the printer's own MQTT broker on your home network. No
Bambu cloud account is involved.

With it you can:

- **ask** how the print is going, when it will be done (as a clock time),
  and how hot the nozzle, bed and chamber are;
- **pause, resume and stop** a print, and switch the chamber light, by voice,
  from the chat assistants or from the app;
- **write rules** on the printer: "when the print finishes, turn on the
  office lamp", "when the nozzle cools below 50 degrees, switch off the
  printer's plug";
- **hear** when a print finishes or fails, in a room you choose;
- **see** the job, progress, time left, temperatures and any errors on a
  "3D printer" page in the app's menu, and a small card on the front page or
  a room.

It is written for the X1C and should work with other Bambu Lab printers that
offer LAN mode (X1, X1E, P1P, P1S); only the X1C reports a chamber
temperature.

## Setting up the printer

1. **Note the access code.** The eight characters under Settings → Network
   (WLAN) on the printer's screen. It changes if you refresh it there, and
   then Niles needs the new one.
2. **Note the serial number.** Settings → Device on the printer's screen, or
   Bambu Studio → Device. An X1C's starts with `00M`.
3. **Give the printer a fixed address** in your router, and note it.
4. **Decide between cloud mode and LAN-only mode**, below. Nothing to change
   if you keep cloud mode.

## Cloud mode or LAN-only mode?

Since Bambu's Authorization Control firmware (X1C 01.08.05.00, January 2025),
a printer in its normal **cloud mode** lets anything on your network with the
access code watch it, but takes control commands only from Bambu's own apps.

| | Cloud mode (normal) | LAN Only + Developer mode |
|---|---|---|
| Bambu Handy, cloud printing | yes | no |
| Status, progress, temperatures, the page and card | yes | yes |
| Rules on readings and on `finished`, `failed`… | yes | yes |
| "The print is done" announcements | yes | yes |
| Chamber light from Niles | yes, as far as is known | yes |
| Pause, resume and stop from Niles | **no**: the printer refuses them | yes |

So most households can keep cloud mode and the Bambu app, and pause or stop
prints from the app. The first time the printer refuses a pause, resume or
stop, Niles says so plainly ("The printer didn't accept that: in cloud mode
Bambu only lets its own apps control it…"), aloud if you asked by voice, and
from then on the 3D printer page shows a short note instead of the Pause,
Resume and Stop buttons, and the assistant answers the same sentence rather
than trying again. The printer also shows "MQTT command verification failed"
(HMS 0500_0500_0001_0007) on its screen for each refused command, which is
why Niles does not keep trying.

To control prints from Niles, turn on **LAN Only mode** and then **Developer
mode** on the printer (Settings → Network (WLAN)). The printer then stops
talking to Bambu's cloud, so Bambu Handy stops working; Bambu Studio and
OrcaSlicer still find it on the network. Then press **Try the controls again**
on the 3D printer page.

## Installing

In the Niles app: Integrations → Available → **Add from GitHub** (at the end of
the page), paste `https://github.com/MarkNygaard/niles-bambulab` and press
**Preview**. The approval screen says what it may do:

- Connect to the MQTT broker at the address you enter under Printer address,
  on your home network, trusting the certificate it shows the first time
- Add devices to your home. They appear in rooms, in rules and by voice.
- Offer 5 tools to the assistant
- Speak through your satellites (for announcements, if you pick a room, and
  to say when the printer refuses a command you gave by voice)
- Adds a page to the menu: 3D printer; adds a card to the front page: 3D printer

Then fill in its settings:

| Setting | What to enter |
|---|---|
| Printer address | The printer's IP address, such as `192.168.1.40`. |
| Serial number | From step 4. |
| LAN access code | From step 3. Kept in Niles's encrypted secret store. |
| Room | Where the printer stands. Its devices join this room. |
| Say when a print is done in | A room for "the 3D printer has finished Benchy". Empty for silence. |

The card starts in the printer's room; move it from the plugin's panel on the
Integrations page. Clock times ("done around 15:05") are in Niles's own time
zone. It needs Niles 0.2.1 or later.

## What you get

### Devices

In the room you chose, say `office`:

| Device | Class | What it is |
|---|---|---|
| `office/printer` | sensor | Readings: `progress` (%), `remaining` (min), `layer`, `total_layers`, `nozzle_temp`, `nozzle_target`, `bed_temp`, `bed_target`, `chamber_temp` (°C). Actions, when the print does something: `started`, `paused`, `resumed`, `finished`, `failed` and `cancelled` (stopped by somebody). |
| `office/printer_light` | light | The chamber light, on and off. |

The chamber light is not one of the room's lights: "turn off the office
lights", scenes, presence and the daily light curve leave it alone, because
the printer's camera and failure detection need it during a print. "Turn off
the printer light" and rules that name it still switch it.

### Things to say

- "How's the print going?" / "When will the printer be done?"
- "How hot is the nozzle?"
- "Pause the print." / "Resume the print." (LAN-only and Developer mode)
- "Stop the print." The assistant asks first: stopping cannot be undone.
- "Turn on the printer light."

### Rules

By voice ("when the printer reports finished, turn on the office lamp"), in
the Automations editor, or as Niles stores them. Niles reads the first back
as *When the office printer reports finished, turn on the office lamp.*

```json
{
  "description": "Lamp on when a print is done",
  "triggers": [{ "type": "device_action", "device": "office/printer", "action": "finished" }],
  "actions": [{ "do": "set_device", "device": "office/lamp", "on": true }]
}
```

```json
{
  "description": "Printer plug off once it has cooled after a print",
  "triggers": [{
    "type": "numeric", "device": "office/printer", "measure": "nozzle_temp",
    "below": 50, "for": { "minutes": 10 }
  }],
  "conditions": [],
  "actions": [{ "do": "set_device", "device": "office/printer_plug", "on": false }]
}
```

### The page

**3D printer** in the menu shows the job, a status badge, progress, time left
and when it will be done, the layer, the temperatures with their targets,
Pause or Resume and Stop (Stop asks again before it stops), the chamber light,
fan speeds, and any error codes the printer reports. When the printer is off
or unreachable it says so, with the last thing it heard.

### When the printer's certificate changes

The printer's MQTT broker uses a certificate signed by Bambu Lab rather than a
public authority, so Niles cannot check it the way a browser checks a website.
Instead it remembers the certificate's key the first time it connects and from
then on talks only to a printer showing the same key. A firmware update or a
factory reset can give the printer a new key; so would another device
answering at the printer's address. When that happens Niles sends the printer
nothing, not even the access code, and the 3D printer page says so, with a
**Trust the new certificate** button. Press it if you have just updated or
reset the printer. The plugin never presses it for you.

## What it does not do

- The camera, the AMS, starting prints and uploading files: those are not
  MQTT, or need more than a plugin can do today (Niles plugins have no raw TCP
  or FTP).
- Bambu's cloud: only LAN mode.

## Developing

You need Rust 1.85 or later, `rustup target add wasm32-wasip2`, and the
`niles` binary (`cargo install --locked --git https://github.com/MarkNygaard/niles niles-bin`,
or `cargo run -p niles-bin --` from a Niles checkout).

```sh
./build.sh                     # or .\build.ps1: builds integration.wasm
cargo test                     # the plugin's own tests, natively
niles plugin check .           # what an install would check
```

`build.sh` copies the component to `integration.wasm`, which `niles plugin
dev .` and `niles plugin check .` prefer over anything under `target/`, so
rebuild with it rather than with a bare `cargo build`.

### Trying it without a printer

`fake-printer/` is a stand-in X1C: an MQTT broker with TLS and a self-signed
certificate, user `bblp` and an access code, that runs a print from PREPARE
through RUNNING to FINISH with partial reports, answers `pushall`, and obeys
pause, resume, stop and the chamber light.

```sh
cargo run -p fake-printer -- --speed 0.5      # a print in about a minute
cargo run -p fake-printer -- --cloud-mode     # refuses pause, resume and stop
cargo run -p fake-printer -- --help           # failing, repeating, a new certificate…
```

It listens on `127.0.0.1:8883`. An installed plugin can never reach
loopback, so `niles plugin dev` needs `--allow-loopback` to let it. Copy
`dev-settings.example.toml` to `dev-settings.toml`, and:

```sh
niles plugin dev . --allow-loopback
niles plugin dev . --allow-loopback --dry-run --call printer_status '{}' --render printer
niles plugin dev . --allow-loopback --time-zone America/New_York
```

`--call` and `--render` wait for the printer's first MQTT status. At the
prompt:

```text
> tool printer_status
> render printer
> render printer_now
> tool pause_print
> action printer stop
> action printer stop_confirm
> set office/printer_light {"on":false}
> action printer light {"type":"toggled","value":true}
```

To see the certificate banner, stop the fake printer and start it again with
`--new-cert`, then `render printer` and `action printer trust_certificate`.
The trusted key is in `.niles-dev/storage.json`; delete that file to start
afresh.

### The interface

`wit/niles-integration.wit` is a copy of
[`schemas/niles-integration.wit`](https://github.com/MarkNygaard/niles/blob/main/schemas/niles-integration.wit)
from Niles, at interface 0.2.1. To move to a newer interface, copy the new
file over it, set `interface` (and `min_niles`, if it needs a newer Niles) in
`niles-integration.toml`, rebuild, and fix what the compiler says. A patch
version of the interface (0.2.x) only adds; this plugin uses 0.2.1's
`config.time-zone` and `devices.set-room-light`, so it sets `min_niles =
"0.2.1"`.

### Releasing

Bump `version` in `niles-integration.toml`, commit, and push a tag with the
same version: `git tag v0.1.0 && git push origin v0.1.0`. The workflow in
`.github/workflows/release.yml` builds the component and publishes a release
with `niles-integration.toml`, `integration.wasm` and `logo.svg`.

## Thanks

The logo is Bambu Lab's mark from [Simple Icons](https://simpleicons.org/)
(CC0).

The report fields and commands follow what the open-source
[ha-bambulab](https://github.com/greghesp/ha-bambulab) integration for Home
Assistant and the [OpenBambuAPI](https://github.com/Doridian/OpenBambuAPI)
notes document. Bambu Lab is a trademark of its owner; this project is not
affiliated with it.

## License

MIT
