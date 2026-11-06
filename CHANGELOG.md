# Changelog

All notable changes to this project are written here.

## Unreleased

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
