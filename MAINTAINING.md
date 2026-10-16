# Maintaining and adapting Vessel Trend Manager

The current version uses Rust, eframe/egui for the GUI and rusqlite with bundled
SQLite. Four application modules, one test file and one build script.
The GUI is in English or Italian, chosen with the flags in the header; the
choice is stored in the preferences. Names, units and groups come from the archives.

## Development environment

For the Windows package: Windows 10/11 x64, Rust with rustfmt and Clippy, and
Visual Studio Build Tools with C++ and the Windows SDK. The local build uses
Rust 1.99; Cargo.toml declares Rust 1.88 as the minimum version.
Official installation: [Rust](https://www.rust-lang.org/tools/install).

On a new PC, after installing the tools:

```powershell
cargo fetch --locked
cargo run --locked --offline -- --data-dir "D:\AmsExports"
```

In the local development copy Rust is kept in `.rust`, excluded from the
repository. `build.ps1` detects it automatically. To use Cargo directly:

```powershell
$env:CARGO_HOME = Join-Path $PWD.Path '.rust\cargo'
$env:RUSTUP_HOME = Join-Path $PWD.Path '.rust\rustup'
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
```

The network is needed to download the toolchain and dependencies. With the cache
ready, tests and builds use `--locked --offline`. Keep `Cargo.lock`; update it
only together with a dependency review. The operator PC needs no Rust, Python
or SQLite service. A graphics driver compatible with the eframe OpenGL renderer
is required. Other operating systems have not been tested.

## Where to change the source

| Need | File and entry point |
|---|---|
| Startup, arguments and headless check | `src/main.rs`: `options`, `request`, `run` |
| UI language and translated texts | `src/main.rs`: `tr!`, `t`, `set_italian`; each text has an English and an Italian version |
| Texts, colours, panels and charts | `src/gui.rs`: `App`, `CachedPlot`, `RawView`, `charts`, `flag` |
| Analysis of the visible range | `src/gui.rs`: `Summary`, `measure`, `span`, `save_csv`, `csv_cell` |
| Isolation Forest | `src/gui.rs`: `Forest`, `reading_features`, `threshold`, `train_forest`, `unusual_readings`, `FOREST_*` |
| Memory reuse of loaded readings | `src/archive.rs`: `narrow`, `extend`, `Series::absorb`; `src/gui.rs`: `App::cache` |
| Quick periods | `src/gui.rs`: `PERIODS`, `set_period` |
| New data schema | `src/archive.rs`: `scan`, `load`, `TAG_SQL`, `sample`, `nominal` |
| Synthetic demo archives | `src/demo.rs`: `ensure`, `sample`, `FEATURED` |
| Load limits | `src/archive.rs`: `MAX_TAGS`, `MAX_ROWS`, `MAX_BUCKETS`, `MAX_RAW`, `MAX_SECONDS`, `MAX_TOTAL_SECONDS`, `Deadline` |
| Application icon | `assets/icon.png` (window), `assets/icon.ico` (executable, via `build.rs`) |
| Packages and public files | `build.ps1`: `publicTrend`; `.gitignore` |

From/To accept custom dates, with the end excluded and no fixed 30-day limit.
Each request keeps four signals and 50 million rows as limits, and stops after
120 seconds without progress (rows read or an archive completed) or 600 seconds
in all; catalog scans keep 120 seconds in all.
Each signal has at most 800 buckets; each keeps up to four valid samples:
first, last, minimum and maximum. Daily loads up to 2 million readings also keep
every reading in compact form (`Raw`), and the chart draws the visible range from
them per pixel column (`RawView`). A shorter period of loaded signals is served
from memory (`narrow`); a wider one reads only what memory lacks (`extend`).

`Manager` uses one thread, one pending request and one pending result.
A new request cancels the previous one; the GUI accepts only the current
generation. Do not run SQLite queries in the `update` loop. Keep the SQLite
interrupt check, streaming reads and limits in new readers.
Reduce the data before passing it to the chart.

Archive workers retain at most one batch of four partial results. Input caps are
20,000 archive files, 10,000 catalog entries, 1,024 UTF-8 bytes per metadata field
and 10,000 dynamic IDs per signal. SQLite uses a shared 64 MiB hard heap limit and
memory-only temporary storage. Views and triggers are disabled; expected archive
objects must be ordinary tables. These are defensive rejection limits, not a cap
on the entire GUI process. Reassess them with synthetic fixtures when adapting a schema.

If `MAX_TAGS` is increased, also adapt the `COLORS` palette, texts and GUI
layout. Test limit increases on the target PC. Operating limits for signals
are a future local configuration of the vessel.

## Adapting another vessel

With the same AMS export schema and standard names, just choose the new archives.
The tables used are `TagConfig`, `TagType`, `AlarmGroup`, `TagConfigDynamic`
and `TagValue`. The group link uses `AlarmGroupNumber`, not `Id`.
`TagValue` holds timestamp, value and quality flags; aggregates also have
`Min`, `Max` and `IntervalMarker`.

Files are searched in the chosen folder and in the immediate subfolders `days`
and `weeks`. Expected names: `Day_YYYY-MM-DD.s3db` and
`Week_from_YYYY-MM-DD_to_YYYY-MM-DD.s3db`. Nominal intervals avoid counting
again old states copied into later exports. Daily and weekly data stay
separate. Timestamps are not converted between time zones.

For another format adapt `scan` and `load`, keeping the
`Catalog` / `Request` / `Trends` contract, quality and provenance. An additional
format that is really needed can have a small reader module; do not duplicate the GUI.
Recreate the views when the tags change. Do not infer units, aggregation
intervals or the meaning of undocumented digital states.

## Verifying a change

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --locked --offline
cargo run --locked --offline -- --smoke-test
cargo run --locked --offline -- --smoke-test --demo --source weekly --all
cargo run --locked --offline -- --smoke-test --data-dir "D:\AmsExports" --source weekly --days 90
cargo run --locked --offline -- --check --data-dir "D:\AmsExports" --source weekly --all
```

Tests create only temporary synthetic databases. The smoke test checks the
hidden GUI and does not read or save preferences. `--check` runs the retrieval
without GUI and prints JSON: it can contain operating values, so keep it private.
The exit code must be zero. `--tags A,B` chooses the signals;
`--start "2025-01-01 00:00"` and `--end` bound a precise interval.
Without explicit tags the checks choose four analog signals.

Manually check search, scrolling, zoom, cancellation, views, readability and
both languages. Zoom alone does not reread the data: narrow From/To and reload.
Reduced digital signals are not joined to invent durations; native samples more
than 15 minutes apart are not connected. This plotting choice is not a diagnosis.

## Rebuilding and distributing the executable

```powershell
.\build.ps1
```

The script checks formatting, Clippy and tests, then compiles the release. It
produces in `dist` the program `VesselTrendManager.exe`, the Windows
ZIP, the source ZIP and their SHA256 files. It includes the operator TXT and the
component licenses; it does not copy exports, preferences, environments or logs.
The MSVC runtime is statically linked. Development paths in Rust messages are
remapped; build releases only with `build.ps1`, since a direct `cargo build`
keeps personal paths in the executable. The executable loads DLLs only from System32.

`dist` keeps only the latest release. The build is staged in `dist.new` and,
only when complete, replaces the previous release items in `dist`. Any other
file there (for example `preferenze.json` written next to the executable) stops
the build and nothing is deleted. Source and output paths with redirected parent
directories are refused.

For sources only, without a compiler:

```powershell
.\build.ps1 -SourceOnly
```

Test the extracted package on the target PC. Executable smoke test:

```powershell
$processTrend = Start-Process -FilePath '.\dist\VesselTrendManager\VesselTrendManager.exe' -ArgumentList '--smoke-test' -WindowStyle Hidden -Wait -PassThru
if ($processTrend.ExitCode -ne 0) { throw 'Executable smoke test failed' }
```

On the operator PC, install the new version in a separate folder. With the program closed,
`preferenze.json` can be copied from the previous version: compatible views
are also read from the former Python version. The file can contain local data.

## Publication

Use this folder as the repository, never the exports folder.
`.gitignore` and `publicTrend` in `build.ps1` allow only the listed files:
update them together when adding a public file. Check
`git status --short` and `git diff --cached`; files already tracked and history
need a separate check.

Exclude real data, screenshots, preferences, operating configurations, vessel
names, personal paths, logs, `.private`, `.rust`, `target` and `dist`.
Rules for automated coding agents are in [AGENTS.md](AGENTS.md).
The script collects the dependency licenses. New dependencies and executables
must be reviewed before distribution. The project is MIT-licensed (`LICENSE`);
publication authorization is still to be confirmed. No automatic upload.

References: [eframe](https://docs.rs/eframe/0.33.3/eframe/),
[rusqlite](https://docs.rs/rusqlite/0.37.0/rusqlite/),
[local Rustup](https://rust-lang.github.io/rustup/installation/index.html).
