# Findings

What this repository knows about iRacing's data, and how sure it is.

Every fact has one of two marks:

- **documented**: taken from the iRacing SDK header and its published documentation, and
  implemented in the code at the place named. Nothing here has been checked against a running
  simulator by this repository.
- **not observed yet**: a fact the code assumes, or one that is still open. It has not been
  seen on a real machine.

No fact is marked as observed: no recording from a real machine is in the repository yet. When
one is added, the fact it confirms gets its own mark and a description of the recording.

## Header and variables

- **documented**: the mapping is called `Local\IRSDKMemMapFileName` and the data-valid event
  `Local\IRSDKDataValidEvent` (`crates/irsdk/src/defs.rs:7`).
- **documented**: the layout version is 2; a header with another version is refused
  (`crates/irsdk/src/defs.rs:12`, `crates/irsdk/src/header.rs:170`).
- **documented**: the header is 112 bytes, little-endian. It holds `status` (bit 0 set while the
  simulator is connected), `tick_rate`, `session_info_update`, the session text's length and
  offset, `num_vars`, `var_header_offset`, `num_buf`, `buf_len`, and four pairs of tick count
  and buffer offset from byte 48, 16 bytes apart (`crates/irsdk/src/header.rs:170`).
- **documented**: a variable header is 144 bytes: type, offset inside the row, count,
  `count_as_time`, then name (32 bytes), description (64) and unit (32), each ended by a NUL
  (`crates/irsdk/src/vars.rs:133`).
- **documented**: the types are char, bool, int, bitfield, float and double, of 1, 1, 4, 4, 4
  and 8 bytes. Both `i32` and `u32` handles accept int and bitfield variables, because some
  builds are said to type a bitfield as an int (`crates/irsdk/src/vars.rs:53`).
- **documented**: at most four row buffers; the reader looks only at the first `num_buf`
  (`crates/irsdk/src/header.rs:208`).
- **documented**: an `.ibt` file has a 32-byte sub-header after the header, with the session
  start date and time, the end time, the lap count and the record count
  (`crates/irsdk/src/header.rs:279`).
- **not observed yet**: the real number of variables, the row length and the exact set of
  variable names of the current simulator build. The limits the code accepts (up to 4096
  variables and 4 MiB per row, `crates/irsdk/src/header.rs:15`) are chosen, not measured.
- **not observed yet**: whether `count_as_time` is set on any variable and what a reader should
  do with it. It is parsed and kept (`crates/irsdk/src/vars.rs:103`) and used for nothing.
- **not observed yet**: which variables a given car lacks. The reader reports a missing variable
  as an error at lookup, never as zero (`crates/irsdk/src/vars.rs:222`).
- **not observed yet**: the values of the pace-mode and pace-flag constants; they are not
  defined in the code for that reason.

## YAML and encoding

- **documented**: the session text sits in the map at `session_info_offset`, is padded with
  NULs, and `session_info_update` changes whenever it changes
  (`crates/irsdk/src/yaml/mod.rs:47`, `crates/irsdk/src/worker.rs:31`).
- **documented**: the text is Windows-1252 unless the simulator is asked for UTF-8, in which case
  it starts with `WeekendInfo:` and `Encoding: UTF8` on the first lines
  (`crates/irsdk/src/yaml/mod.rs:32`). Five byte values have no Windows-1252 meaning and are
  dropped (`crates/irsdk/src/yaml/mod.rs:36`).
- **documented**: names that users type (user, team and setup names, abbreviations, initials)
  are written without quotes, so a name with a colon, a quote or a `#` can break the YAML. The
  reader quotes the five name keys before parsing (`crates/irsdk/src/yaml/mod.rs:83`).
- **documented**: booleans in the text are written as `0` and `1`; lengths, speeds and
  pressures are text with a unit (`5.79 km`, `80.00 kph`); an unlimited lap count or time is the
  word `unlimited` (`crates/irsdk/src/session.rs:1`, `crates/irsdk/src/session.rs:98`,
  `crates/irsdk/src/session.rs:161`).
- **documented**: a pace car or a spectator can appear in the driver list; the reader counts a
  driver as real when it is neither and has a name (`crates/irsdk/src/session.rs:265`).
- **documented**: an `.ibt` file's session text is written once; a session change inside a
  recording is not reflected (`crates/irsdk/src/ibt.rs:72`).
- **not observed yet**: whether a name with a line break reaches the text as is, and whether a
  broken driver list can happen on a real machine. The code survives it: the typed parse loses
  the section, and the fallback scanner still finds the player's car
  (`crates/irsdk/src/yaml/mod.rs:245`, `crates/irsdk/src/yaml/fallback.rs:87`).
- **not observed yet**: how the simulator treats the UTF-8 request on the current build, and
  whether the Windows-1252 table above matches every name seen in practice.
- **not observed yet**: how often the text changes during a session and how long the simulator
  leaves the counter inconsistent while it rewrites the text. The worker re-reads when the
  counter moves during a copy or a parse (`crates/irsdk/src/worker.rs:226`,
  `crates/irsdk/src/worker.rs:239`).
- **not observed yet**: the size of the text of a full grid. The code is tested with 64 drivers
  and a text of 1 MiB, chosen as a generous case.

## Track and car identity

- **documented**: the track is described under `WeekendInfo` by `TrackID`, `TrackName`,
  `TrackDisplayName`, `TrackConfigName` and `TrackLength` (a text with a unit)
  (`crates/irsdk/src/session.rs:46`).
- **documented**: the player's car is the entry of `DriverInfo.Drivers` whose `CarIdx` equals
  `DriverInfo.DriverCarIdx`; the entry gives `CarID`, `CarPath`, `CarScreenName`, `CarClassID`
  and `CarClassShortName` (`crates/irsdk/src/session.rs:249`,
  `crates/irsdk/src/yaml/fallback.rs:87`).
- **documented**: fuel capacity in litres is `DriverCarFuelMaxLtr` times `DriverCarMaxFuelPct`
  (`crates/irsdk/src/session.rs:258`).
- **documented**: the sessions of an event are listed under `SessionInfo.Sessions`; the current
  one is named by `CurrentSessionNum` (`crates/irsdk/src/session.rs:143`).
- **not observed yet**: whether `TrackName` and `TrackID` are stable for a track across
  seasons and layouts, and which of them a consumer should key on.
- **not observed yet**: whether `CarPath` is stable for a car across seasons, and what the value
  looks like for each class of car.
- **not observed yet**: what `DriverCarIdx` is while the player is a spectator or in a replay.
- **not observed yet**: the identity of a session across a reconnection (`SessionID`,
  `SubSessionID`).

## Rates

- **documented**: the header carries `tick_rate`, the ticks per second at which row buffers are
  filled (`crates/irsdk/src/header.rs:135`).
- **documented**: some builds tick at a multiple of 60 per second; the reader keeps one row per
  60 Hz step, that is one row in `tick_rate / 60` (`crates/irsdk/src/sampler.rs:96`).
- **documented**: the data-valid event is signalled for each new row, and other programs wait
  on it too, so the reader polls after every wait, signalled or not
  (`crates/irsdk/src/live/mod.rs:80`).
- **not observed yet**: the `tick_rate` of the current build (60, or a higher value for which
  the decimation above applies).
- **not observed yet**: how often a copy is torn at full rate, and whether two retries are
  enough. The reader counts torn copies and retries in `SamplerStats`
  (`crates/irsdk/src/sampler.rs:37`).
- **not observed yet**: what the tick count does when a session changes or a replay is run.
  The reader treats a lower tick as a reset (`crates/irsdk/src/sampler.rs:190`).
- **not observed yet**: whether the mapping outlives the session after the simulator is
  closed. The reader keeps a disconnected view and leaves the decision to its caller.

## Window

- **not observed yet**: the title and class of the simulator's window, whether it is found by
  its process, and what a capture of it looks like in a headset or on several monitors. This
  repository's code does not look for the window.

## What is still to be observed

Six tests are written for recordings from a real machine and ignored until the recordings are
in `crates/irsdk/tests/fixtures/real/` (`crates/irsdk/tests/real_fixtures.rs`). They check
that every row of a recording reads with sensible values, that the variable table equals a
golden list, that a dump of the live header parses, that every session text parses without
a section error, that a replayed recording equals the file, and what the tick rate and
decimation are. Run `cargo test -p irsdk -- --ignored --nocapture` to see which of them find
their files.