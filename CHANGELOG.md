# Changelog

All notable changes to this project are written here.

## 0.1.0 (2026-11-05)

- `iracing-core`: `announce` with the announcement catalog `connector.iracing`
  (`data/catalog.json`, English, validated with the runtime's validator), the `Call` and `Calls`
  types, the shared gate (session open, no replay, player in the car, ten quiet seconds after a
  change) and the flag `Detector`: the yellow family with the sector of a slow car, debris, the
  player's own yellow, clear, the blue flag train, red, the meatball flag and the full-course
  caution (start, pits open, last lap, restart). `tests/detector_replay.rs` replays these cases
  from scenarios, and `tests/no_alloc.rs` counts the detector's allocations.
- `iracing-core`: the `Detector` also announces the black flag penalty (without a type, as the
  simulator has no penalty variable) and its serving, the disqualification, the final lap, the
  chequered flag with the position, the player's retirement and a rival's (`on_meta`, from the
  session text), a rival's disqualification, the player's incidents (merged within two seconds)
  and the incident limit warning. The gate no longer starts a quiet time when the session state
  goes from racing to chequered, so the finish is announced. New `announce::spotter::Spotter`
  turns `CarLeftRight` into car left or right, in the middle, still there, clear and all clear
  calls. `tests/detector_replay.rs` has the new cases, a ten-minute randomised test keeps the
  spotter under 120 calls a minute, and `tests/no_alloc.rs` counts its allocations.
- `iracing-core`: `lap` (the per-lap channel columns in Pitwall's units, reserved once, no allocation per
  row; steering is the angle over half the reported lock to lock angle, the clutch is 1 pressed),
  `sectors` (sector times interpolated at the sector starts, dropped when they do not add up to
  the lap time) and `validity` (the lap validity inferred from incidents, time off the track,
  tows and the pit road, and the incident tracker; a lap iRacing did not time is invalid), with
  unit tests and `tests/no_alloc.rs`, which counts allocations on the per-row path.
- `iracing-core`: `meta` builds a `SessionMeta` from the parsed session text (`build`) on the
  thread `iracing-meta` (`MetaWorker`), using the line scan for what the typed parse lost; a
  driver name with a line break no longer loses the whole `DriverInfo` section. `catalog` makes
  the track, layout, car and class ids and the catalog (the whole roster at the start, the cars
  only for later additions). `setup` turns `CarSetup` into the write API setup (typed fields,
  `extra`, unit conversion) and keeps the tyre readings apart. `resume` holds the resume clock
  and the session identity; an offline event resumes only within ten minutes.

- `iracing-core`: `tracker` turns rows into sessions, laps and pit visits (`Tracker::step`, no
  allocation while no lap ends and no session changes) and keeps the live `LapView`. The counters
  `Lap` and `LapCompleted` are taken again whenever driving starts and when one falls (they drop
  to 0 when the player leaves the car and `Lap` rises in the stall on re-entry), a car that
  reappears in the stall in one row is a reset (tested before the wrap of the lap fraction),
  `LapLastLapTime` of -1 makes the lap not timed and invalid, a lap that touched the pit road is
  invalid, and out of the car with a session loaded is the garage. Fuel is read only while
  driving and the tyre temperatures and wear are taken at every stall entry (`StallTyres`). New
  variables: `IsGarageVisible`, `ReplayFrameNumEnd`, `PitstopActive`, `PlayerCarPitSvStatus`.
  A lap time equal to the previous lap's is not seen as new and the lap takes its measured time.
- `iracing-core`: the `Sink`, `Source` and `Clock` traits and the records they carry (`io`),
  `IbtSource` (rows of an `.ibt` file), and, with the `testing` feature, `RecordingSink`,
  `FixedClock` and the scenario steps `session_state` and `drive_through`.
  `tests/golden.rs` replays scenarios through the tracker and compares every call to the sink
  with `tests/golden/<name>.json` (`UPDATE_GOLDEN=1` rewrites them), including one that follows a
  recorded session's order of events; `tests/no_alloc.rs` counts the tracker step.

- Shared runtime moved from `v0.1.0` to `v0.1.2`: a refusal after the hello shows `refused`, and
  every process lowers its priority. No code change was needed.
- `iracing-core`: the game window is found by class `SimWinClass`, with the title
  `iRacing.com Simulator` (lowercase in the spec) as the fallback.
- The recorder's hello test no longer depends on whether iRacing runs on the machine: it accepts
  either state and checks the message and phase that go with it.
- `docs/FINDINGS.md`: the facts observed in a 30-minute offline session (header, formats,
  encoding, window, lap counters, states, surfaces, incidents, pit stops, tyres, setup, signs and
  scales, disk logging), and the list of what is not observed yet.- Repository created.
- Workspace skeleton: toolchain, cargo configuration, licence checks, notices, manual CI file.
- `iracing-core`: `SPEC`, the connector description on the shared connector runtime (id `iracing`,
  simulator iRacing, the process `iRacingSim64DX11.exe`, the product key, 60 Hz), the English UI
  catalog `locales/en.json`, and tests that the spec is valid and the catalog complete.
- The three programs on the runtime, pinned to its release tag `v0.1.2`: `iracing-connector-ui`
  (control app), `iracing-connector-window` and `iracing-recorder`, each with its icon and
  version resource. The recorder connects to Pitwall and reports whether iRacing runs; it reads no
  simulator data and writes no laps.
- Tests for `--version` of the three programs, the recorder against a fake Pitwall (hello, status,
  stop event), `--register` and `--unregister`, and unknown options.
- Docs: `docs/usage.md`.
- `iracing-core`: the empty `Source`, `Sink` and `Clock` traits.
- `irsdk`: the SDK constants (`defs`), the header and variable header layouts with bounds-checked
  parsing (`header`, `vars`), the copy of one row (`row`), the byte source trait (`view`), and the
  synthetic layout, row and live image builders for tests (`testing`).
- `irsdk`: the sampler (newest buffer, tick-checked copy with retries, reset on a layout change or a
  tick drop, decimation of faster tick rates to 60 rows per second), the read-only live reader
  (`live`: the memory map opened with read access, the data-valid event with wait access, and
  `sim_mapping_exists`), and `testing::map::TestMap`, a writer of a uniquely named test mapping that
  refuses the simulator's names. Tests for the sampler, the live reader and the per-tick path
  (no allocation), and a benchmark of one poll with thirty reads.
- `iracing-probe` (`tools/probe`): a read-only diagnostic that waits for the simulator's map and
  records the header, the variables, every session text (raw and decoded), a CSV of the scalar
  variables, raw rows, the layout bytes and the simulator's windows into one folder.
- `irsdk`: the session text. `yaml::decode` (Windows-1252, or UTF-8 when the text carries the
  UTF-8 marker; NUL cut, control characters dropped), `requote` for the free-text name keys,
  `sections` and `SectionHashes`, `parse` with every top-level section parsed on its own (a broken
  section loses only itself), number-with-unit helpers, and a line-based `fallback::extract` for
  the key facts. `session` holds the partial typed model (track, event, sessions, results, sectors,
  drivers, setup). A hand-made corpus of session texts and tests for decoding, parsing and the
  fallback.
- `irsdk`: `ibt::Ibt`, an `.ibt` file read whole and read-only over the same parsers as the live
  map (header, disk sub-header, variables, session text, rows from the first buffer offset; the
  record count is cut to the rows that fit, and a count of zero uses what fits). The synthetic
  `.ibt` builder (`testing::ibt_bytes`), a synthetic lap generator, and `testing::replay`, which
  writes the session text and pushes rows into a test mapping at a chosen rate.
- `iracing-replay` (`tools/replay`): replays an `.ibt` file or a number of synthetic laps into a
  test mapping with a name given on the command line; the simulator's names are refused. It can
  loop and stop after a time.
- `irsdk`: `worker`: `YamlSource` (the session text and its update counter; implemented for the live
  map, which re-checks the counter around the copy, and for `Ibt`), `SessionDoc` (decoded text,
  typed sections, errors, fallback, section hashes) and `YamlWorker`, a thread that re-reads and
  parses the text only when the counter moved and discards a parse the counter overtook. `notify`
  is an atomic store and an unpark; `latest` hands out the newest document without waiting.
  `Ibt::session` now returns a `SessionDoc`.
- `irsdk`: the indentation of the synthetic session text in `testing` is kept (two lines had lost it).
- `irsdk`: property tests (`proptest`) that the layout parsers, the session text path and the byte
  views never panic on arbitrary or damaged input, and that what the builders write is what the
  parsers read; a corpus of hand-made layout images (truncated at every field, impossible counts
  and offsets, unusable variable headers); a session text of 1 MiB generated from the 64-car
  corpus file; six ignored tests for recordings from a real machine, which print a `SKIP` line
  naming the file they need.
- Docs: `docs/architecture.md` (the reader, for an outside reader) and `docs/FINDINGS.md` (what is
  known about the simulator's data, each fact marked documented or not observed yet).
- `iracing-core`: the variable map. `vars::Vars` resolves every SDK variable the recorder reads by
  name (a missing or mistyped required one is an error that names it; an absent optional one is
  listed once and reads as absent, never zero), `tick::Tick` holds one row as plain values in the
  SDK's units and is refilled without allocation, and `codes` maps session type texts, surface
  classes and pit states to the values the recorder writes.
- `iracing-core`: the data types of a session (`meta::SessionMeta` with the track, roster, results,
  setup snapshot and hashes).
- `iracing-core`: the `testing` feature. `full_layout` (every variable of the map, 64-slot per-car
  arrays), `YamlBuilder` (session texts with weekend, sessions, results, sectors, drivers in
  classes, setup with tyre readings; Windows-1252 output; metric or imperial) and `Scenario`
  (scripted drives on a 60 Hz clock: garage, stall, laps, pit stops, off-track, cuts, incidents,
  tows, session changes, flags, cautions, rivals, disconnects), with `Scenario::ibt` and, on
  Windows, `Scenario::replay_into` for a test map.
- `irsdk`: the YAML parser is now `serde-saphyr` (safe Rust) instead of `serde_yaml_ng`, which depends on the
  unmaintained `unsafe-libyaml` (RUSTSEC-2024-0449, failing `cargo deny check advisories`). The parser's
  budget is set for session texts of a few hundred KB. The minimum Rust version is now 1.89.
- `irsdk`: breaking, `CarSetup` now wraps a `serde_json::Value` instead of the YAML parser's value type, so the
  parser is not part of the public API.
