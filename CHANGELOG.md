# Changelog

All notable changes to this project are written here.

## Unreleased

- Repository created.
- Workspace skeleton: toolchain, cargo configuration, licence checks, notices, manual CI file.
- `iracing-core`: the empty `Source`, `Sink` and `Clock` traits.
- `irsdk`: the SDK constants (`defs`), the header and variable header layouts with bounds-checked
  parsing (`header`, `vars`), the copy of one row (`row`), the byte source trait (`view`), and the
  synthetic layout, row and live image builders for tests (`testing`).
