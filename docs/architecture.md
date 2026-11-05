# Architecture

This page explains how the repository is built, for someone who has not seen it before. Paths
are relative to the repository root. Facts about the simulator itself are in
[FINDINGS.md](FINDINGS.md).

## What is in the repository today

| Path | What it is |
|---|---|
| `crates/irsdk` | A reader of the iRacing SDK data: the live memory map, `.ibt` files and synthetic images, through one set of parsers. Library. |
| `crates/iracing-core` | The empty `Source`, `Sink` and `Clock` traits the connector will be built on. |
| `tools/probe` | `iracing-probe`: a read-only diagnostic that records what the simulator publishes into a folder. |
| `tools/replay` | `iracing-replay`: writes an `.ibt` file or a synthetic lap into a test mapping, to drive a reader without the simulator. |

The rest of this page is about `irsdk`. The crate never sends anything to the simulator: it
defines no message to send, never maps memory for writing under the simulator's names, and
writes no file in the simulator's folders.

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

A property test also drives the safe `View` implementation over arbitrary offsets and lengths
to check that nothing reads outside the data (`crates/irsdk/tests/props.rs`).
