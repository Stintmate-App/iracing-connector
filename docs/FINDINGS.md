# Findings

What this repository knows about iRacing's data, and how sure it is.

Every fact has one of three marks:

- **documented**: taken from the iRacing SDK header and its published documentation, and
  implemented in the code at the place named. A fact marked only as documented has not been
  checked against a running simulator.
- **observed**: seen on a real machine, with what was observed and how. Unless a fact says
  otherwise, the observation is the first session described next; facts from later sessions
  (a second probe run, an offline race against AI cars, offline practice) name theirs.
- **not observed yet**: a fact the code assumes, or one that is still open.

The observed session: iRacing build 2026.11.02.01 (members build), an offline Test Drive
(session type `Offline Testing`), a Mazda MX-5 Cup at Oulton Park Circuit, International
layout, one player and no other car, 30 minutes read with `iracing-probe` at 60 Hz (107890 rows,
none torn). The probe also wrote a state sample about once a second; the facts about states and
counters below come from those samples, so a time shorter than a second cannot be told from
them. That session had no race and no other car; a race with other cars was seen only offline,
against AI cars, in the later sessions named where they are used. Nothing here was seen online
(see [Not observed yet](#not-observed-yet)).

## Assumed, to confirm

Every default the code uses without having seen it on the simulator is listed here, with the
place in the code and what would confirm it. The full statement of each is in the section
named; a doc comment that says "Not observed on the simulator yet" points to that section.

| Assumed | Where in the code | What would confirm it |
|---|---|---|
| A session restarted within ten minutes continues the open session while `SubSessionID` is 0 ([Session identity](#session-identity)) | `crates/iracing-core/src/resume.rs` | Two offline events started one after the other: do `SessionUniqueID` and the other ids tell them apart? |
| The layout id is the `TrackID` as a decimal number ([Session identity](#session-identity)) | `crates/iracing-core/src/catalog.rs` | A second layout of a track already seen: a different `TrackID`? |
| Session type texts other than `Offline Testing` match the rules in the code ([Session type](#session-type)) | `crates/iracing-core/src/codes.rs` | The `SessionType` text of a practice, a qualifying, a warm-up and a race session |
| Flag, caution, pit-open and restart rules, and the cause of a yellow or blue flag ([Incidents and flags](#incidents-and-flags)) | `crates/iracing-core/src/announce/detector.rs` | A race with a full-course caution, a local yellow and a lapping car |
| A penalty, its serving, the final lap, the finish and the finishing position ([Race end and rivals](#race-end-and-rivals)) | `crates/iracing-core/src/announce/detector.rs` | A race with a penalty, then the chequered flag, in a multiclass and a single-class event |
| `ReasonOutId` turns non-zero for a car that is out, and a disqualified rival is told once ([Race end and rivals](#race-end-and-rivals)) | `crates/iracing-core/src/announce/detector.rs` | A race with a retirement and a disqualification |
| The incident limit warning threshold ([Race end and rivals](#race-end-and-rivals)) | `crates/iracing-core/src/announce/detector.rs` | An event with an incident limit |
| The values of `CarLeftRight` ([Spotter](#spotter)) | `crates/iracing-core/src/announce/spotter.rs` | Cars alongside, on each side and on both |
| `CarLeftRight` does not flicker at the edge of an overlap, and the call rate stays under the limit ([Spotter](#spotter)) | `crates/iracing-core/src/announce/spotter.rs` | A recording of `CarLeftRight` while passing a car |
| The live frame shows the three live tyre temperatures as tread temperatures, their mean as the carcass temperature, and wear as one minus the mean ([Tyres](#tyres)) | `crates/iracing-core/src/live.rs` | Tyre temperatures against a source that names the layer |
| Another car's pit mode in the live frame is "into the pits" whenever it is on the pit road outside its stall ([Tyres](#tyres)) | `crates/iracing-core/src/live.rs` | A race with rivals making stops |
| The order of the three `TreadRemaining` values is the order the text writes ([Setup](#setup)) | `crates/iracing-core/src/setup.rs` | A setup with different wear on the outer and inner edge of a right-hand tyre |
| A steering lock of 450 degrees when the simulator reports none | `crates/iracing-core/src/lap.rs` | A car whose `SteeringWheelAngleMax` is 0 or absent |

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
- **observed**: the header of the live mapping read layout version 2, status 1 (connected),
  `tick_rate` 60, 3 row buffers of 8600 bytes and 331 variables. The session text area was 524288
  bytes at offset 112; the variable headers came after it, at offset 524400, and the row
  buffers at 1114224 with a stride of 24576. The text area sits before the variable headers, so
  a reader must take every offset from the header and assume no order.
- **observed**: rows arrived at 60.00 per second for the 30 minutes, with 0 torn copies, 0
  retries and no layout reset, also while the player was out of the car.
- **observed**: the `_ST` variables are live at 360 Hz, six values per row
  (`VelocityX/Y/Z_ST`, `YawRate_ST`, `PitchRate_ST`, `RollRate_ST`, `Vert/Lat/LongAccel_ST`,
  `SteeringWheelTorque_ST`, `<wheel>shockDefl_ST`, `<wheel>shockVel_ST`), while `tick_rate`
  stays 60.
- **observed**: suspension travel is live (`<wheel>shockDefl`, `<wheel>shockVel`), and so is the
  tyre odometer `<wheel>odometer` (metres since the tyre was placed on the car; it goes to 0 when
  that tyre is changed). `<wheel>coldPressure` (kPa) is the garage value, 193.053 kPa here.
- **observed**: floats with the unit `%` are fractions from 0 to 1: `RelativeHumidity` read 0.70
  while the session text said 70 %, and `FuelLevelPct` read 0.518.
- **observed**: `PlayerIncidents` exists (type int, unit `irsdk_IncidentFlags`), and so do
  `PlayerCarMyIncidentCount`, `PlayerCarTeamIncidentCount` and `PlayerCarDriverIncidentCount`,
  equal all session.
- **observed**: there is no scalar `PaceFlags`; the variable is the array `CarIdxPaceFlags`.
- **not observed yet**: which variables a given car lacks. The reader reports a missing variable
  as an error at lookup, never as zero (`crates/irsdk/src/vars.rs:222`).
- **not observed yet**: whether `count_as_time` is set on any variable and what a reader should
  do with it. It is parsed and kept (`crates/irsdk/src/vars.rs:103`) and used for nothing.
- **not observed yet**: the values of the pace-mode and pace-flag constants other than
  `PaceMode` 4, the value all session; they are not defined in the code for that reason.
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
- **observed**: the text started with `---`, `WeekendInfo:` and ` Encoding: ISO_8859_1`. It had
  no UTF-8 marker, every version was valid UTF-8, and no byte above 0x7F appeared: the session
  was pure ASCII, so it does not show the difference between Windows-1252 and UTF-8. In
  iRacing's `app.ini`, section `[Misc]`, `irsdkUTF8SessionStr` was 0 (the file's own comment:
  0 is iso-8859-1, 1 is utf8). The other `irsdk*` keys are in that section too, not in a
  `[Telemetry]` section.
- **observed**: the text changed 44 times in 30 minutes (12097 to 12410 bytes): at the session
  load, at a garage visit and once or twice per lap (the results block). No version failed to
  parse.
- **observed**: in the offline Test Drive `ResultsPositions` held the player's row
  (`Position: 1`, `ReasonOutId: 0`, `ReasonOutStr: Running`).
- **not observed yet**: whether a name with a line break reaches the text as is, and whether a
  broken driver list can happen on a real machine. The code survives it: the typed parse loses
  the section, and the fallback scanner still finds the player's car
  (`crates/irsdk/src/yaml/mod.rs:245`, `crates/irsdk/src/yaml/fallback.rs:87`).
- **not observed yet**: how the simulator treats the UTF-8 request, and whether the
  Windows-1252 table above matches every name seen in practice.
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
- **observed**: `TrackLength` is a number, a space and `km` (`4.2865 km`), and
  `TrackLengthOfficial` is written the same way (`4.33 km`). `TrackPitSpeedLimit` is a number, a
  space and `kph` (`49.97 kph`); `TrackPaceSpeed` is `72.40 kph`. The parser takes "n km" and
  "n kph" (`crates/irsdk/src/session.rs:88`, `crates/irsdk/src/yaml/units.rs:14`).
- **observed**: other units in the text: `TrackAltitude: 69.47 m`, `TrackLatitude: 53.180009 m`
  (the unit letter is iRacing's, the value is degrees), `TrackNorthOffset: 3.0267 rad`,
  `TrackAirPressure: 29.68 Hg`, `TrackSurfaceTemp: 27.11 C`, `WindSpeed: 11.27 km/h`. The live
  `AirTemp`, `TrackTemp` and `TrackTempCrew` are in C.
- **observed**: `TrackID` 180, `TrackName: oulton international`, `TrackConfigName:
  International`, and 7 timing sectors in `SplitTimeInfo.Sectors`.
- **observed**: `CarPath` was `mx5 mx52016`, **with a space**; `CarScreenName` `Mazda MX-5 Cup`,
  `DriverCarFuelMaxLtr` 44.987, `DriverCarMaxFuelPct` 1.000, `DriverCarIdx` 0, `CarIsPaceCar` 0,
  `CarIsAI` 0 and `IsSpectator` 0. The tank filled to 44.986 L, the capacity times the fraction.
- **observed**: offline, `SessionID`, `SubSessionID`, `SeriesID` and `SeasonID` were 0, and the
  live `SessionUniqueID` was 1 all session. `WeekendOptions` had `IncidentLimit: unlimited`,
  `CourseCautions: off`, `StrictLapsChecking: default` and `NumStarters: 0`.
- **not observed yet**: whether `TrackName` and `TrackID` are stable for a track across
  seasons and layouts (only one layout was seen), and which of them a consumer should key on.
- **not observed yet**: whether `CarPath` is stable for a car across seasons, and what the value
  looks like for each class of car.
- **not observed yet**: what `DriverCarIdx` is while the player is a spectator or in a replay.
- **not observed yet**: the identity of a session across a reconnection, or across two offline
  events (the offline ids above would be the same).
## Session identity

- **observed**: offline, `SubSessionID` and `SessionID` are 0 in every event and
  `SessionUniqueID` numbers the sessions of an event from 1 (a warmup 1 and a race 2; a test drive
  1), so the ids repeat across events. `SessionTime` restarts with each event and each session.
  The session text is empty (length 0) for a while when the event changes.
  Assumed: while `SubSessionID` is 0 a restarted recorder continues the open session only when its
  first frame comes within ten minutes of the last frame the stopped recorder saw
  (`crates/iracing-core/src/resume.rs`), besides the same ids, session number, car, track,
  layout and session type; the ids alone cannot tell two offline events apart, so the time is
  needed.
- **not observed yet**: whether `TrackID` differs for two layouts of one track. Assumed: the
  layout id is the `TrackID` as a decimal number (`crates/iracing-core/src/catalog.rs`); to
  confirm with a second layout.

## Session type

- **observed**: the `SessionType` of the offline Test Drive was `Offline Testing`.
- **assumed** (not observed beyond that): the text is matched ignoring case, the first rule that
  matches wins: `race`, then `qualif`, `warm`, `test` and `practice`; anything else is "other"
  (`crates/iracing-core/src/codes.rs`). The strings the table test expects are "Practice", "Open
  Practice", "Lone Practice", "Offline Testing", "Lone Qualify", "Open Qualify" and "Race"; to
  confirm with a session of each kind.

## Rates

- **documented**: the header carries `tick_rate`, the ticks per second at which row buffers are
  filled (`crates/irsdk/src/header.rs:135`).
- **documented**: some builds tick at a multiple of 60 per second; the reader keeps one row per
  60 Hz step, that is one row in `tick_rate / 60` (`crates/irsdk/src/sampler.rs:96`).
- **documented**: the data-valid event is signalled for each new row, and other programs wait
  on it too, so the reader polls after every wait, signalled or not
  (`crates/irsdk/src/live/mod.rs:80`).
- **observed**: the `tick_rate` of this build is 60, so no decimation applied; the measured
  rate was 60.00 rows per second over 30 minutes, with 0 torn copies and 0 retries.
- **not observed yet**: how often a copy is torn when the machine is loaded. The reader counts
  torn copies and retries in `SamplerStats` (`crates/irsdk/src/sampler.rs:37`).
- **not observed yet**: what the tick count does when a session changes or a replay is run.
  The reader treats a lower tick as a reset (`crates/irsdk/src/sampler.rs:190`).
- **not observed yet**: whether the mapping outlives the session after the simulator is
  closed. The reader keeps a disconnected view and leaves the decision to its caller.
## Window

- **observed**: the simulator had one visible top-level window for the whole session: window
  class `SimWinClass`, title `iRacing.com Simulator`, client area 1024 x 720, not minimised,
  read 180 times by `iracing-probe` over the 30 minutes. The connector's description (`SPEC`) names the class
  and, as a fallback, the title in lowercase (`crates/iracing-core/src/lib.rs`). The window
  existed before the mapping did.
- **not observed yet**: a virtual-reality mirror window, full screen, a minimised window, and
  what a capture of the window looks like in a headset or on several monitors.

## Laps

- **observed**: `Lap` and `LapCompleted` moved by one at each line crossing, never by two,
  including a crossing made in the pit lane (the stall sits past the line). On the first entry
  into the car both read `Lap 0`, `LapCompleted -1`.
- **observed**: **when the player leaves the car both counters reset**: `Lap` went from 1 to 0
  and `LapCompleted` stayed 0. On re-entering, `Lap` went from 0 to 1 in the stall without a
  crossing. A reset to the pits (the teleport of Test Drive) kept both counters. A reader must
  re-baseline the counters when the player starts driving and read a counter that falls as a
  new baseline, not as a lap.
- **observed**: a reset to the pits ends with `OnPitRoad` and `PlayerCarInPitStall` at 1 and
  `LapDistPct` at 0.0313 (from 0.839 and from 0.232 in the two resets seen);
  `PlayerCarTowTime` stayed 0. A reset from late in the lap would pass a "wrapped past the
  line" test, so the stall reappearance has to be tested first. Method: the first sightings
  came from a probe sampling at about 1 Hz, which saw the change as one step. A 60 Hz `.ibt`
  recorded by iRacing (a reset at session time 1061.5) shows it takes two rows: one row with
  the car out of the world (`PlayerTrackSurface` -1, `LapDistPct` 0, `Speed` 0, `Lap` and
  `LapCompleted` 0, `OnPitRoad` and `PlayerCarInPitStall` still 0), then the row in the stall
  (`PlayerTrackSurface` 1, `OnPitRoad` and `PlayerCarInPitStall` 1, `Lap` and `LapCompleted`
  back at their values). A reader that waits for the stall row has already seen the lap
  jump; the out-of-world row is the sign of the return to the pits.
- **observed**: `LapLastLapTime` and `LapCurrentLapTime` change together, at most one sample
  (1.02 s) after `Lap` increments. In 6 of 10 crossings the row where `Lap` had already
  increased still showed the old lap clock, up to 0.66 s past the line; in the other 4 both had
  updated 0.34 to 0.83 s past the line. A wait of a few seconds is ample.
- **observed**: `LapLastLapTime` is **-1 for a lap that iRacing did not time**: the out lap from
  the stall that ended with a reset, and a lap with a reset. The first lap, from the garage and
  ending in the pit lane, gave no update at all (it stayed 0 for 10 s). Laps with an off-track
  excursion, the in-lap to a stop and the lap with a 67 s stop were all timed, and a lap with
  four wheels on the grass was timed and kept as best.
- **observed**: `LapBestLapTime` and `LapBestLap` were -1 until the first timed lap, then
  followed the best. `LapDistPct` wrapped at the line (0.98 to 0.0127). `RaceLaps` counted like
  `Lap` in testing.
- **observed**: `CarIdxPosition` and `PlayerCarPosition` were 0 until the first timed lap, then
  1 (one second after `LapLastLapTime` got its first value).
- **observed** (offline practice, last minutes): a lap in which the player used the menu's
  return to pits, 19 s after a line crossing, was recorded as a flying lap: the end of the
  session replaced the session identity and took the car out of the world in the same row, so
  the open lap was closed as interrupted with the car last seen on the track. **Handled**: a lap
  open on the track when the simulator ends or replaces the session, when the car leaves the
  world for the garage, or when the car reappears on the pit road in one jump, is typed `in`.
  A game that goes away is an interruption, not a return to the pits: its open lap keeps the type
  its last row gave (flying, unless on the pit road). Out laps and formation laps keep their
  type; a lap ended by a real crossing is unchanged.

- **observed** (offline race against AI): the first lap of a race carried no sector times: its
  official lap time was longer than the sum of its sectors, so the sectors were dropped as not
  adding up. The simulator times that lap from the start, before the car reaches the line, and
  the connector times a lap from the line. **Handled**: on the first lap of a race that began at
  the line, the difference is added to the first sector; elsewhere a mismatch still drops the
  sectors. Which start (standing or rolling) shifted by how much is not observed separately.
- **observed**: `CarIdxPosition`, `CarIdxClassPosition` and the session text's result positions
  read 0 for every car until the first timed lap, so the player had no position on the out lap
  and the first lap of a race. **Handled**: in a race, a car with no position from those sources
  gets its running order, counted from the laps completed and the lap fraction of the real cars
  in the world; outside a race it stays unknown.
- **observed** (offline race, 5 laps, 20 cars): after a pit stop the live position and the cars
  "ahead" and "behind" were stale on the out lap: position 8 with gaps of 0.3 s and 1.7 s while
  the cars around the player on the road were the last two (distance estimates 55 s and 64 s);
  the classification had the player 19th. `CarIdxPosition` and `CarIdxClassPosition` change only at a
  car's own crossing of the line, so mid-lap they keep the order of the previous crossing.
  **Handled**: while a race is under way (`SessionState` 4), every car's position is its running
  order (laps completed plus lap fraction, real cars in the world) and the cars ahead and behind
  are the nearest on the road by that measure; the gap is the difference of `CarIdxEstTime`
  plus the whole laps between the cars at the class lap estimate (the time behind the leader is
  kept where the neighbour is also the adjacent car in the simulator's order). Before the green
  flag, after the chequered flag and outside a race the simulator's positions stand.

## Player state: garage, stall, out of the car

- **observed**: the values in each state (`IsOnTrack` / `IsOnTrackCar` / `IsInGarage` /
  `IsGarageVisible` / `IsReplayPlaying` / `PlayerTrackSurface`):

| State | Values |
|---|---|
| out of the car, before the first drive | 0 / 0 / 0 / 0 / **1** / -1 |
| in the car, in the stall | 1 / 1 / 0 / 0 / 0 / 1 (in the stall) |
| leaving the car (about 6 s) | **0** / **1** / 0 / 0 / 1 / not read |
| garage screen open | 0 / 0 / 1 / 1 / 1 / -1 |
| out of the car, garage screen closed | 0 / 0 / 0 / 0 / 1 / -1 |

- **observed**: **`IsReplayPlaying` is 1 whenever the player is out of the car** (the simulator
  shows its live replay; `ReplayFrameNumEnd` was 1) and 0 while driving. It alone cannot mean
  "menus".
- **observed**: `IsInGarage` is 1 only while the garage screen is open (4 s here), not while the
  car is out of play; `IsGarageVisible` is 1 with it. On leaving the car `IsOnTrack` dropped
  about 6 s before `IsOnTrackCar`.
- **observed**: `FuelLevel` reads 0 while the player is out of the car and the true level again
  on entry. A reader must ignore it when the player is not driving.
- **observed**: `EnterExitReset` was 1 (exit) in the stall, 2 (reset) on track and 0 out of the
  car. `CarLeftRight` was 1 (clear) in the car and 0 out of it, with no other car.
- **not observed yet**: spectating and a rewound replay.

## Surfaces: grass and kerbs

- **observed**: over 1782 samples, `PlayerTrackSurfaceMaterial` read 1 (asphalt) 1432 times, 5
  (concrete, the stall) 166, -1 135, 15 (grass) 35, 9 (paint) 10, 13 (rumble) 3 and 12 (rumble)
  once. `PlayerTrackSurface` read 3 (on track) 1402 times, 1 (in the stall) 159, -1 135, 2 (pit
  lane) 52 and 0 (off track) 34. Grass came with `PlayerTrackSurface` 0 in 34 of 35 rows.
  `CarIdxTrackSurfaceMaterial[0]` equalled `PlayerTrackSurfaceMaterial` in every raw row.
- **observed**: **the material follows one point of the car, its centre, not a wheel.** The
  per-wheel `TireLF/RF/LR/RR_RumblePitch` (Hz) were non-zero on kerbs while the material read
  asphalt in most of those rows: for example two wheels at 40.0 and 49.2 Hz with material 9,
  two at 36.3 and 37.1 Hz with material 13, three wheels at 40 to 42 Hz, and single wheels with
  material 1. A kerb under two wheels shows as asphalt, paint or rumble depending on where the
  centre is; the per-wheel kerb contact is in `Tire*_RumblePitch`.
- **not observed yet**: hold times of short contacts, which the one-second samples miss.

## Incidents and flags

- **observed**: `PlayerCarMyIncidentCount` went from 0 to 14 over the session: +1 with an off
  track, once +1 on paint at 36 m/s (not every +1 is an off), +2 for a spin, and two +1 steps
  one second apart during one off. **`PlayerIncidents` was 0 in all 1782 samples and in all 92
  raw rows** (offline). Twelve raw rows fall within a second after a count change, so a
  one-row pulse cannot be ruled out; a latched value can.
- **observed**: `SessionFlags` read `0x10040200` for most of the session (`startHidden`,
  `servicible`, `oneLapToGreen`), `0x10000200` before the first drive (no `servicible`),
  `0x10040240` for 5 samples (adds `debris`) and `0x100C0200` for 2 s right after the first off
  (adds `furled`). **`oneLapToGreen` (0x200) and `startHidden` were set the whole session, in
  testing.** `green` (0x4) was never set, and no yellow, blue, black or checkered flag
  appeared. `CarIdxSessionFlags[0]` carried only the player's own bits.
- **observed**: `SessionState` 4 (racing), `SessionTimeRemain` 604800, `SessionLapsRemain` 32767
  (unlimited), `PaceMode` 4 (not pacing), `PitsOpen` 1, `TrackWetness` 1 (dry) and
  `Precipitation` 0 all session.
- **observed**: among the 331 variables there is no live `Lat`, `Lon` or `Alt`, no lap-validity
  variable and no penalty variable. The closest names are not validity: `LapDeltaToBestLap_OK`
  and the other `LapDelta*_OK` mean the delta is valid, `LapLasNLapSeq` was 0 all session and
  `DCLapStatus` was 2. `PlayerCarWeightPenalty` and `PlayerCarPowerAdjust` are balance of
  performance (0 here), not penalties.

- **assumed** (not observed on a race yet): the flag announcements follow the flag bits as the
  SDK header documents them. A full-course caution is `cautionWaving` or `caution` rising in a
  race; the pits open when `PitsOpen` goes from false to true during it; `oneLapToGreen` counts
  only while a caution has been seen, in a race; the restart is the `green` flag after the
  caution bits clear. A yellow's cause is a car that moves slower than 6 m/s on the track
  within 350 m behind to 1500 m ahead of the player, its speed taken from its lap fraction
  between two reads; the lapping car of a blue flag is the nearest real car up to 500 m behind
  that is more than half a lap ahead in race distance. To confirm in a race with cautions.

- **observed** (second probe run, offline race): `PlayerIncidents` is a **one-row pulse**: it
  holds its word for one row (1/60 s) and is 0 otherwise, which is why every 1 Hz sample read 0.
  `PlayerCarMyIncidentCount` rises 1 to 3 rows after the pulse. The report byte (low byte) was
  reliable (2 off track, 5 collision with the world, 7 car contact). **The penalty byte did not
  match the points**: "2x" came with +1 once and with 0 once, and the warmup words had no penalty
  byte, so the points are taken from the count only. A report can come with no points. The team,
  driver and own incident counts were equal.
- **observed**: `PaceMode` was 1 (double-file start) in every race sample, also after the green
  and after the finish, and 4 in every warmup sample: it says how the race starts, not whether the
  field is pacing now. No state 3 (parade laps) appeared in the race. `CarIdxPaceFlags` was not
  seen in the race.
- **observed**: the debris flag (`0x40`) came alone, with no yellow, yellow-waving or caution bit,
  at the same place (lap fraction 0.61 to 0.65) on two laps. No yellow, yellow-waving, caution or
  caution-waving bit appeared in the race.

## Race end and rivals

- **observed** (second probe run, offline race of five laps): the **white** bit rose at lap
  fraction 0.909 of lap 4, 12.7 s before the player started lap 5; the **checkered** bit rose at
  0.910 of lap 5, 12.3 s before the player's finish; the session state went to 5 at about the
  same time as the bit. The state went to 6 three seconds after the player left the car. After the
  finish the bits read checkered with `oneLapToGreen`.
- **observed**: a position changes only at that car's own crossing of the line (18 of 18 changes
  of eight cars fell within 5 % of the line). `CarIdxPosition` was 0 for every car on the grid
  and all were set at the first crossing. `PlayerCarPosition` equalled `CarIdxPosition` of the
  player in every sample. The player's position stays after the finish.
- **observed**: after the chequered flag `Lap` rose at a crossing in the pit lane and fell back
  about five seconds later (race 6 to 7 and back to 6; warmup 4 to 5 and back to 4): no lap is
  counted after the session's end.
- **observed**: `ResultsPositions` holds `ClassPosition` counted from 0, while the live
  `PlayerCarClassPosition` counts from 1; `Position` counts from 1.
- **observed**: `SessionLapsRemainEx` went 3, 2, 1, 0 at the leader's crossings, up to 3 s before
  the player's own.

- **observed**: there is no penalty variable (see "Incidents and flags"), so a penalty is
  announced without its type. Offline, `PlayerIncidents` stayed 0, so an incident's cause comes
  from the time off the track (`IncidentTracker`), and the announcement does not name a cause.
- **assumed** (not observed on a race yet): a penalty is the `black` flag bit held for half a
  second in a race, and it is served when the bit clears after the car was on the pit road; the
  `disqualify` bit is the player's disqualification and its `black` bit is not a penalty to
  serve. The final lap is the `white` bit, or, when it is missing, the race leader crossing the
  line with `SessionLapsRemainEx` at 1. The finish is the `checkered` bit; the session state
  5 that comes with it does not start a quiet time. The finishing position is
  `PlayerCarClassPosition` in a multiclass event, else `PlayerCarPosition`; 0 gives a call
  without a position. To confirm in a race.
- **assumed** (not observed yet): `ReasonOutId` of the session text's results turns non-zero for
  a car that is out of the race, for the player and for rivals; the first session text of a
  session is the baseline. A rival shown the `disqualify` bit in `CarIdxSessionFlags` is told
  as disqualified and not again as out. To confirm with a retirement and a disqualification.
- **assumed** (not observed yet): the incident limit warning comes when the points left to the
  event's limit are at most the larger of 4 and a quarter of the limit. To confirm in an
  event with a limit.

## Spotter

- **assumed** (not observed with a car alongside yet): `CarLeftRight` reads 1 with the sides
  clear, 2 a car on the left, 3 on the right, 4 both sides, 5 two cars on the left, 6 two on the
  right, and 0 with the simulator's own spotter off. Two cars on a side count as one.
- **assumed** (not observed yet): the value does not flicker at the edges of an overlap, so a
  side that stays empty for 0.15 s is clear and one that fills again within 1.5 s of its clear
  is called only if it still holds. To confirm with cars alongside; if the value bounces, the
  clear delay goes up.
- **assumed** (not observed yet): the shortest cycle the spotter can follow, a car, a clear after
  0.15 s and a 1.5 s bounce window, gives about 140 calls a minute from a value that flickers on
  both sides at that pace. The announcer's window of 120 spotter messages in 60 s cuts it, so the
  limit holds whatever the value does. To confirm with cars alongside.

## Pit stops

- **observed**: a stop for four tyres and fuel, in order:
  - `OnPitRoad` 1, with `PlayerTrackSurface` 2 while approaching the stall; the line was crossed
    in the pit lane (`Lap` 6 to 7);
  - `PlayerCarPitSvStatus` 102 while the car was too far forward, not yet in its box;
  - `PlayerCarInPitStall` and `PitstopActive` both 1 in the same row, and
    `PlayerCarPitSvStatus` 1 (in progress) until the end;
  - `PitSvFlags` went `0x3F`, `0x1F` (windshield tear-off done), `0x0F` (fuel full), `0x0E`
    (left front), `0x0A` (left rear), `0x08` (right front), `0x00` (right rear); the bits are
    0x01 LF, 0x02 RF, 0x04 LR, 0x08 RR, 0x10 fuel and 0x20 tear-off;
  - fuel rose about 0.76 L/s, and the car was in the stall about 67 s;
  - `PitstopActive` 0 and `PlayerCarPitSvStatus` 2 (complete) with the car still in the stall,
    then `PlayerCarInPitStall` 0, the status back to 0, and `OnPitRoad` 0 at the exit, where
    `PitSvFlags` returned to `0x3F` (the next request).
- **observed**: `PitSvFlags` and the `dp*` request variables read 0 whenever the car sat in the
  stall outside a service, and in the row of a reset. A plain entry into the car in the stall
  also set `PitstopActive` for 8 s while fuel rose from 22.8 to 27.4 L.
- **observed**: the tyre-set counters (`LFTiresUsed`, `TireSetsUsed`) did not move, as Test Drive
  has unlimited sets. `<wheel>odometer` going to 0 is the reliable sign that a tyre was changed.

## Tyres

- **observed**: the live `<wheel>tempCL/CM/CR` and `<wheel>wearL/M/R` stay constant on track and
  change only in the row the car enters the stall: a stop, a reset to the pits or a plain
  entry. Their values are the readings at that moment (`LFtempCM` read 42.05, 54.89, 58.81,
  71.79 and 71.30 at successive entries). They do not reset when the tyres are changed: after
  the stop `LFwearM` kept the old tyre's value until the next entry.
- **observed** (the same files at 60 Hz, iRacing's own telemetry files): the readings change in a
  single row, and that row comes before `PlayerCarInPitStall` rises. On a stop the row is the one
  where `PlayerTrackSurface` goes from 2 (pit road) to 1 (stall area) with `OnPitRoad` already 1;
  the stall flag and `PitstopActive` rise 19 rows (0.32 s) later. On a reset the readings change
  in the out-of-world row (`PlayerTrackSurface` -1, `OnPitRoad` 0), one row before the stall row.
  One entry carried no new values: after the stall flag rose, it toggled between 1 and 0 five
  times in five seconds (the car shuffling in its box) and the readings did not change to the end
  of the file. So a stall entry does not always bring new readings, and the flag can rise several
  times in one visit; a reader has to compare with the values it last took. The `wear` variables
  say `%` in the file header, but the values are fractions of tread remaining, 1 being new.
  **Handled**: the tracker compares the readings on rows on the pit road or in the stall with the
  last ones sent and sends each new set as a `tyre_readings` message ([usage](usage.md)).
- **observed**: the session text's tyre readings (`CarSetup` `LastHotPressure`, `LastTemps*`,
  `TreadRemaining`) changed only at a visit to the garage (`UpdateCount` 1 to 2 to 3), never at
  a stop or a reset, and `TreadRemaining` read `100%, 100%, 100%` to the end while the live wear
  read 0.977. So hot pressures are available only after a garage visit; a stop gives
  temperatures and wear.
- **observed**: the left wheels (`LeftFront`, `LeftRear`) write `LastTempsOMI` (outer, middle,
  inner) and the right wheels (`RightFront`, `RightRear`) write **`LastTempsIMO`** (inner,
  middle, outer): the order mirrors the car.
- **observed**: none of these exist among the variables: brake temperature, live hot pressure,
  ride height, damage.
- **observed** (offline race against AI): the live frame's wheels were empty for the whole race
  while Test Drive filled them. The wheels come only from the readings taken when the car enters
  its stall, and a race that starts on the grid has no stall entry until the first stop; no other
  variable carries live tyre temperatures or wear. **Not handled**: the wheels stay absent until
  the first stall entry.
- **observed** (the same race): the pressure stayed 1.93 bar on all four wheels throughout, and
  the tyre temperatures changed only in the pits. These are the simulator's own limits (pressure
  is known only from the setup text after a garage visit), not faults of the connector.
- **observed**: while the car stands in its pit box the live frame's `phase` stays `on_track`,
  with `pit_mode` saying it is in the box. The protocol's phases are `menus`, `garage`,
  `on_track` (the pit lane included) and `paused`; none says "in the pits", and a recorder
  reads `on_track` as "driving", so the box is told apart by `pit_mode`.
- **observed**: the player's own entry in the field's lap records took its validity from a rule
  that knows only timed laps and pit road, so invalid laps (incidents, cuts) read as valid in a
  classification. **Handled**: the player's rows take the lap tracker's validity for the same lap.
- Assumed: in the live frame the three live temperatures of a wheel are shown as the tread
  temperatures (outer, middle, inner) and their mean as the carcass temperature, and a wheel's wear is
  one minus the mean of its three wear values; whether the live temperatures are tread or carcass
  readings is not observed. To confirm.
- Assumed: another car's pit mode in the live frame is 1 (into the pits) whenever it is on the pit
  road outside its stall, because the visit is not tracked per car. To confirm.

## Setup

- **observed**: `CarSetup.UpdateCount` was 1 at the load and 3 after the garage visit, and stayed
  3; neither the stop nor the resets changed it. For this car the tree holds
  `Suspension.Tire.TireCompound`, `Suspension.Front` (toe-in, anti-roll bar, cross weight,
  shock settings), one block per corner (`ColdPressure` 193 kPa, `LastHotPressure`,
  `LastTemps*`, `TreadRemaining`, `CornerWeight`, `RideHeight`, `Camber`, `ToeIn`, springs and
  dampers) and `Suspension.Rear` (which holds `FuelLevel`). The car has no brake bias, ABS or
  traction control keys.
- **not observed yet**: the order of the three `TreadRemaining` values. Assumed: the order the
  text writes, not reversed for the right-hand wheels as `LastTempsIMO` is; to confirm.

## Signs and scales

Measured on 1427 driving rows above 15 m/s, on a clockwise circuit.

- **observed**: `YawRate` integrated over a flying lap came to -6.34 and -6.44 rad: one turn per
  lap, negative on a clockwise track, so **`YawRate` above 0 is a left turn**.
  `SteeringWheelAngle` correlates with `YawRate` at +0.77 and `LatAccel` with `YawRate` times
  speed at +0.97: **both are above 0 to the left**.
- **observed**: `LongAccel` averaged -9.46 m/s2 under braking and +1.84 at full throttle: above
  0 is accelerating. `VertAccel` averaged 9.73, so it includes gravity.
- **observed**: `Clutch` and `ClutchRaw` read 1.0 in every driving row (pedal up, engaged):
  **0 is pressed**. In the stall `Clutch` read 0 while `ClutchRaw` read 1, because the
  simulator disengages the clutch there; `ClutchRaw` is the pedal.
- **observed**: **`SteeringWheelAngleMax` read 14.0097 rad and the largest absolute
  `SteeringWheelAngle` was 7.0049 rad**: the maximum is lock to lock, so full lock on one side is
  half of it.

## Position by dead reckoning

All of this is **observed** on `.ibt` files of one track (Oulton Park, one car, 60 Hz); the table lists the five laps used, from three files,
with no pit road and no incident, comparing against the file's `Lat` and `Lon`.

- `VelocityX` is forward and **`VelocityY` is positive to the left** (car frame, m/s): with the
  sign as written a lap closes within 0.2 to 2.1 m; with the opposite sign it misses by 17 to
  31 m.
- `Yaw` is counter-clockwise positive. **`YawNorth` is a compass heading, clockwise from north,
  equal to a constant per track minus `Yaw`** (`Yaw + YawNorth` was constant on every sample; the
  constant was 173.4 degrees in all four files). `YawNorth` is a live variable too.
- `SessionTime` steps are 16.67 ms, but one file has a repeated row (0 ms) and a 33.3 ms step:
  the step of the integration comes from `SessionTime`.
- **Accuracy.** `crates/iracing-core/tests/real_fixtures.rs`, test `real_ibt_outline` (ignored;
  run it with `IRSDK_FIXTURES` set to a folder whose `ibt` subfolder holds the files). It cuts
  each file's laps at the `LapDistPct` wrap, integrates `VelocityX`, `VelocityY` and `YawNorth`
  with the code above, applies the closure, projects `Lat` and `Lon` equirectangularly about the
  lap's crossing of the line (east = delta lon x R x cos lat, north = delta lat x R,
  R = 6 378 137 m) and compares row by row, with no rotation and no fit:

| Lap | Closure | Max deviation, no closure | Max deviation | Mean deviation | Path | `Lat`/`Lon` path |
|---|---|---|---|---|---|---|
| file 1, lap 1 | 2.02 m | 1.67 m | 2.60 m | 1.61 m | 4290.2 m | 4285.3 m |
| file 2, lap 1 | 0.38 m | 1.75 m | 1.56 m | 0.69 m | 4292.0 m | 4286.7 m |
| file 3, lap 1 | 0.44 m | 1.71 m | 1.77 m | 0.81 m | 4290.5 m | 4285.3 m |
| file 3, lap 2 | 1.08 m | 1.74 m | 2.28 m | 1.18 m | 4288.3 m | 4283.1 m |
| file 3, lap 3 | 1.58 m | 1.74 m | 2.62 m | 1.45 m | 4288.9 m | 4283.6 m |

  The dead-reckoned path is 0.1 % longer than the `Lat`/`Lon` path. The closure is measured
  between the interpolated line crossings. Part of it is real: the car does not cross the line
  at the same point across the track on every lap, and the point `Lat`/`Lon` name is about a
  metre from the point the velocity belongs to; spreading it adds up to a metre to the maximum
  on this track (and it is what keeps a long lap closed). The lap with a mean of 1.61 m has the
  largest closure of the five. One track, one car and five laps: the figures say nothing about
  other tracks.
- Without the velocity, `Speed` along the heading (no sideslip) gave 9.7 to 15 m maximum
  deviation on the same laps (measured once, with a script outside this repository).

## Disk telemetry

- **observed**: after the session `app.ini`, section `[Misc]`, held `irsdkEnableDisk=1`,
  `irsdkEnableMem=1`, `irsdkAutoLogDisk=0`, `irsdkLog360Hz=0`, `irsdkLogAllCars=0`,
  `irsdkLogSetup=1` and `irsdkLimitFileSize=0`. The file was last written when the simulator
  closed, so these values may have been set at exit rather than during the session.
- **observed**: `IsDiskLoggingEnabled` and `IsDiskLoggingActive` were 0 in all 1782 samples, no
  `.ibt` file existed under the iRacing documents folder afterwards, and the logging key
  started nothing.

- **observed** (second probe run): four `.ibt` files (60 Hz, 282 variables, `ver 2`) hold `Lat`,
  `Lon` and `Alt` (`Alt` 54 to 72 m over a lap) and, per wheel, `tempL`, `tempM`, `tempR`
  (surface temperatures), `pressure`, `rideHeight` and `speed`, plus `BrakeABScutPct`: **28
  variables that exist on disk only** (the live map has none of them). The files lack 77 live-only
  variables: every `CarIdx*` array, `IsInGarage`, `IsReplayPlaying`, `CarLeftRight`, `RaceLaps`
  and the 360 Hz variables. The session text in a file is that of the whole weekend. The
  accuracy of dead reckoning against `Lat` and `Lon` is under "Position by dead reckoning".

## Not observed yet

- A race session, a safety-car period, parade laps, and the flags under caution (yellow, blue,
  black, checkered, and `green` after a yellow); `PitsOpen` under caution; `ReasonOutStr` other
  than `Running`.
- Other cars: `CarLeftRight` with a car alongside, the gaps and estimated times of rivals, and
  the rivals' arrays.
- Online sessions: the id fields per event, the `PlayerIncidents` pulse and the limits of the
  `CarIdx*` arrays.
- A second layout or event, to see whether `TrackID` differs per layout.
- A pause, spectating, a rewound replay, a virtual-reality mirror, full screen.

Tests for recordings from a real machine are written and ignored. `crates/iracing-core/tests/real_fixtures.rs`
reads the folder `iracing-probe` writes (`IRSDK_FIXTURES=<folder>`): the recorded rows fill the
tick, every recorded session text builds a session description with the track, the layout, the
player's car, the fuel capacity and the sectors, the setup gives typed pressures and fuel, the
signs of the lateral and longitudinal acceleration agree with the steering and the brake, and,
when the folder holds `.ibt` files, the lap crossings match the declared lap count.
`crates/irsdk/tests/real_fixtures.rs` reads `.ibt` files, a header dump and session texts from
`crates/irsdk/tests/fixtures/real/`. Run `cargo test -p iracing-core --test real_fixtures -- --ignored --nocapture`
and `cargo test -p irsdk -- --ignored --nocapture`; each test prints `SKIP` and what it needs
when it finds nothing. The recordings are real user data and are not in the repository.
