# Usage

The connector is three programs. Stintmate starts and supervises them; you can also run the
control app by hand.

| Program | What it does |
|---|---|
| `iracing-connector-ui.exe` | The control app: a tray icon, the supervision of the recorder, the connector's own endpoint (`connector-iracing`) and the registration. |
| `iracing-connector-window.exe` | The window with the Services, Options and Logs pages. The control app starts it on demand; one window at most. |
| `iracing-recorder.exe` | The recorder, started hidden by the control app. It connects to Pitwall and reports whether iRacing runs. It reads no simulator data and records no laps yet. |

## What the recorder does today

- It connects to Pitwall, says hello as connector `iracing` and publishes the status
  `connector.status` whenever it changes (it looks once a second).
- `game_running` is true while the `iRacingSim64DX11.exe` process exists. The check opens the
  simulator's memory map by name without mapping it, and takes a process snapshot when the map
  appears, and at least every 60 seconds.
- `phase` is `null` while the simulator is not running and `"menus"` while it runs. The message
  is "Waiting for iRacing" or "iRacing is running; laps are not recorded by this version".
- It stops when the control app sets its stop event, and exits 0.

## Options

The control app prints its full list with an unknown option or no valid command; the main ones:

| Option | Meaning |
|---|---|
| `--tray` | Start hidden in the tray. |
| `--workspace <dir>`, `--allow-synced` | The Stintmate workspace to use, and permission to use one inside OneDrive. Without `--workspace` the folder comes from `STINTMATE_WORKSPACE`, then from `%APPDATA%\Stintmate\settings.json`. |
| `--register [--flavour <name>] [--uninstall <exe>]` | Write the descriptor `%LOCALAPPDATA%\Stintmate\connectors\iracing.json` and exit. It needs no workspace. It refuses (exit 1) a descriptor of another product. |
| `--unregister` | Remove the descriptor when it names this install; logs, outbox and settings stay. |
| `--read-registration <out-file>` | Write the registered launcher and uninstaller as `key=value` lines; exit 2 when there is none. |
| `--quit` | Ask the running control app to exit. |
| `--version` | Print `iracing-connector-ui <version>` and exit. |

An option the build does not know is ignored, so a newer Stintmate can pass one. Exit status:
0 done, 1 failed, 2 bad arguments, 3 workspace error, 4 managed by Stintmate (`--quit`).

The window accepts `--version` (prints `iracing-connector-window <version>`) and the workspace
options. The recorder accepts `--version`, `--hz <rate>`, `--workspace` and `--allow-synced`, and,
for tests and replay runs, `--sdk-map <name>` with `--sdk-event <name>` (both or neither): it then
opens that memory mapping and event instead of the simulator's and treats the simulator as
running. Any other option is exit 2, because only the control app starts it.

The recorder reads the simulator's shared memory and sends sessions, laps, setups, pit exits and
the other cars' laps to Pitwall through an outbox in `connectors\iracing\outbox\`. When it stops
while the simulator runs with a session open, it keeps the session open
(`connectors\iracing\session-resume.json`), and a recorder started later in the same session
continues it with the same session key.

## Data folder and logs

Everything the connector writes is under `<workspace>\connectors\iracing\`:

- `recorder.log` and `ui.log`: the logs of the recorder and the control app; the Logs page of
  the window shows them.
- `ui_config.json`: the options of the control app (the recorder rate and
  starting minimized). Starting with Windows is a value in the current user's `Run` registry key.

## Registration for development

```
cargo build --release
target\release\iracing-connector-ui.exe --register --flavour dev
```

Registering writes `iracing.json` in `%LOCALAPPDATA%\Stintmate\connectors\`; Stintmate finds the
connector there. `STINTMATE_CONNECTORS_DIR` replaces that folder, which is how the tests avoid
the real one. Setting `STINTMATE_DEV_INSTANCE=<tag>` gives a second instance its own mutex and
pipe names, so it shares nothing with an installed connector. Remove the descriptor with
`--unregister`.

## Related

- [architecture.md](architecture.md): the `irsdk` reader.
- [FINDINGS.md](FINDINGS.md): what is known about the simulator's data.
