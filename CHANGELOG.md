# Changelog

All notable changes to this project are written here.

## 0.1.13 (2026-11-11)

- **Getting into the car is recorded every time.** The tyre readings written when you get into the car from the garage or the menu (`stall_tyres`, occasion `car_entry`) were missing: the first row after the entry is not yet in the pit stall, so the connector took its readings as a starting point and the stall row 17 ms later compared equal. Now the entry row writes a `car_entry` record every time, even when the readings equal the last ones, in the first entry of a session and in the later ones. The stall row that follows an entry is no longer read as a reset to the pits, so it does not end a lap as a return to the pits either. A reset to the pits (from the track) is still `reset`, and a recorder that starts while the car is already driving still writes nothing for that first row.
- Leaving the car gives no new readings, so nothing is written for it. `docs/FINDINGS.md` has the rows of the car entry and of two resets seen in a 60 Hz live recording, and corrects the 0.1.12 note that said a reset and leaving to the menu looked the same: what it took for leaving to the menu was a reset.

## 0.1.12 (2026-11-11)

- **Works with an older Stintmate without false alarms.** The connector runtime is now pinned at v0.5.0: when the linked Pitwall does not declare `sim_data_kinds`, `sim_data` or `lap_positions` (an older Stintmate), those writes are skipped and logged once, instead of being refused and reported in the connector window as refused writes. With a Pitwall that has them nothing changes.
- **No more warning a second while iRacing starts.** Until the game publishes its data, its shared memory reads layout version 0, and the recorder logged "the live layout was refused: unsupported layout version 0" once a second. That is now a quiet wait: one line "Waiting for iRacing to publish its data" at the start of each wait, the repeats at debug level. Any other unsupported layout version is still a warning. Checked against the recorder's log of nine game starts: every refusal said version 0 and each was followed by a successful attach.
- **What the tyre-reading occasions can and cannot tell apart (documented, no change in behaviour).** Two offline test sessions (iRacing's own `.ibt` files, replayed through the tracker) show that a reset to the pits and getting out of the car to the menu look the same in the data: both give a row with `PlayerTrackSurface` -1 followed by a row in the stall, with the same `EnterExitReset` values, and the readings change in the first row. Only the values differ (after the menu the car shows a new set, about 43 °C and tread 0.9998). The connector therefore still labels both `reset`. `docs/FINDINGS.md` has the rows and says which recording would settle the entry into the car from the garage.

## 0.1.11 (2026-11-11)

- **Exact positions from iRacing's disk telemetry.** While you drive, each lap's `x` and `z` are still dead-reckoned (about 2 to 3 m). When iRacing also logged the session to disk (Alt+L, or its option to log automatically; the connector never changes that setting), the recorder finds the matching `.ibt` files after the session, reads the car's latitude, longitude and altitude for each recorded sample, and sends them as `lap_positions` messages that replace the lap's `x` and `z` and add `y`. Laps that iRacing did not log keep their dead-reckoned positions.
- How it works: for each lap the recorder keeps the `SessionTick` and `LapDistPct` of every sample, and the session time of its line crossings, in a small file per session in `positions\` in the connector's data folder (about 8 bytes a sample, written once per lap on the writer thread; nothing is added to the sampling path beyond two reserved columns). After the session has ended, and only while no session is open, a low-priority thread (wakes every 30 s) matches those samples to file rows by `(SessionNum, SessionTick)`, checks `LapDistPct`, and projects to metres about the lap's line crossing. It waits for each disk file to be complete and unchanged for a minute, stops at once when you get back into a car, only reads under `Documents\iRacing\telemetry` and never writes there. A lap that contains a row with latitude and longitude both 0 (the car out of the world) or whose samples are not all in the files keeps its positions. Files of finished sessions are deleted, and any file after 7 days. One line per session in `recorder.log` says how many laps got exact positions.
- A file may start up to 30 s before the session opened (the connector learns of a session a moment after iRacing started it and a file's start date has one-second resolution).
- The connector declares the feature `lap_positions` and tells the engineer, through its guidance, that positions are exact only for laps iRacing logged to disk. The connector runtime is now pinned at v0.4.0, which provides the message and the feature.
- Needs a Pitwall that implements `lap_positions`: an older one refuses the message and the connector drops the write.
- `docs/FINDINGS.md` records what was observed of the disk files; `docs/usage.md` has a section on the feature.

## 0.1.10 (2026-11-11)

- **The readings in the pits are sent as simulator data.** Each new set of tyre readings goes as a `sim_data` record of the kind `stall_tyres`, and the connector describes the kind to Pitwall field by field (`sim_data_kinds`), instead of the `tyre_readings` message of 0.1.9, which was never released. Same moments, same values: the change detection, the occasions and the order (after the lap that ended before it, before the stop's `pit_exit`) are as in 0.1.9.
- The connector declares the feature `sim_data` instead of `tyre_readings`. The connector runtime is now pinned at v0.3.0.
- Needs a Pitwall that implements simulator data: an older one refuses the two messages and the connector drops them.

## 0.1.9 (2026-11-10, tagged, never released)

- **Tyre readings at stops.** iRacing gives its tyre temperatures and tread remaining only in the pits (a stop, a reset to the pits, getting into the car), so the connector now sends each new set as a `tyre_readings` message, which needs a Pitwall that implements it (an older Pitwall refuses the message and the connector drops it). A set is new when the readings on a row with the car on the pit road or in its stall differ from the ones last sent in the session; the first driving row of a session only sets the baseline (unless the car is in its stall, which sends it as `car_entry`). The `occasion` is `pit_stop`, `reset` or `car_entry`. Readings that change in the same visit are sent again, and an entry that carries no new values sends nothing. The message comes after the lap that ended before it and before the stop's `pit_exit`. Carcass temperatures and tread remaining are sent per wheel and edge (`in` is the edge towards the car's centreline, 1 is a new tyre); iRacing gives no surface temperature or hot pressure at a stop, so those are left out.
- The connector declares the feature `tyre_readings`. The connector runtime is now pinned at v0.2.2, which provides the message and lists the feature.
- Readings that change more than once while a lap waits for its time are all sent, in order; a wheel whose values were all out of range is left out of the message (and none is sent when no wheel is left).

## 0.1.8 (2026-11-10)

- **The official icon everywhere.** The three programs (`iracing-connector-ui.exe`, `iracing-connector-window.exe`, `iracing-recorder.exe`) carry the official Stintmate iRacing connector icon in their file properties, and the tray and the connector window show it instead of the drawn letter "I" (the tray with the level badge in its corner, the window alone). The installer uses it for the setup and for the entry in Windows' Installed apps. The shared runtime is now pinned at v0.2.0, which provides the artwork support.
- **CI on pull requests.** The `ci` workflow runs on every pull request to `main` (and by hand). No code change.

## 0.1.7 (2026-11-10)

No behaviour or protocol change.

- Source formatting (`cargo fmt`) and two clippy findings (a collapsible `if`, a test type alias), so
  the release checks pass.
- Hooks in `.githooks/`: the formatting check before a commit and clippy before a push (enabled
  with `git config core.hooksPath .githooks`).

### Documentation

- The documents and the doc comments were checked against the code again. Corrected: the
  installer's AppId line, the list of declared features, which sessions the findings come from
  (offline practice and offline races against AI cars were seen, nothing online), the gap and
  position doc comments after the race-position change, and the README's wording on Pitwall.
  Behaviour is unchanged.

## 0.1.6 (2026-11-09)

- **Lap type on return to the pits, mid-lap.** A lap left through the menu's return to the pits
  or to the garage while the car was moving on the track was recorded as a flying lap. The
  simulator takes the car out of the world for one row before it appears in the stall, and
  that row is now read as the return to the pits: the lap is an in lap (still partial), and the
  next lap starts from the stall as an out lap. Laps ended by a real line crossing, out laps
  and formation laps are unchanged.

## 0.1.5 (2026-11-09)

- **Lap type on return to the pits.** A lap cut short because the simulator removed the car from
  the track (the menu's return to the pits, most visibly at the very end of a session, or the
  session ending under the car) is now an in lap. It used to be recorded as a flying lap. Laps
  ended by a real line crossing, out laps and formation laps are unchanged, and so is a lap
  interrupted by the game closing.
- **Race position after a stop.** In a race, the live position and the gaps to the cars ahead
  and behind now follow the order on the road (laps completed and place on the lap). The
  simulator updates its positions only at the line, so after a pit stop they stayed at the
  order from before it until the next crossing. Practice and qualifying keep the simulator's
  positions.

## 0.1.4 (2026-11-07)

- **Documentation.** Checked against the code; corrections: the version and the pinned runtime tag, the features the connector declares (five were missing), the installer's AppId, the position accuracy bound (2.7 m), one source reference, Pitwall named on first use.

- **Installer.** `iRacing-Connector-<version>-setup.exe` (Inno Setup 6, per user, no
  administrator rights, unsigned) installs the three programs, the licence notices and `icon.png`
  next to the control app, and registers the connector with `iracing-connector-ui.exe --register`;
  uninstalling unregisters it and never touches the data. Unattended mode, progress file and exit
  statuses: [docs/installer.md](docs/installer.md). Built by `scripts/build-installer.ps1`.
- **Release workflow.** A `release` workflow, run by hand, builds the tag's tree, checks it,
  builds the installer and uploads it with its `.sha256` file to a draft release (a dry run by
  default).

## 0.1.3 (2026-11-07)

- **Incident call.** The text no longer glues an "x" to the number ("1x incident"): it reads
  "Incident, points added: {points}. Total: {total}." with both numbers spoken bare, so no
  plural agreement is needed in any language.
- **Chequered call.** The text no longer ends with a full stop after the position, because a
  written ordinal can carry its own in some languages and the two made "20..".
- **Return to pits.** A lap ended by the simulator menu's "return to pits" (the car reappears in
  its stall without driving the pit lane) is typed `in`, not `flying`.
- **Icon.** The connector's 256 x 256 PNG icon is kept in `bins/iracing-connector-ui/assets/icon.png`
  for the setup to install as `icon.png` next to the executables; the descriptor names it in the
  optional `icon` key (shared runtime 0.1.3, now pinned).
- **First race lap sectors.** The simulator times a race's first lap from the start, before the
  line, so its time was longer than its sectors and all of them were dropped. The run to the line
  now goes to the first sector.
- **Live sector after a start away from the line.** A lap that begins mid-track (a reset, a
  tow, the grid) shows the sector the car is in, not the first.
- **Position on the first laps.** While the simulator's positions are 0 (the out lap and a race's
  first lap), a race's participants get their running order by laps completed and lap fraction.
- **Invalid laps of the player in the field records.** The player's rows take the lap tracker's
  validity (incidents, cuts, resets), so a classification counts them.

## 0.1.2 (2026-11-07)

- **Track outline.** Each lap now carries `x` (east) and `z` (north) in metres, dead-reckoned
  from `VelocityX`, `VelocityY` and `YawNorth` at the simulator's 60 Hz, with the origin at the
  start/finish line and the closure of a lap from the line to the line spread along the lap by
  path length. Pitwall draws the track outline and the track maps of iRacing layouts from them.
  Checked against the `Lat`/`Lon` of four `.ibt` files of one track (five clean laps, no
  rotation and no fit): maximum deviation 1.6 to 2.6 m, mean 0.7 to 1.6 m (the
  `real_ibt_outline` test; figures in `docs/FINDINGS.md`).
- Without `VelocityX`/`VelocityY` the speed is used (about ten metres worse); without a heading no
  position is sent. A lap that starts away from the line and reaches it is shifted so the line is
  the origin; a lap with neither end at the line, a gap of more than 0.25 s between rows, or a
  closure error over `max(10 m, 0.5 % of the layout length)` sends none.
- The recorder logs which position inputs it has when it attaches.

## 0.1.1 (2026-11-07)

Fixes for what a second recording session on the simulator showed (a five-lap offline race with
its warmup, and four `.ibt` files).

- **`.ibt` files open.** `Vars::resolve_file` accepts a file's variable table, which lacks the
  variables the simulator publishes only live (`IsInGarage`, `IsReplayPlaying`, `CarIdxLap`,
  `CarIdxLapCompleted`, `CarIdxLapDistPct`, `CarIdxOnPitRoad`, `CarIdxTrackSurface`). They read as not in the garage,
  no replay, lap 0, no completed laps, no car in the world. `IbtSource` uses it; `Vars::resolve`
  still requires them on a live map.
- **`final_lap` is told at the crossing.** The white bit rises about 12 s before the final lap
  starts; the call now waits for the player's next crossing of the line.
- **`chequered` is told at the finish.** The checkered bit rises about 12 s before the player
  finishes, and positions change only at a car's own crossing. The call waits for the player's
  crossing, then half a second, and names the position after it.
- **A debris flag alone is no yellow.** With no yellow bit out it is said as the new `flag_debris`
  ("Debris on the track."); `flag_yellow_cause` ("Yellow for debris.") is kept for a debris flag
  together with a yellow.
- **Incidents.** Two rises of the count that land on either side of the two-second window no longer
  lose the first one: the open incident is settled before the next starts. The points come only
  from `PlayerCarMyIncidentCount`; the report byte of `PlayerIncidents` (a one-row pulse) only
  names the cause.
- **A crossing after the finish.** After the chequered flag the lap counter can rise at a later
  crossing and fall back some seconds later. A crossing after the finish crossing is a lap only
  when the counter holds for 10 s.
- **An empty session text** (seen when the event changes) publishes no session description and
  the previous one stays in force.
- **Class position from the results** counts from 1 (the results count it from 0).
- Tests: the real-recording tests read `.ibt` files from the `ibt/` folder of a probe recording
  and skip empty session texts.

## 0.1.0 (2026-11-06)

First release: a connector that reads iRacing's shared memory and records sessions, laps, setups,
pit exits and the other cars through Pitwall, serves the live frame and the telemetry stream, and
announces flags and the spotter's calls. Read-only towards the simulator.

### `irsdk`

- The SDK constants (`defs`), the header and variable header layouts with bounds-checked parsing
  (`header`, `vars`), the copy of one row (`row`) and the byte source trait (`view`).
- The sampler: newest buffer, tick-checked copy with retries, reset on a layout change or a tick
  drop, decimation of tick rates above 60 Hz to 60 rows per second. No allocation per poll.
- The read-only live reader (`live`): the memory map opened with read access, the data-valid event
  with wait access, and `sim_mapping_exists`.
- The session text: `yaml::decode` (Windows-1252, or UTF-8 when the text carries the UTF-8 marker;
  NUL cut, control characters dropped), `requote` for the free-text name keys, `sections` and
  `SectionHashes`, `parse` with every top-level section parsed on its own (a broken section loses
  only itself), number-with-unit helpers, and a line-based `fallback::extract` for the key facts.
  `session` holds the partial typed model. `CarSetup` wraps a `serde_json::Value`.
- `worker`: `YamlSource`, `SessionDoc` and `YamlWorker`, a thread that re-reads and parses the text
  only when the update counter moved and discards a parse the counter overtook. `notify` is an
  atomic store and an unpark; `latest` hands out the newest document without waiting.
- `ibt::Ibt`: an `.ibt` file read whole and read-only over the same parsers as the live map. The
  record count is cut to the rows that fit; a count of zero uses what fits.
- `testing`: layout, row, live image and `.ibt` builders, a synthetic session whose layout holds
  the variables the recorder needs and whose session text lists the player, `TestMap` (a writer of
  a uniquely named test mapping that refuses the simulator's names) and `replay`.
- The YAML parser is `serde-saphyr` (safe Rust), sized for a session text of a few hundred KB. The
  minimum Rust version is 1.89.
- Tests: the sampler, the live reader, the per-tick path (no allocation), property tests that the
  parsers never panic on arbitrary or damaged input, a corpus of hand-made layouts and session
  texts (one of 1 MiB with 64 drivers), and ignored tests for recordings from a real machine
  (`tests/real_fixtures.rs`). A benchmark of one poll with thirty reads.

### `iracing-core`

- `SPEC`, the connector description on the shared connector runtime (id `iracing`, the process
  `iRacingSim64DX11.exe`, 60 Hz), the English catalog `locales/en.json`. The game window is found
  by class `SimWinClass`, with the title `iRacing.com Simulator` as the fallback.
- `vars::Vars` resolves every SDK variable by name (a missing or mistyped required one names
  itself in the error; an absent optional one reads as absent, never zero). `tick::Tick` holds one
  row as plain values in the SDK's units and is refilled without allocation. `codes` maps session
  type texts, surface classes and pit states.
- `meta`: `SessionMeta` built from the parsed session text on the thread `iracing-meta`
  (`MetaWorker`); a driver name with a line break no longer loses the `DriverInfo` section.
  `catalog` makes the track, layout, car and class ids and the catalog. `setup` turns `CarSetup`
  into the write API setup (typed fields, `extra`, unit conversion) and keeps the tyre readings
  apart. `resume` holds the resume clock and the session identity; an offline event resumes only
  within ten minutes.
- `tracker` turns rows into sessions, laps and pit visits (`Tracker::step`, no allocation while no
  lap ends and no session changes) and keeps the live `LapView`. The counters `Lap` and
  `LapCompleted` are taken again whenever driving starts and when one falls; a car that reappears
  in the stall in one row is a reset; `LapLastLapTime` of -1 makes the lap untimed and invalid; a
  lap that touched the pit road is invalid; a lap that did not start at the line is sent invalid
  with cause `unknown` and no time; a disconnect ends the session after five seconds of wall time.
  `Tracker::stop(true)` drops the open lap; `Tracker::finish_open_lap` sends it without ending the
  session. Fuel is read only while driving; the tyre readings are taken at every stall entry.
- `lap` (the per-lap channel columns in Pitwall's units, reserved once; steering is the angle over
  half the reported lock to lock angle, the clutch is 1 pressed), `sectors` (interpolated at the
  sector starts, dropped when they do not add up to the lap time) and `validity` (inferred from
  incidents, time off the track, tows and the pit road).
- `rivals`: `RivalTracker`, one `ParticipantLap` per real car per lap the simulator counts, with
  the official time once `CarIdxLastLapTime` changes (waiting 3 s, else measured), interpolated
  crossing times, sector times, pit visits, position, best lap and why a car is out.
  `CatalogWatch` sends the catalog again for a session description whose cars differ.
- `live` (`LiveSnapshot`, a fixed-size copy of the row, and `frame_json`, which builds the live
  frame only when asked), `telemetry` (the 32-byte sample, filled without allocating) and
  `guidance` (the items for the engineer).
- `announce`: the catalog `connector.iracing` (`data/catalog.json`, English), the `Call` and
  `Calls` types, the shared gate, the flag `Detector` (the yellow family with the sector of a
  slow car, debris, the player's own yellow, clear, the blue flag train, red, the meatball flag,
  the full-course caution, the black flag penalty and its serving, disqualification, the final
  lap, the chequered flag with the position, retirements, the player's incidents merged within
  two seconds and the incident limit warning) and `Spotter` (`CarLeftRight` to car left or right,
  in the middle, still there, clear and all clear).
- `io`: the `Sink`, `Source` and `Clock` traits and the records they carry; `IbtSource`. With the
  `testing` feature: `full_layout`, `YamlBuilder`, `Scenario` (scripted drives on a 60 Hz clock:
  garage, stall, laps, pit stops, off-track, cuts, incidents, tows, session changes, flags,
  cautions, rivals, `car_left_right`, disconnects), `RecordingSink` and `FixedClock`.
- Tests: golden scenarios (`tests/golden.rs`, also through `IbtSource`), detector replays, a
  ten-minute randomised spotter test, allocation counts for the tracker, rivals, detector and
  spotter (`tests/no_alloc.rs`), the live frame against its JSON schema, and ignored tests for
  recordings from a real machine (`tests/real_fixtures.rs`).

### `iracing-recorder`

- Reads the simulator's shared memory at 60 Hz and records sessions, laps with their channels,
  setups, pit exits and the other cars' laps through Pitwall's write API and an outbox, allocating
  nothing per row (`tests/alloc.rs`); the session text is parsed on other threads. `--sdk-map` and
  `--sdk-event` read a test mapping instead of the simulator's.
- A recorder stopped while the simulator runs with a session open keeps the session open, and a
  restarted one continues it with the same session key (`session-resume.json`). The control app
  answers the resume watcher by reading the session ids from the simulator's memory.
- A new event no longer opens a session with the previous event's car and track: the session
  description is dropped when the simulator goes away, when a mapping opens and when its layout
  restarts, and is taken only when built from the text the simulator holds now. The outbox summary
  is read on its own thread, so the sampling thread never waits for the outbox lock.
- Serves Pitwall: `connector.live_frame` (a double buffer filled by a copy on the sampling thread,
  the JSON built by the handler), `connector.telemetry` (an 8-slot ring of atomics and a sender
  thread at the declared 60 Hz, the `rate_hz` option, a keep-alive repeat every 0.25 s),
  `connector.capture_target` and `connector.action` (`test.ping`, `test.announce`). The
  announcement catalog, the guidance and the actions are declared at every connection.
- Announces events: the detectors and the spotter read every row on the sampling thread and
  allocate nothing while nothing fires; a fired call goes through a 64-slot channel to a sender
  thread and the runtime's announcer, whose windows allow 10 messages in any 60 s and 120 of the
  spotter group. A call over a window or over the channel's capacity is dropped, never waited for.
- Declared features: `setups`, `fuel`, `sectors`, `conditions`, `rivals`, `live_frame`,
  `lap_status`, `capture_target`, `brake_cues`, `guidance`, `actions`, `announcements`, `spotter`.
- `Reader::poll_timed` returns the time a row took, for the measurements; `Reader::poll` reads no
  clock.
- Tests run the recorder against a fake Pitwall: hello and status, stop, a synthetic session,
  laps, the live frame, the telemetry stream, the capture target, announcements, the outbox,
  resume, and the allocation count of the whole loop.

### Programs and tools

- `iracing-connector-ui` (control app), `iracing-connector-window` and `iracing-recorder`, on the
  shared connector runtime pinned to `v0.1.2`, each with its icon and version resource.
- `iracing-probe` (`tools/probe`): a read-only diagnostic that records the header, the variables,
  every session text, a CSV of the scalar variables, raw rows, the layout bytes and the
  simulator's windows into one folder.
- `iracing-replay` (`tools/replay`): replays an `.ibt` file or a number of synthetic laps into a
  test mapping with a name given on the command line; it can loop and stop after a time.
- `iracing-measure` (`tools/measure`): runs the recorder's loop over a synthetic session and
  reports CPU time, the time of each row's step, allocations and memory.

### Documentation and repository

- `docs/usage.md`, `docs/architecture.md` (the recorder, its measurements and the reader),
  `docs/FINDINGS.md` (what is known about the simulator's data, from one offline session, and the
  list of assumed facts).
- Toolchain, licence checks, notices and a manual CI file.
