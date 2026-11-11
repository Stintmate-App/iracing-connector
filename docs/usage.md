# Usage

The connector is three programs. Stintmate starts and supervises them; you can also run the
control app by hand.

| Program | What it does |
|---|---|
| `iracing-connector-ui.exe` | The control app: a tray icon, the supervision of the recorder, the connector's own endpoint (`connector-iracing`) and the registration. |
| `iracing-connector-window.exe` | The window with the Services, Options and Logs pages. The control app starts it on demand; one window at most. |
| `iracing-recorder.exe` | The recorder, started hidden by the control app. It reads the simulator, records through Pitwall and answers Pitwall's live requests. |

## What the recorder does today

- It connects to Pitwall, says hello as connector `iracing` and publishes the status
  `connector.status` whenever it changes (it looks twice a second).
- `game_running` is true while the `iRacingSim64DX11.exe` process exists. The check opens the
  simulator's memory map by name without mapping it, and takes a process snapshot when the map
  appears, and at least every 60 seconds.
- `phase` is `null` while the simulator is not running, `"menus"` while it runs and the player
  is not in a car, `"garage"` in the garage and `"on_track"` while recording. The message is
  "Waiting for iRacing", "iRacing is running: not in a car", "In the garage", "Recording" or
  "Replay: not recording". `live` is true while a session is open and `telemetry` while Pitwall
  is subscribed to the sample stream.
- It records sessions, laps, setups, pit exits, the tyre readings taken at stops (simulator data, kind `stall_tyres`) and the other
  cars' laps (see below). Each lap
  carries `x` and `z`, the car's position in metres (east and north, origin at the
  start/finish line) by dead reckoning; Pitwall draws the track outline and the track maps
  from them. The position is approximate while driving: within about 3 m on the laps checked
  (see [FINDINGS.md](FINDINGS.md#position-by-dead-reckoning)); exact positions replace it after
  the session for the laps iRacing logged to disk (see "Exact positions from disk telemetry").
- It stops when the control app sets its stop event, and exits 0.

### What Pitwall can ask of it

The recorder registers these before it connects, and answers on its own threads: the sampling
thread only copies the row into preallocated buffers.

| Request | Answer |
|---|---|
| `connector.live_frame` | The live frame built from the last row: `phase` `"on_track"`, `"garage"`, `"menus"` or `"paused"` (no new row for one second), with the session, track, car, lap, splits, conditions and participants. The error `game_not_running` while the simulator is not running or no row arrived. |
| `connector.capture_target` | `{hwnd, pid, title}` of the largest visible window of `iRacingSim64DX11.exe`; failing that, a window titled `iRacing.com Simulator` of class `SimWinClass` (observed on the simulator). The errors `game_not_running` (no such window) and `window_minimized`. |
| `connector.telemetry` (stream) | One 32-byte sample per row, declared at 60 Hz. Nothing is produced while nobody is subscribed. The subscription option `rate_hz` (10 to 200) lowers the cap, a value above 60 gives 60, a value out of range is refused with `bad_request`. With no new row for 0.25 s the last sample is sent again with `FRESH` clear; its `game_state` says paused after a second, or not running once the simulator is gone. |
| `connector.action` | `test.ping` (answers `pong` and says the radio check; `say: false` only answers) and `test.announce` (sends one announcement of the connector's catalog; a bad code, parameter or variant is `bad_request`). Any other code is `unknown_action`. |

At every connection it also declares to Pitwall its announcement catalog (`connector.iracing`),
its guidance for the engineer (at most 16 items) and the two actions. The features it declares
are `window`, `notices`, `translated_texts`, `remembered_services` and `options` (the five the
control app provides), then `setups`, `fuel`, `sectors`, `conditions`, `rivals`, `live_frame`,
`lap_status`, `capture_target`, `brake_cues`, `guidance`, `actions`, `announcements`, `spotter` and `sim_data`.

Simulator data: the tyre temperatures and tread remaining iRacing gives only when the car comes into the
pits (a pit stop, a reset to the pits) are sent as `sim_data` records of the kind `stall_tyres`, one per new
set (a set is new when its values differ from the last one sent in the session). The readings are of the set
that came in, just before it was changed: they describe the stint that ended, and the car leaves on new
tyres. Getting into the car writes a record with occasion `car_entry` every time, with no `wheels`: it marks
the start of a stint (the readings shown then only repeat the last set that came in). A record carries
`occasion` (`pit_stop`, `reset` or `car_entry`) and, except for `car_entry`, `wheels` (`fl`, `fr`, `rl`, `rr`), each with the carcass
temperatures `carcass_in_c`, `carcass_mid_c`, `carcass_out_c` in degrees C and the tread remaining `tread_in`,
`tread_mid`, `tread_out` as a fraction (1 is new); `in` is the edge towards the car's centreline. A value out
of range is left out. At every connection the connector also sends a `sim_data_kinds` message that describes
each field, so a reader needs no other source. Both messages need a Pitwall that implements simulator data;
an older one refuses them and the connector drops them.

While a session is open it announces flags, cautions, penalties, the finish, other cars'
retirements, incidents and the spotter's calls through `pitwall.announce`. At most 10 messages in
any 60 seconds go out, and 120 of the spotter's; one over its limit is dropped, not delayed.

## Exact positions from disk telemetry

While you drive, each lap's `x` and `z` are dead-reckoned from speed and yaw, accurate to about
2 to 3 metres. iRacing can also log its own telemetry to disk (`.ibt` files in
`Documents\iRacing\telemetry`), which holds the car's latitude and longitude. When it did, the
connector replaces the lap's positions with exact ones after the session, so track maps and
traces get smoother.

- **What it needs.** Disk telemetry is your setting in iRacing: Alt+L starts logging (each time
  you get into the car a new file starts), or its option to log automatically. The connector never
  changes that setting. Laps that iRacing did not log, including the part of a lap before a file
  started, keep their dead-reckoned positions. With logging off there is nothing to do.
- **What it needs from Pitwall.** A Pitwall that implements `lap_positions`; an older one refuses
  the message and the connector drops the write.
- **When it runs.** After a session has ended and while no session is open, on a low-priority
  thread of the recorder that wakes every 30 seconds. It waits until the files of the session are
  complete and have not changed for a minute, and stops at once if you get back into a car. It
  never runs while you drive.
- **What it does to your files.** It only reads: it never writes, moves or deletes anything in
  the iRacing folder.
- **What it logs.** One line per session in `recorder.log`, for example
  `Exact positions from disk telemetry: 12 of 14 laps of session <key> (2 left with their recorded positions)`;
  the reason for each lap that was left is in the log at debug level.

## Options

The control app's command line is implemented by the shared connector runtime this repository
pins (`Cargo.toml`, tag `v0.1.3`; `control/src/cli.rs` there):

| Option | Meaning |
|---|---|
| `--tray` | Start hidden in the tray. |
| `--workspace <dir>`, `--allow-synced` | The Stintmate workspace to use, and permission to use one inside OneDrive. Without `--workspace` the folder comes from `STINTMATE_WORKSPACE`, then from `%APPDATA%\Stintmate\settings.json`. |
| `--register [--flavour <name>] [--uninstall <exe>]` | Write the descriptor `%LOCALAPPDATA%\Stintmate\connectors\iracing.json` and exit. It needs no workspace. It refuses (exit 1) a descriptor of another product. |
| `--unregister` | Remove the descriptor when it names this install; logs, outbox and settings stay. |
| `--read-registration <out-file>` | Write the registered launcher and uninstaller as `key=value` lines; exit 2 when there is none. |
| `--quit` | Ask the running control app to exit. |
| `--managed --parent-pid <pid>` | Run as a child of Stintmate (each needs the other; a missing one is exit 2). |
| `--stintmate-hold [--purpose update\|uninstall] [--wait <seconds>]` | Ask the Stintmate hub to stop and hold this connector, for an update or an uninstall. |
| `--stintmate-release [--no-start]` | Ask the hub to end the hold and start the connector, or only end the hold with `--no-start`. |
| `--from-shortcut` | What the Start menu shortcut runs: open Stintmate, or say it is needed. |
| `--version` | Print `iracing-connector-ui <version>` and exit. |

An option the control app does not know is ignored and logged once, so a newer Stintmate can
pass one. Exit status: 0 done, 1 failed, 2 bad arguments, 3 workspace error, 4 managed by
Stintmate (`--quit`).

The window accepts `--version` (prints `iracing-connector-window <version>`) and the workspace
options. The recorder accepts:

| Option | Meaning |
|---|---|
| `--version` | Print `iracing-recorder <version>` and exit. |
| `--workspace <dir>`, `--allow-synced` | As for the control app. |
| `--hz <rate>` | A positive, finite number. The runtime takes it (the connector description's `recorder_hz`, 60, when absent); this recorder's loop does not read it and follows the simulator's rows. |
| `--pitwall-pipe <name>`, `--control-pipe <name>` | Replace Pitwall's and the control app's endpoints (tests). |
| `--sdk-map <name>` with `--sdk-event <name>` | Both or neither (`bins/iracing-recorder/src/cli.rs`): the recorder opens that memory mapping and event instead of the simulator's and treats the simulator as running. For tests and replay runs. |

Any other option is exit 2, because only the control app starts the recorder.

The recorder reads the simulator's shared memory and sends sessions, laps, setups, pit exits,
tyre readings (simulator data) and the other cars' laps to Pitwall through an outbox in `connectors\iracing\outbox\`. When it stops
while the simulator runs with a session open, it keeps the session open
(`connectors\iracing\session-resume.json`), and a recorder started later in the same session
continues it with the same session key.

## Data folder and logs

Everything the connector writes is under `<workspace>\connectors\iracing\`:

- `recorder.log` and `ui.log`: the logs of the recorder and the control app; the Logs page of
  the window shows them.
- `positions`: one small file per session with the address of every recorded sample (about 8
  bytes a sample), kept until the session's exact positions are found and for at most 7 days
  (see below).
- `ui_config.json`: the options of the control app (the recorder rate and
  starting minimized). Starting with Windows is a value in the current user's `Run` registry key.

## Registration for development

```
cargo build --release
target\release\iracing-connector-ui.exe --register --flavour dev
```

Registering writes `iracing.json` in `%LOCALAPPDATA%\Stintmate\connectors\`; Stintmate finds the
connector there. `STINTMATE_CONNECTORS_DIR` replaces that folder, which is how the tests avoid
the real one. Setting `STINTMATE_DEV_INSTANCE=<tag>` gives a second instance its own mutex and
pipe names, so it shares nothing with an installed connector. Remove the descriptor with
`--unregister`.

## Tools

| Program | Options |
|---|---|
| `iracing-probe` (`tools/probe`) | `--out <folder>` (required), `--minutes N`, `--csv-hz <rate>` (default 1), `--map-name <name>` and `--event-name <name>` (a test mapping instead of the simulator's). Waits for the simulator's map and records what it publishes into the folder; it only reads. |
| `iracing-replay` (`tools/replay`) | `--map-name <name>` (required), `--event-name <name>`, one of `--ibt <file>` or `--synthetic <laps>`, `--hz` (default 60), `--lap-seconds` (default 10), `--loop`, `--stop-after <seconds>`. Writes a recording or a synthetic session into a test mapping; the simulator's names are refused. |
| `iracing-measure` (`tools/measure`) | `--synthetic <seconds>` (required, 5 or more), `--cars <n>` (1 to 64, default 64), `--subscribe`. Replays a synthetic session at 60 rows per second into a test mapping and runs the recorder's own loop over it; prints the CPU time, the time of each row's step, the allocations and the memory. Windows only. See [architecture.md](architecture.md#performance). |

Example, from a clean clone:

```
cargo build --release -p iracing-measure
target\release\iracing-measure.exe --synthetic 600 --cars 64 --subscribe
```

## Related

- [architecture.md](architecture.md): the reader, the recorder's threads and loop, and the measurements.
- [FINDINGS.md](FINDINGS.md): what is known about the simulator's data.
