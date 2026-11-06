# Findings

What this repository knows about iRacing's data, and how sure it is.

Every fact has one of three marks:

- **documented**: taken from the iRacing SDK header and its published documentation, and
  implemented in the code at the place named. A fact marked only as documented has not been
  checked against a running simulator.
- **observed**: seen on a real machine, with what was observed and how. Unless a fact says
  otherwise, the observation is the one session described next.
- **not observed yet**: a fact the code assumes, or one that is still open.

The observed session: iRacing build 2026.11.02.01 (members build), an offline Test Drive
(session type `Offline Testing`), a Mazda MX-5 Cup at Oulton Park Circuit, International
layout, one player and no other car, 30 minutes read with `iracing-probe` at 60 Hz (107890 rows,
none torn). The probe also wrote a state sample about once a second; the facts about states and
counters below come from those samples, so a time shorter than a second cannot be told from
them. Nothing here was seen in a race session, online or with other cars (see
[Not observed yet](#not-observed-yet)).
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
  "n kph" (`crates/irsdk/src/session.rs:98`).
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

- **not observed yet**: how a session is told from another one in an offline event. Offline,
  `SessionUniqueID` was 1 and `SubSessionID` 0 in the one event seen, probably in every event.
  Assumed: while `SubSessionID` is 0 a restarted recorder continues the open session only when its
  first frame comes within ten minutes of the last frame the stopped recorder saw
  (`crates/iracing-core/src/resume.rs`), besides the same ids, session number, car, track,
  layout and session type; to confirm with two offline events.
- **not observed yet**: whether `TrackID` differs for two layouts of one track. Assumed: the
  layout id is the `TrackID` as a decimal number (`crates/iracing-core/src/catalog.rs`); to
  confirm with a second layout.

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
  read 180 times by `iracing-probe` over the 30 minutes. The connector's spec names the class
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
- **observed**: a reset to the pits changed `OnPitRoad` and `PlayerCarInPitStall` from 0 to 1
  in one row and `LapDistPct` jumped to 0.0313 (from 0.839 and from 0.232 in the two resets
  seen); `PlayerCarTowTime` stayed 0. A reset from late in the lap would pass a "wrapped past
  the line" test, so the stall reappearance has to be tested first.
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

## Race end and rivals

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

## Disk telemetry

- **observed**: after the session `app.ini`, section `[Misc]`, held `irsdkEnableDisk=1`,
  `irsdkEnableMem=1`, `irsdkAutoLogDisk=0`, `irsdkLog360Hz=0`, `irsdkLogAllCars=0`,
  `irsdkLogSetup=1` and `irsdkLimitFileSize=0`. The file was last written when the simulator
  closed, so these values may have been set at exit rather than during the session.
- **observed**: `IsDiskLoggingEnabled` and `IsDiskLoggingActive` were 0 in all 1782 samples, no
  `.ibt` file existed under the iRacing documents folder afterwards, and the logging key
  started nothing.

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
- `.ibt` files, including whether they hold `Lat` and `Lon`.

Six tests are written for recordings from a real machine and ignored until the recordings are
in `crates/irsdk/tests/fixtures/real/` (`crates/irsdk/tests/real_fixtures.rs`). They check
that every row of a recording reads with sensible values, that the variable table equals a
golden list, that a dump of the live header parses, that every session text parses without
a section error, that a replayed recording equals the file, and what the tick rate and
decimation are. Run `cargo test -p irsdk -- --ignored --nocapture` to see which of them find
their files.