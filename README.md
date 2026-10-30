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
  <a href="https://github.com/yizems/openidea/releases/latest"><b>Download</b></a>
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
  which is proprietary and not bundled.

#### Quick install (macOS/Linux)

[`scripts/install-db-driver.mjs`](scripts/install-db-driver.mjs) downloads the vendor's
ODBC driver and registers it with unixODBC in one step — no Oracle or IBM account
needed, it fetches the same unauthenticated files `pip install oracledb`/`ibm_db` do
internally. It's a single file with no npm dependencies: copy it anywhere and run it
with plain `node`.

```sh
node scripts/install-db-driver.mjs oracle
node scripts/install-db-driver.mjs db2
```

It installs into `~/.dbdriver/<oracle|db2>` and prints the exact driver name to paste
into OpenIDEA's "ODBC driver" field. Re-running without `--force` reuses the existing
download; add `--force` to redo it (e.g. after a version bump). Requires `tar` and
`unzip` (Linux) or `hdiutil` (macOS, built in) to unpack the vendor archive, besides
unixODBC itself. Behind a corporate proxy, it honors `HTTPS_PROXY`/`HTTP_PROXY`/
`ALL_PROXY`/`NO_PROXY` the same way curl/git do (tunneled by hand over Node's
built-in `http`/`tls`, no proxy-agent package).

This covers macOS and Linux, x86_64 and arm64. For anything else (Windows, other
architectures), or to install a specific version yourself, see the manual steps below.

#### Adding the Oracle driver manually

1. Download the **Basic** and **ODBC** packages for your platform from
   [Oracle Instant Client Downloads](https://www.oracle.com/database/technologies/instant-client/downloads.html)
   (an Oracle account is required). Unzip both into the same directory, e.g.
   `/opt/oracle/instantclient_21_8`.
2. Register the driver with unixODBC using the script the ODBC package ships with:

   ```sh
   cd /opt/oracle/instantclient_21_8
   ./odbc_update_ini.sh / "$(pwd)"
   ```

   On Windows, run the bundled `odbc_install.exe` instead; it registers the driver with
   the Windows ODBC Data Source Administrator.

   **macOS caveat**: the `odbc_update_ini.sh` shipped in at least the Instant Client 23
   ODBC package has a bug — it hardcodes the Linux library name (`libsqora.so.23.1`)
   even though the macOS package's actual file is `libsqora.dylib.23.1`. If `odbcinst -q -d`
   shows the wrong extension afterwards, fix the `Driver=` line in `odbcinst.ini`
   (`odbcinst -j` prints its path) by hand, or use the install script above instead.

3. Make sure the Oracle client libraries are on the loader path whenever OpenIDEA runs,
   e.g. `export LD_LIBRARY_PATH=/opt/oracle/instantclient_21_8:$LD_LIBRARY_PATH` (Linux)
   or the macOS/Windows equivalent — see the Instant Client README for your platform.
4. Run `odbcinst -q -d` (Linux/macOS) to confirm the driver is registered and note its
   exact name, e.g. `[Oracle 21 ODBC driver]`.

#### Adding the Db2 driver manually

1. Download the **IBM Data Server Driver for ODBC and CLI** from
   [IBM Fix Central](https://www.ibm.com/support/fixcentral/) (an IBM account is
   required). Extract it and run the installer it ships with (`installDSDriver` on
   Linux/macOS, the `.exe` on Windows); this both unpacks the client libraries and
   registers the driver with the system's ODBC driver manager.
2. Run `odbcinst -q -d` (Linux/macOS) or open the ODBC Data Source Administrator
   (Windows) to confirm the driver is registered and note its exact name, e.g.
   `[IBM DB2 ODBC DRIVER - DB2COPY1]`.

Exact script names and output differ by the driver version you download; follow the
README bundled with your download if it diverges from the above.

#### Configuring the data source in OpenIDEA

In **New ▸ Data Source ▸ Oracle** (or **Db2**):

- **ODBC driver**: the name printed by `install-db-driver.mjs`, or the one
  `odbcinst -q -d` showed after a manual install.
- **Host** / **Port**: the database server (Oracle default port `1521`, Db2 `50000`).
- **Database**: Oracle's service name (`ORCLPDB1`, …) or Db2's cataloged database
  alias/name — whatever you'd otherwise put after `host:port/` in a connection string.
- **User** / **Password**: as usual.

Then use **Test Connection** before saving: if the ODBC driver name is wrong or the
driver isn't on the loader path, this is where it will report it.

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
