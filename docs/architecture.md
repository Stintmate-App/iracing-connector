# Architecture

This page explains how the repository is built, for someone who has not seen it before. Paths
are relative to the repository root. Facts about the simulator itself are in
[FINDINGS.md](FINDINGS.md).

## What is in the repository today

| Path | What it is |
|---|---|
| `crates/irsdk` | A reader of the iRacing SDK data: the live memory map, `.ibt` files and synthetic images, through one set of parsers. Library. |
| `crates/iracing-core` | The connector's core: the variable map and tick, lap channels, sectors, validity, announcements, the session meta (`meta`, built on its own thread), the catalog ids (`catalog`), the car setup and tyre readings (`setup`) and the resume clock (`resume`). |
| `tools/probe` | `iracing-probe`: a read-only diagnostic that records what the simulator publishes into a folder. |
| `tools/replay` | `iracing-replay`: writes an `.ibt` file or a synthetic lap into a test mapping, to drive a reader without the simulator. |
| `tools/measure` | `iracing-measure`: runs the recorder's loop over a synthetic session and reports CPU, step time, allocations and memory (see [Performance](#performance)). |
| `bins/iracing-recorder` | `iracing-recorder`: the recorder, a library and a program (see [The recorder](#the-recorder)). |
| `bins/iracing-connector-ui`, `bins/iracing-connector-window` | The control app and the window, on the shared connector runtime ([usage.md](usage.md)). |

The page first describes the recorder, then `irsdk`. The `irsdk` crate never sends anything to the simulator: it
defines no message to send, never maps memory for writing under the simulator's names, and
writes no file in the simulator's folders.

## The recorder

`iracing-recorder.exe` is started hidden by the control app. Its loop is `recorder::run`
(`bins/iracing-recorder/src/recorder.rs`).

### Threads

| Thread | Does | Source |
|---|---|---|
| main (the sampling thread) | waits for the simulator's event for at most 25 ms, copies the row, runs the tracker, the detectors and the spotter, and publishes the live frame and the telemetry sample; fills the status every 500 ms | `recorder.rs` (`WAIT`, `STATUS_EVERY`) |
| `irsdk-yaml` | reads and parses the session text when its update counter moved | `crates/irsdk/src/worker.rs` |
| `iracing-meta` | builds the session description (`SessionMeta`) from the parsed text | `crates/iracing-core/src/meta.rs` |
| `outbox-summary` | reads the outbox's counters every 250 ms, so the sampling thread never takes the outbox lock | `bins/iracing-recorder/src/summary.rs` |
| `telemetry` | sends the samples of the ring to Pitwall while it is subscribed | `bins/iracing-recorder/src/telemetry.rs` |
| `positions` | after a session ended and while none is open, finds exact positions for its laps in iRacing's disk telemetry; wakes every 30 s, in the Windows background mode | `bins/iracing-recorder/src/positions.rs` |
| `announce` | builds the announcement of a fired call and hands it to the runtime's announcer | `bins/iracing-recorder/src/announce.rs` |

The shared connector runtime adds its own threads: the link to Pitwall, the writer of the outbox
and the link to the control app. Writing a lap, a setup or the other cars' laps happens there,
never on the sampling thread.

### The loop

While no simulator is open the loop looks for it once a second: the process
`iRacingSim64DX11.exe`, or, with `--sdk-map`, the named test mapping. With a mapping open,
`Reader::poll` does, for each row (`bins/iracing-recorder/src/recorder.rs`):

1. `Live::wait`: wait for the event, then poll the sampler (see "The tick-checked copy").
2. `Tick::fill`: read every variable of the row into a plain-value struct without allocating.
3. Pass the session text's update counter to `irsdk-yaml`, and take a new `SessionMeta` from
   `iracing-meta` when it was built from the text the simulator holds now.
4. `Pipeline::row`: the `Tracker` turns rows into sessions, laps and pit visits and writes them to
   the sink; `Detector` and `Spotter` read the row and may fire a call; the `RivalTracker` follows
   the other cars' laps.
5. `Pipeline::publish`: copy the row into the live frame's double buffer
   (`bins/iracing-recorder/src/live.rs`: two slots, each guarded by a sequence number) and, while
   Pitwall is subscribed, push a 32-byte sample into an 8-slot ring of atomics
   (`bins/iracing-recorder/src/telemetry.rs`, `SLOTS`).

Nothing in steps 1 to 5 allocates while no lap ends, no session opens or closes and no call
fires; `bins/iracing-recorder/tests/alloc.rs` counts allocations on the loop thread over a replay.
The JSON of the live frame is built by the request's handler thread from the last published
snapshot (`LiveBuffer::answer`), never by the loop.

### The serving side

The recorder registers its methods before the link to Pitwall starts
(`bins/iracing-recorder/src/serve.rs`, `recorder.rs`): `connector.live_frame`,
`connector.capture_target`, `connector.action` and the stream `connector.telemetry`. Their
behaviour is in [usage.md](usage.md). The telemetry stream repeats the last sample every 250 ms
with the fresh flag clear when no row arrives (`telemetry.rs`, `KEEPALIVE`); the ring is
single-producer, single-consumer, and a full ring drops the new sample and counts it.

### Announcements

The detectors run on the sampling thread. A fired call is a small value moved into a 64-slot
channel (`announce.rs`, `CAPACITY`); when the channel is full the call is dropped, never waited
for. The `announce` thread turns it into an announcement and gives it to the runtime's announcer,
which applies its message windows (10 messages in any 60 s, 120 of the spotter group). The
catalog of announcements (`crates/iracing-core/data/catalog.json`), the guidance for the
engineer and the actions are declared to Pitwall at every connection (`serve.rs`, `install`).
`bins/iracing-recorder/tests/announce.rs` checks the windows against a fake Pitwall.

### Performance

Measured with `iracing-measure` (`tools/measure`), which runs the recorder's own loop
(`Reader::poll_timed` and `Pipeline`, the live frame, the telemetry ring, the detectors and the
spotter included) over a synthetic session replayed into a test mapping at 60 rows per second.
The records go to a sink that counts them; the pipe to Pitwall, the writer thread and the
simulator itself are not part of the measurement. Allocations are counted on the sampling thread
only, with a counting global allocator.

**How it was measured.** On 2026-11-07, one run of
`target\release\iracing-measure.exe --synthetic 600 --cars 64 --subscribe`, built with
`cargo build --release -p iracing-measure` (the workspace's release profile: thin LTO, one
codegen unit) with rustc 1.99.1, on Windows 11 Pro, an Intel Core i7-10700K at 3.8 GHz (8 cores,
16 logical processors; model and counts from `Win32_Processor`), with the simulator not running
and no other heavy program. It is a synthetic replay: 64 cars, laps of 60 s, 36499 rows (608.3 s),
the telemetry stream switched on and drained by another thread. One run, not a distribution
over runs; the figures will differ on another machine.

| Figure | Result |
|---|---|
| Step per row, from the wait returning to the row published (tracker, rivals, detectors, spotter, live frame, telemetry sample) | p50 6.7 us, p99 27.6 us, max 490.0 us |
| Sum of the steps | 0.304 s over 608.3 s, 0.05 % of one core |
| CPU of every thread of the process except the feeder (the loop, the session text and description threads, the drain thread) | 1.766 s over 608.3 s, 0.29 % of one core |
| Allocations on the sampling thread | 2514 in 36499 rows; 36459 rows allocated nothing; the 40 that did (the session opening, the laps ending, the rivals' rows, the first description) allocated at most 253 times in one row |
| Working set of the process | 136.3 MiB before the loop, 137.1 MiB after, peak 137.2 MiB; 121.7 MiB of it is the replayed rows, held in memory by the tool, so the loop's growth is 0.8 MiB |
| Torn copies, resets | 0, 0 |

The system accounts a thread's CPU time in clock ticks of about 15.6 ms: the process figure is
statistical, and the sampling thread's own figure read 0.000 s (below a tick), so it is not
reported; the step times come from a monotonic clock and are exact. The step time includes the
cost of the allocations in the rows that made them. The pipe to Pitwall, the outbox writer and
the real simulator are not in these figures: the simulator writes the map in its own process.
Rebuild and run the command above to reproduce them.

## The reader, in one picture

```
 iRacing                         irsdk                                     your code
 -------                         -----                                     ---------
 memory map  --(read access)-->  live::MapView  --View-->  Sampler  --Poll-->  Row + Var<T>
 data event  --(wait access)-->  live::Live::wait ---------^
 session text (inside the map) -----------------> YamlWorker (own thread) --> SessionDoc
 .ibt file  ---------------------> Ibt  --View (SliceView)--^
```

Every parser works on `&[u8]`. The live map is never turned into a slice (see "What is
`unsafe`"); the parsers run on copies of its parts, and on files, which is how one set of
functions serves both.

## The map

The simulator publishes one named memory mapping (`defs::MEM_MAP_NAME`,
`crates/irsdk/src/defs.rs:7`). It starts with a 112-byte header (`header::Header`,
`crates/irsdk/src/header.rs:135`), followed by:

- the variable headers: 144 bytes each, `num_vars` of them at `var_header_offset`, each naming
  one telemetry variable with its type, offset inside a row, element count, unit and
  description (`vars::VarTable`, `crates/irsdk/src/vars.rs:133`);
- the session text (YAML) at `session_info_offset`, `session_info_len` bytes;
- up to four row buffers (`num_buf`), each `buf_len` bytes, rotated by the simulator. The
  header says, for every buffer, the tick count of the row inside and where the buffer is.

An `.ibt` file is the same layout with a 32-byte sub-header after the header
(`header::DiskHeader`) and the rows laid one after the other from the first buffer offset
(`crates/irsdk/src/ibt.rs:72`).

Parsing is checked everywhere: the length of the input is compared with every offset and
length with checked arithmetic, and a layout that does not fit is a `LayoutError` naming the
field (`crates/irsdk/src/header.rs:208`). A variable header that cannot be used (unknown type,
negative or zero count, empty or repeated name, a value that would end past the row) is skipped
and listed with its reason in `VarTable::skipped()`; it never fails the table
(`crates/irsdk/src/vars.rs:160`).

Variables are looked up by name, once, into typed handles: `Var<T>` for a scalar and
`ArrayVar<T>` for an array. Reading a handle is an offset and `from_le_bytes` on the row. A
variable the car does not have is an error (`VarError::Missing`) at lookup time, never a silent
zero. A handle remembers the generation of the table it came from; after a reset it must be
looked up again.

## The event

The simulator also publishes a named event that it signals after each new row. `Live::wait`
waits on it with a timeout and then polls **whether the event was signalled or the wait timed
out** (`crates/irsdk/src/live/mod.rs:80`). The event is a hint, not the truth: other programs wait
on it too, and a signal can be consumed by another reader. The tick count in the header is the
truth, so a missed signal costs at most one tick. The event is opened with wait access only and
the only call made on it is waiting (`crates/irsdk/src/live/sys.rs:186`).

## The tick-checked copy

The simulator writes the map while we read it, so a row can change under the copy.
`Sampler::poll` (`crates/irsdk/src/sampler.rs:119`) never trusts a copy until the tick count of
its buffer has been read again afterwards:

1. read `status`; if the connected bit is clear, the result is `Poll::Disconnected`;
2. compare the layout fields in the header (`num_vars`, `var_header_offset`, `num_buf`,
   `buf_len`) with the attached ones; a difference is a reset (below);
3. pick the buffer with the highest tick count among the `num_buf` in use; a tick lower than the
   last one we delivered is a reset (a new session or a restart of the simulator);
4. a tick equal to the last one is `Poll::NoNewRow`;
5. decimation: the simulator can tick faster than 60 Hz; only one row in
   `max(1, tick_rate / 60)` is kept (`crates/irsdk/src/sampler.rs:96`), the others count as
   `skipped_rate`;
6. copy the buffer into the preallocated row, then read the tick count of that buffer again. If
   it is unchanged the row is whole (`Poll::Row`); if not, try again, at most twice, then give
   up with `Poll::Torn` (`crates/irsdk/src/sampler.rs:161`).

`poll` allocates nothing, and a test counts allocations over ten thousand polls
(`crates/irsdk/tests/no_alloc.rs`).

## Resets

When the layout changes or the tick count drops, the sampler reads the header and the variable
headers again, builds a new `VarTable` with the next generation, allocates a new row and
returns `Poll::Reset` (`crates/irsdk/src/sampler.rs:190`). This allocates, which is allowed:
it happens at a session change, not per tick. The caller looks its `Var` handles up again. If
the new layout cannot be read, the result is `Poll::Disconnected`. A disconnected map is kept
open: the mapping can outlive a session, and the caller decides when to open again.

## The session text and its worker

The session text is a YAML document of tens of kilobytes (about a megabyte in a large grid
with a large car setup). Parsing it takes milliseconds, so it never happens where rows are
sampled. The header carries a counter, `session_info_update`, that changes whenever the text
changes.

`worker::YamlWorker` (`crates/irsdk/src/worker.rs:141`) is a thread named `irsdk-yaml`. The
sampling thread calls `notify(update)`, which is an atomic store and a thread unpark, with no
allocation and no wait. The worker:

1. parks until a notice differs from the last one it dealt with;
2. copies the text with `YamlSource::copy_yaml`, which reports whether the counter was the
   same before and after the copy; a torn copy is retried (`crates/irsdk/src/worker.rs:226`);
3. decodes and parses it into a `SessionDoc`;
4. checks that the counter still has the value it parsed; if it moved, the parse is thrown away
   and the worker starts again (`crates/irsdk/src/worker.rs:239`);
5. publishes the document; `latest()` hands out the newest `Arc<SessionDoc>` without waiting
   for a parse.

Decoding (`yaml::decode`, `crates/irsdk/src/yaml/mod.rs:47`): the text ends at the first NUL
(the map pads with NULs); it is read as UTF-8 only when it starts with the UTF-8 marker lines,
otherwise as Windows-1252 through a 32-entry table; undefined bytes and control characters
other than line feed and tab are dropped. Names typed by users contain characters that break
YAML, so `yaml::requote` puts double quotes around the values of the name keys before parsing
(`crates/irsdk/src/yaml/mod.rs:95`).

Parsing (`yaml::parse`, `crates/irsdk/src/yaml/mod.rs:245`) splits the text into top-level
sections and deserializes **each section on its own** into a partial typed model
(`crates/irsdk/src/session.rs`). A broken section loses only itself and is reported in
`errors`. The YAML parser is `serde-saphyr` (safe Rust), with its default duplicate-key rule (a repeated key
fails its section) and a budget sized for a session text of a few hundred KB
(`crates/irsdk/src/yaml/mod.rs:281`). A line scanner, `yaml::fallback::extract`, runs on the same text without a YAML
parser and gives the key facts (track, session ids, the player's car) even when the typed parse
of a section failed. `SectionHashes` lets a consumer tell which section changed between two
documents.

## Position (dead reckoning)

The simulator publishes no world position, so each lap carries its own `x` (east) and `z`
(north) channels in metres, computed in `crates/iracing-core/src/position.rs`. The channel `y`
is not sent while driving; the exact positions found after the session carry it (see below).

- **Method.** Every row integrates the car's velocity over `SessionTime` (trapezoid, `f64`
  accumulators; the step is the difference of `SessionTime`, never a fixed 1/60 s). The heading
  is `YawNorth`, a compass heading, so the top-down view comes out north-up with no fitting. The
  velocity is `VelocityX` (forward) and `VelocityY` (left) rotated by the heading. Without
  `VelocityX` or `VelocityY` the lap uses `Speed` along the heading (no sideslip, about ten
  metres worse over a lap); without `YawNorth` it uses `Yaw` (compass heading `pi/2 - Yaw`, an
  arbitrary but fixed north); without any heading the lap has no position. The method is
  decided from the first row of the lap, and a lap that changes it is dropped. The recorder logs
  which inputs it has when it attaches.
- **Anchors.** The origin is where the car crossed the start/finish line, at the crossing time
  interpolated between two rows. A lap that started at the line (on the track, not on pit road)
  starts at `(0, 0)`. A lap that started elsewhere (the stall, the garage, a resume) and reaches
  the line is shifted so the line is `(0, 0)`.
- **Closure.** A lap from the line to the line that never touched pit road has its closure
  error (where dead reckoning puts the line at the end of the lap, against `(0, 0)`) spread over
  the lap in proportion to the path driven. A lap that touched pit road keeps its start anchor
  only, because the pit lane meets the line away from the racing line.
- **What is dropped.** A lap sends no `x`/`z` when: a row had no heading; two rows were more
  than 0.25 s apart; it neither started nor ended at the line; or its closure error exceeds
  `max(10 m, 0.5 % of the layout length)`.
- **Cost.** One `sin_cos` and a few multiplications per row, two pushes into reserved columns;
  nothing allocates (`crates/iracing-core/tests/no_alloc.rs` runs it with the velocity and the
  heading on the rows). The anchor and the closure run once per lap, in place.

The accuracy, measured against the `Lat` and `Lon` of recorded `.ibt` files, is in
[FINDINGS.md](FINDINGS.md#position-by-dead-reckoning).

## Exact positions from disk telemetry

Each lap keeps the address of its samples next to the dead-reckoned channels: the `SessionTick`
and `LapDistPct` of every row (two more reserved columns of the lap buffer, pushed with the
others) and the session time of its line crossings. The sink sends laps through the runtime's
`lap_with_address`; once the lap is stored in the outbox, on the writer thread, a record of
it (tag 1) is appended to `<data folder>\positions\<session key>.bin`
(`crates/iracing-core/src/positions/file.rs` has the format). The file starts with a header (the
session key, `SessionNum`, `SessionUniqueID`, `SubSessionID`, the layout's `TrackID`, the player's
`CarPath` and the wall-clock time the session opened); the end of the session appends a closed
time (not when the session is kept open for the next recorder, which continues the same file),
and a lap that has its positions appends a resolved record. Nothing is added to the sampling path
beyond the two reserved pushes.

`positions.rs` holds the thread. At each wake it deletes the files that are finished (closed,
every lap resolved) or older than 7 days, and works on the closed ones that have been quiet for a
minute and have laps left, only when the sampling loop says no session is open (a shared flag set
at the status cadence) and with the stop flag checked before every chunk of a file. A disk file
belongs to a session when it was modified after the session opened, starts inside the session's
wall-clock span, and names the same track, car and sub-session
(`positions/select.rs`). **A file may start up to 30 s before the session opened**
(`START_SLACK_SECS`): this is a tolerance of this connector, because the file's start date has
one-second resolution and the connector learns of a session a moment after the simulator started
it. It is complete when its size is `bufOffset + recordCount x bufLen` and has not changed for
60 s. The job (`positions/job.rs`) matches each recorded sample to a file row by
`(SessionNum, SessionTick)`, checks `LapDistPct`, projects `Lat`/`Lon`/`Alt` to metres about the
lap's line crossing and gives the laps that matched fully to the loop, which queues each as a
`lap_positions` write (the idempotency key makes a repeat harmless). **A lap recorded without
`SessionTick` has tick 0 on every row and matches nothing, so it keeps its recorded positions.**
When every file of the session was complete, the laps that did not match are marked resolved
too, and the thread logs one line for the session.

## `.ibt` files

`ibt::Ibt` reads a whole file into memory and parses it with the same functions as the live
map, over a `SliceView`. The row count is the record count of the sub-header, cut to the rows
that fit in the file; a count of zero (a file that was not closed) uses what fits; a file with
no complete row is `IbtError::NoRows` (`crates/irsdk/src/ibt.rs:84`). The session text is written
once at the start of the file, so an `.ibt` is one session.

## Test support

With the `testing` feature, `irsdk::testing` builds layouts (`LayoutBuilder`), rows
(`RowBuilder`), full live images (`live_image`) and `.ibt` images (`ibt_bytes`) from scratch,
so every test runs without the simulator. On Windows, `testing::map::TestMap` creates a real
named mapping and event under a name chosen by the test, writes rows and session text into it,
and `Live::open_named` reads it like the simulator's. `TestMap` refuses the simulator's names
and any name containing `IRSDK` (`crates/irsdk/src/testing/map.rs:51`), so a test cannot write
where the simulator would. `testing::replay` pushes the rows of an `.ibt` into a `TestMap` at a
chosen rate.

Tests that need recordings from a real machine are written and ignored; see
[FINDINGS.md](FINDINGS.md#what-is-still-to-be-observed) and `crates/irsdk/tests/real_fixtures.rs`.

## What is `unsafe`, and why

`crates/irsdk` has `#![deny(unsafe_code)]` at its root (`crates/irsdk/src/lib.rs:14`). Two
modules switch it off for themselves, and nothing else contains `unsafe`:

- `live::sys` (`crates/irsdk/src/live/sys.rs`): the calls that open the mapping and the event
  (`OpenFileMappingW` and `OpenEventW`, read and wait access only), map the view
  (`MapViewOfFile` with read access), ask for its size (`VirtualQuery`) and wait
  (`WaitForSingleObject`). They are needed because the operating system gives a raw pointer.
- `testing::map` (`crates/irsdk/src/testing/map.rs`): the same calls with write access, on
  names a test chose, to build a mapping that behaves like the simulator's.

Each `unsafe` block carries a `// SAFETY:` comment. The live view is exposed through three safe
methods:

- `len()`;
- `load_i32(off)`: the offset must be 4-aligned and inside the view; the load is atomic with
  acquire ordering (`crates/irsdk/src/live/sys.rs:136`);
- `copy_out(off, dst)`: the range must be inside the view; a raw copy followed by an acquire
  fence (`crates/irsdk/src/live/sys.rs:148`).

No `&[u8]` is ever formed over the live view. The simulator writes it concurrently, so a slice
over it would be undefined behaviour in Rust; copying out is not. The parsers therefore run on
the copies.

Outside `irsdk` there is `unsafe` in three places (`iracing-core` forbids it):

- `bins/iracing-recorder/src/live.rs`: the live frame's double buffer, a sequence-numbered slot
  that one writer fills and any number of readers copy, keeping a copy only when the number did
  not change;
- `tools/probe/src/win.rs`: the calls that list the simulator's windows;
- `tools/measure/src/main.rs`: the counting allocator, and the calls that read a thread's and a
  process's CPU time and the working set.

A property test also drives the safe `View` implementation over arbitrary offsets and lengths
to check that nothing reads outside the data (`crates/irsdk/tests/props.rs`).
