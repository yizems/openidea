<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="src-tauri/icons/source/openidea-wordmark-dark.svg">
    <img src="src-tauri/icons/source/openidea-wordmark.svg" alt="OpenIDEA" height="56">
  </picture>
</p>

<p align="center">
  The IDE you know, rebuilt in Rust.
</p>

<p align="center">
  <a href="https://github.com/l0ng-ai/openidea/releases/latest"><b>Download</b></a>
  &nbsp;·&nbsp;
  <a href="#benchmark">Benchmark</a>
  &nbsp;·&nbsp;
  <a href="#build">Build</a>
  &nbsp;·&nbsp;
  <a href="https://discord.gg/2Zy4yn7gP">Discord</a>
</p>

https://github.com/user-attachments/assets/234dd54c-1255-4b3a-b35c-533d78d737c0

OpenIDEA brings the IntelliJ IDEA way of working — its keymap, its tool windows,
its search-everything habits — to a native app with a Rust core.

Editor, code insight, debugger, Git, terminal, database and HTTP clients,
for 60+ languages. Builds for macOS, Windows and Linux.

## Benchmark

Opening [freqtrade](https://github.com/freqtrade/freqtrade) (Python, ~770 files)
with warm caches, median of 5 runs:

|                             | OpenIDEA          | IntelliJ IDEA 2026.2.3 |
| --------------------------- | ----------------- | ---------------------- |
| Window with the project     | **0.9 s**         | 3.1 s                  |
| Ready (CPU settled)         | **1.1 s**         | 13.2 s                 |
| Memory (USS, all processes) | **169 MB**        | 2011 MB                |
| CPU time until ready        | **2.4 s**         | 47.9 s                 |
| Idle CPU                    | **4 %** of a core | 7 % of a core          |
| Quit                        | **0.3 s**         | 5.9 s                  |

A release build of OpenIDEA from `main` and IDEA with default settings and plugins, on
Windows 11, 32-core AMD, 62 GB RAM.
IDEA analyses Python more deeply, so "ready" doesn't mean the same work on
both sides. Both are measured from the outside, the same way: see
[`perf/compare`](perf/compare) to run it on your machine and projects.

## Build

```sh
pnpm install
pnpm tauri dev
```

Rust 1.96, Node 22, pnpm and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

Native notebook execution tests are opt-in and need two distinct Python environments
with `ipykernel` installed:

```sh
OPENIDEA_TEST_PYTHON=/first/env/bin/python \
OPENIDEA_TEST_PYTHON_SECOND=/second/env/bin/python \
cargo test -p ide-notebook --test kernel_runtime -- --ignored --test-threads=1
```

### Database drivers

SQLite, PostgreSQL, MySQL/MariaDB and SQL Server connect out of the box: their drivers
are pure Rust (`sqlx`) or bundled (`tiberius`), no extra software needed.

Oracle and Db2 have no such driver, so OpenIDEA connects to them through the system's
**ODBC driver manager** instead. This has two consequences:

- **Building (and running) OpenIDEA at all requires unixODBC to be installed**, even if
  you never connect to Oracle or Db2: `ide-db` links against it unconditionally.
  - macOS: `brew install unixodbc`
  - Debian/Ubuntu: `sudo apt install unixodbc`
  - Fedora/RHEL: `sudo dnf install unixODBC`
  - Windows: nothing to install, it ships with the OS.
- **Connecting to an actual Oracle or Db2 server additionally needs that vendor's own
  ODBC driver** (Oracle Instant Client's ODBC component, or IBM's Data Server Driver),
  which is proprietary and not bundled: install it separately, register it with
  `odbcinst -i -d` (Linux/macOS) or the ODBC Data Source Administrator (Windows), and
  enter its registered name as the data source's "ODBC driver" (Advanced tab) in
  OpenIDEA.

The live Oracle/Db2 integration tests are opt-in the same way, each needing a
throwaway database and an installed driver:

```sh
OPENIDEA_TEST_ORACLE=host:port:service:user:password:driver cargo test -p ide-db --test oracle
OPENIDEA_TEST_DB2=host:port:database:user:password:driver cargo test -p ide-db --test db2
```

## License

[Apache 2.0](LICENSE). Bundled third-party assets are listed in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

<sub>Not affiliated with JetBrains. IntelliJ IDEA is a trademark of JetBrains s.r.o.</sub>

## me

```
pnpm tauri build --target aarch64-apple-darwin

target/aarch64-apple-darwin/release/bundle/macos/OpenIDEA.app/Contents/MacOS/openidea: Mach-O 64-bit executable arm64

ditto -c -k --sequesterRsrc --keepParent \
  target/aarch64-apple-darwin/release/bundle/macos/OpenIDEA.app \
  OpenIDEA.zip
```
