# Changelog

All notable changes to this project are written here.

## Unreleased

- Repository created.
- Workspace skeleton: toolchain, cargo configuration, licence checks, notices, manual CI file.
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
