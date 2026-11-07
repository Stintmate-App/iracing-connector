# iracing-connector

Stintmate's connector for iRacing: it reads the simulator's telemetry through the iRacing SDK and
records laps, live data and race calls for Stintmate's race engineer (through Pitwall, Stintmate's telemetry database). Written in Rust, for Windows.
Read-only: it never writes to the simulator.

Not affiliated with or endorsed by iRacing.com Motorsport Simulations. Licensed under the Apache
License 2.0 ([LICENSE](LICENSE)).

## Status

Version 0.1.4. What works, tested against synthetic sessions replayed into a test memory
mapping and against one recorded offline session:

- `irsdk` reads the simulator's shared memory (header, variables, rows, the session text) and
  `.ibt` files.
- The recorder records sessions, laps with their channels, sectors and validity, setups, pit
  exits and the other cars' laps through Pitwall, resumes a session after a restart, answers the
  live frame, the telemetry stream, the capture target and two actions, and announces flags,
  cautions, penalties, the finish, retirements, incidents and the spotter's calls.
- Each lap carries its position as the channels `x` (east) and `z` (north) in metres,
  dead-reckoned from the car's velocity and heading: within 2.7 m of the simulator's own
  `Lat`/`Lon` on five laps of one track
  ([docs/FINDINGS.md](docs/FINDINGS.md#position-by-dead-reckoning)).
- The recorder allocates nothing per row while no lap ends and no call fires; its measured cost
  is in [docs/architecture.md](docs/architecture.md#performance).

Not observed yet: everything that needs a race or other cars (the flags, penalties, cautions, the
finish, retirements, the spotter's `CarLeftRight` values, the rivals' classification online), a
second layout or event, a pause, a replay, spectating and a virtual-reality mirror. The defaults
used for them are listed in [docs/FINDINGS.md](docs/FINDINGS.md#assumed-to-confirm). Nothing has
been run against the running simulator except the read-only probe.

## Documentation

- [docs/installer.md](docs/installer.md): the installer, its unattended mode and its contract.
- [docs/usage.md](docs/usage.md): the programs, options, data folder, logs and registration.
- [docs/architecture.md](docs/architecture.md): the recorder (threads, loop, serving side,
  announcements, performance) and how the `irsdk` reader works: the map, the event, the
  tick-checked copy, resets, the session text worker, `.ibt` files, and what is `unsafe`.
- [docs/FINDINGS.md](docs/FINDINGS.md): what is known about the simulator's data, each fact marked
  documented, observed or not observed yet, and the list of assumed facts with what would confirm
  them.

## Installing

`iRacing-Connector-X.Y.Z-setup.exe` (a release asset, with its `.sha256` file in `sha256sum`
format) installs the three programs for the current user, without administrator rights, into
`%LOCALAPPDATA%\Programs\Stintmate iRacing Connector\`, and registers the connector for Stintmate
(`%LOCALAPPDATA%\Stintmate\connectors\iracing.json`). Uninstall it from Windows' Installed apps;
the logs, the outbox and the settings (in the workspace) stay. The unattended mode, the progress
file and the exit statuses are in [docs/installer.md](docs/installer.md). To build it:
`scripts\build-installer.ps1` (needs Inno Setup 6, cargo-deny and cargo-about).

## Releases

A release of tag `vX.Y.Z` carries two assets: `iRacing-Connector-X.Y.Z-setup.exe` and
`iRacing-Connector-X.Y.Z-setup.exe.sha256`. The notes are the version's section of
[`CHANGELOG.md`](CHANGELOG.md). A maintainer runs the `release` workflow by hand (Actions,
`release`, Run workflow) from `main` with the tag: it must exist, be an ancestor of `main`, equal
`v` + the workspace version and have its `## <version>` section in the changelog. The workflow
builds the tag's tree, runs fmt, clippy, tests and cargo-deny, builds the installer
(`scripts/build-installer.ps1`), uploads the assets to a draft, downloads them again and compares
them. With `dry_run` (the default) the draft is deleted at the end; without it, it is published.

## Building and testing

The toolchain comes from `rust-toolchain.toml`.

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check licenses advisories bans sources
```

The tests that read recordings from a real machine are ignored; see
[docs/FINDINGS.md](docs/FINDINGS.md) for how to run them.

CI runs by hand only (Actions, `ci`); run the checks above locally before a merge.

## Licence and notices

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE). The constants and data layouts of
`crates/irsdk` follow the iRacing SDK header under the BSD 3-Clause licence:
[THIRD-PARTY-NOTICES-irsdk.txt](THIRD-PARTY-NOTICES-irsdk.txt).

## Contributing

Open a pull request. Every change adds a line under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md).
Report vulnerabilities as [SECURITY.md](SECURITY.md) says.
