# iracing-connector

Stintmate's connector for iRacing: it reads the simulator's telemetry through the iRacing SDK and
records laps, live data and race calls for Stintmate's race engineer. Written in Rust, for Windows.
Read-only: it never writes to the simulator.

Not affiliated with or endorsed by iRacing.com Motorsport Simulations. Licensed under the Apache
License 2.0 ([LICENSE](LICENSE)).

## Status

Under construction. Today the repository holds the `irsdk` crate (a reader of the SDK's data
layouts: header, variable table, rows; with a synthetic writer for tests) and the empty traits of
`iracing-core`. Nothing is read from the game yet.

## Building and testing

The toolchain comes from `rust-toolchain.toml`.

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses advisories bans sources
```

CI runs by hand only (Actions, `ci`); run the checks above locally before a merge.

## Licence and notices

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE). The constants and data layouts of
`crates/irsdk` follow the iRacing SDK header under the BSD 3-Clause licence:
[THIRD-PARTY-NOTICES-irsdk.txt](THIRD-PARTY-NOTICES-irsdk.txt).

## Contributing

Open a pull request. Every change adds a line under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md).
Report vulnerabilities as [SECURITY.md](SECURITY.md) says.
