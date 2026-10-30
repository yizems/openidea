#!/usr/bin/env node
/**
 * Downloads the Oracle or Db2 ODBC driver and registers it with the system's
 * unixODBC driver manager (odbcinst), so it shows up for OpenIDEA's "ODBC driver"
 * data source field.
 *
 * Standalone, single-file, zero npm dependencies: plain Node.js built-ins only
 * (https/fs/child_process/...). Copy this one file anywhere and run it with a
 * plain `node` — no install step, no project checkout required.
 *
 * Usage:
 *   node install-db-driver.mjs oracle
 *   node install-db-driver.mjs db2
 *   node install-db-driver.mjs oracle --force   (re-download / re-extract even if already installed)
 *
 * Requirements:
 *   - macOS or Linux, x86_64 or arm64 (unixODBC is not a Windows thing).
 *   - unixODBC installed (`brew install unixodbc` / `apt install unixodbc` / `dnf install unixODBC`),
 *     so that the `odbcinst` command is on PATH.
 *   - `tar` and `unzip` (Linux) or `hdiutil` (macOS, built in) to unpack the vendor archive.
 *
 * What this does NOT do:
 *   - It does not need an Oracle or IBM account: both vendors publish the files this
 *     script fetches at stable, unauthenticated URLs (the same ones `pip install
 *     oracledb`/`ibm_db` and `npm install ibm_db` use internally to fetch their own
 *     native client). See the README's "Database drivers" section for background.
 *   - It does not touch OpenIDEA's data source settings: after it prints a driver
 *     name, paste that into the "ODBC driver" field yourself (General tab, Oracle/
 *     Db2 data sources).
 *
 * Behind a corporate proxy: downloads honor HTTPS_PROXY / HTTP_PROXY / ALL_PROXY and
 * NO_PROXY (same convention as curl/git), tunneled by hand over Node's http/tls
 * built-ins — no proxy-agent package involved.
 */

import { spawnSync } from "node:child_process";
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { Agent as HttpsAgent, get as httpsGet } from "node:https";
import { request as httpRequest } from "node:http";
import { connect as tlsConnect } from "node:tls";
import os from "node:os";
import path from "node:path";

const HOME = os.homedir();
const INSTALL_ROOT = path.join(HOME, ".dbdriver");

// ---------------------------------------------------------------------------
// Download sources
//
// These are the vendors' own permanent, unauthenticated direct-download URLs —
// the same ones `pip install oracledb`/`ibm_db` and `npm install ibm_db` fetch
// internally. Verified by hand on 2026-10-30; if a vendor reorganizes their
// download site this table is the only thing that needs updating.
//
// Oracle ships the ODBC shim (libsqora.*) and the network client it depends on
// (libclntsh.*, in the "Basic" package) as two separate archives that must be
// merged into one directory. Db2's clidriver tarball is self-contained.
// ---------------------------------------------------------------------------

const ORACLE_PERMALINKS = {
  "darwin-arm64": {
    archive: "dmg",
    urls: [
      "https://download.oracle.com/otn_software/mac/instantclient/instantclient-basic-macos-arm64.dmg",
      "https://download.oracle.com/otn_software/mac/instantclient/instantclient-odbc-macos-arm64.dmg",
    ],
  },
  // Oracle stopped updating Intel Mac builds after the Apple Silicon transition:
  // 19.16 is the last one published, pinned (no "latest" alias exists for it).
  "darwin-x64": {
    archive: "dmg",
    urls: [
      "https://download.oracle.com/otn_software/mac/instantclient/instantclient-basic-macos.x64-19.16.0.0.0dbru.dmg",
      "https://download.oracle.com/otn_software/mac/instantclient/instantclient-odbc-macos.x64-19.16.0.0.0dbru.dmg",
    ],
  },
  "linux-x64": {
    archive: "zip",
    urls: [
      "https://download.oracle.com/otn_software/linux/instantclient/instantclient-basic-linuxx64.zip",
      "https://download.oracle.com/otn_software/linux/instantclient/instantclient-odbc-linuxx64.zip",
    ],
  },
  // No "latest" alias for Linux ARM either: 19.32 is the newest published build.
  "linux-arm64": {
    archive: "zip",
    urls: [
      "https://download.oracle.com/otn_software/linux/instantclient/1932000/instantclient-basic-linux.arm64-19.32.0.0.0dbru.zip",
      "https://download.oracle.com/otn_software/linux/instantclient/1932000/instantclient-odbc-linux.arm64-19.32.0.0.0dbru.zip",
    ],
  },
};

const DB2_PERMALINKS = {
  "darwin-arm64":
    "https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/macarm64_odbc_cli.tar.gz",
  "darwin-x64":
    "https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/macos64_odbc_cli.tar.gz",
  "linux-x64":
    "https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/linuxx64_odbc_cli.tar.gz",
  // IBM does not publish a Linux aarch64 clidriver through this channel.
};

const DRIVERS = {
  oracle: {
    displayName: "OpenIDEA Oracle ODBC Driver",
    description: "Oracle Instant Client ODBC driver, installed by install-db-driver.mjs",
    // Matches libsqora.so.23.1 (Linux) / libsqora.dylib.23.1 (macOS), whatever the
    // version suffix: Oracle bumps it with every Instant Client release.
    libraryPattern: /^libsqora\.(so|dylib)(\.[\w.]+)?$/,
    permalinks: ORACLE_PERMALINKS,
  },
  db2: {
    displayName: "OpenIDEA Db2 ODBC Driver",
    description: "IBM Data Server Driver for ODBC and CLI, installed by install-db-driver.mjs",
    // CLI and ODBC are the same shared library in the clidriver package.
    libraryPattern: /^libdb2\.(so|dylib)$/,
    permalinks: DB2_PERMALINKS,
  },
};

// ---------------------------------------------------------------------------

function fail(message) {
  console.error(`\n✖ ${message}\n`);
  process.exit(1);
}

function info(message) {
  console.log(message);
}

function platformKey() {
  return `${process.platform}-${process.arch}`;
}

function checkTool(command, installHint) {
  const r = spawnSync(command, ["--version"], { stdio: "ignore" });
  if (r.error || r.status === null) fail(`\`${command}\` was not found on PATH. ${installHint}`);
}

// ---------------------------------------------------------------------------
// Proxy support (HTTPS_PROXY / HTTP_PROXY / ALL_PROXY / NO_PROXY, same
// convention as curl/git). Node's http(s) modules never read these on their
// own, so every HTTPS request is tunneled by hand through the proxy with the
// CONNECT method, then wrapped in TLS — the same technique the (deliberately
// unused here) `https-proxy-agent` npm package uses.
// ---------------------------------------------------------------------------

function parseNoProxy() {
  const raw = process.env.NO_PROXY ?? process.env.no_proxy ?? "";
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function hostMatchesNoProxy(hostname, noProxyList) {
  return noProxyList.some((entry) => {
    if (entry === "*") return true;
    const suffix = entry.startsWith(".") ? entry : `.${entry}`;
    return hostname === entry || hostname.endsWith(suffix);
  });
}

/** The proxy to use for `targetUrl`, or `null` for a direct connection. */
function resolveProxyUrl(targetUrl) {
  const target = new URL(targetUrl);
  if (hostMatchesNoProxy(target.hostname, parseNoProxy())) return null;
  const candidates =
    target.protocol === "https:"
      ? [process.env.HTTPS_PROXY, process.env.https_proxy, process.env.ALL_PROXY, process.env.all_proxy]
      : [process.env.HTTP_PROXY, process.env.http_proxy, process.env.ALL_PROXY, process.env.all_proxy];
  const proxy = candidates.find((v) => v);
  return proxy ? new URL(proxy) : null;
}

/**
 * An https.Agent that tunnels every connection through an HTTP proxy via the
 * CONNECT method (RFC 7231 §4.3.6), then layers TLS on top of the tunnel.
 */
class HttpConnectProxyAgent extends HttpsAgent {
  constructor(proxyUrl, agentOptions) {
    super(agentOptions);
    this.proxyUrl = proxyUrl;
  }

  createConnection(options, callback) {
    const proxy = this.proxyUrl;
    const targetHost = options.host;
    const targetPort = options.port || 443;
    const headers = { Host: `${targetHost}:${targetPort}` };
    if (proxy.username || proxy.password) {
      const creds = `${decodeURIComponent(proxy.username)}:${decodeURIComponent(proxy.password)}`;
      headers["Proxy-Authorization"] = `Basic ${Buffer.from(creds).toString("base64")}`;
    }
    const connectReq = httpRequest({
      host: proxy.hostname,
      port: proxy.port || 80,
      method: "CONNECT",
      path: `${targetHost}:${targetPort}`,
      headers,
    });
    connectReq.on("connect", (res, socket) => {
      if (res.statusCode !== 200) {
        socket.destroy();
        callback(new Error(`proxy CONNECT ${targetHost}:${targetPort} failed: ${res.statusCode} ${res.statusMessage}`));
        return;
      }
      const tlsSocket = tlsConnect({ socket, servername: options.servername ?? targetHost });
      tlsSocket.once("secureConnect", () => callback(null, tlsSocket));
      tlsSocket.once("error", callback);
    });
    connectReq.on("error", callback);
    connectReq.end();
  }
}

/** Downloads `url` to `destPath`, following redirects. Resolves to the final byte count. */
function download(url, destPath, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    const proxyUrl = resolveProxyUrl(url);
    const options = proxyUrl ? { agent: new HttpConnectProxyAgent(proxyUrl) } : {};
    if (proxyUrl) info(`  (via proxy ${proxyUrl.protocol}//${proxyUrl.host})`);
    const req = httpsGet(url, options, (res) => {
      const { statusCode, headers } = res;
      if (statusCode >= 300 && statusCode < 400 && headers.location && redirectsLeft > 0) {
        res.resume();
        const next = new URL(headers.location, url).toString();
        download(next, destPath, redirectsLeft - 1).then(resolve, reject);
        return;
      }
      if (statusCode !== 200) {
        res.resume();
        reject(new Error(`HTTP ${statusCode} fetching ${url}`));
        return;
      }
      const total = Number(headers["content-length"] ?? 0);
      let received = 0;
      const file = createWriteStream(destPath);
      res.on("data", (chunk) => {
        received += chunk.length;
        if (total > 0) {
          const pct = Math.floor((received / total) * 100);
          process.stdout.write(`\r  ${path.basename(destPath)}: ${pct}% (${(received / 1e6).toFixed(1)} MB)`);
        }
      });
      res.pipe(file);
      file.on("finish", () => {
        file.close(() => {
          process.stdout.write("\n");
          resolve(received);
        });
      });
      file.on("error", reject);
    });
    req.on("error", reject);
  });
}

function run(command, args, options = {}) {
  const r = spawnSync(command, args, { stdio: "pipe", encoding: "utf8", ...options });
  if (r.error) throw r.error;
  if (r.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed (exit ${r.status}):\n${r.stderr || r.stdout}`);
  }
  return r;
}

function extractZip(archivePath, destDir) {
  checkTool("unzip", "Install it (e.g. `sudo apt install unzip` / `sudo dnf install unzip`).");
  run("unzip", ["-q", "-o", archivePath, "-d", destDir]);
}

function extractTarGz(archivePath, destDir) {
  checkTool("tar", "`tar` is expected to be preinstalled on macOS and Linux.");
  run("tar", ["-xzf", archivePath, "-C", destDir]);
}

/** Mounts a macOS .dmg and copies its contents into `destDir` (merging, not replacing). */
function extractDmg(archivePath, destDir) {
  const mountPoint = mkdtempSync(path.join(os.tmpdir(), "openidea-dmg-"));
  run("hdiutil", ["attach", "-nobrowse", "-quiet", "-mountpoint", mountPoint, archivePath]);
  try {
    // -f: Basic and ODBC packages both ship an install_ic.sh/INSTALL_IC_README.txt
    // with the same read-only mode; merging the second archive in must overwrite them.
    run("cp", ["-Rf", `${mountPoint}/.`, destDir]);
  } finally {
    run("hdiutil", ["detach", mountPoint, "-quiet", "-force"]);
    rmSync(mountPoint, { recursive: true, force: true });
  }
}

function extractInto(archivePath, archiveKind, destDir) {
  mkdirSync(destDir, { recursive: true });
  if (archiveKind === "zip") extractZip(archivePath, destDir);
  else if (archiveKind === "tar.gz") extractTarGz(archivePath, destDir);
  else if (archiveKind === "dmg") extractDmg(archivePath, destDir);
  else fail(`internal error: unknown archive kind ${archiveKind}`);
}

/** Finds the first file under `dir` (depth-bounded) whose name matches `pattern`. */
function findLibrary(dir, pattern, maxDepth = 4) {
  const stack = [{ dir, depth: 0 }];
  while (stack.length) {
    const { dir: current, depth } = stack.pop();
    let entries;
    try {
      entries = readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory() && depth < maxDepth) stack.push({ dir: full, depth: depth + 1 });
      else if (entry.isFile() && pattern.test(entry.name)) return full;
    }
  }
  return null;
}

function registerWithOdbcinst(displayName, description, libraryPath) {
  checkTool(
    "odbcinst",
    "Install unixODBC first (`brew install unixodbc` on macOS, `apt install unixodbc` / `dnf install unixODBC` on Linux).",
  );
  const iniPath = path.join(os.tmpdir(), `openidea-odbc-driver-${Date.now()}.ini`);
  const ini = `[${displayName}]\nDescription=${description}\nDriver=${libraryPath}\nFileUsage=1\n`;
  writeFileSync(iniPath, ini, "utf8");
  try {
    const r = spawnSync("odbcinst", ["-i", "-d", "-f", iniPath], { stdio: "pipe", encoding: "utf8" });
    if (r.status !== 0) {
      const detail = (r.stderr || r.stdout || "").trim();
      fail(
        `odbcinst failed to register the driver${detail ? `:\n${detail}` : "."}\n` +
          "  On Linux, odbcinst.ini is often root-owned: try re-running this script with sudo.",
      );
    }
  } finally {
    rmSync(iniPath, { force: true });
  }
}

async function downloadAll(urls, destDir) {
  mkdirSync(destDir, { recursive: true });
  const paths = [];
  for (const url of urls) {
    const dest = path.join(destDir, path.basename(new URL(url).pathname));
    info(`Downloading ${url}`);
    await download(url, dest);
    paths.push(dest);
  }
  return paths;
}

async function installOracle(destDir, force) {
  const pkg = ORACLE_PERMALINKS[platformKey()];
  if (!pkg) fail(`Oracle Instant Client has no published build for ${platformKey()}.`);
  const finalDir = path.join(destDir, "instantclient");
  if (existsSync(finalDir) && !force) {
    info(`${finalDir} already exists (use --force to redo). Looking for the driver library there…`);
  } else {
    rmSync(finalDir, { recursive: true, force: true });
    const stagingDir = mkdtempSync(path.join(destDir, ".staging-"));
    try {
      const downloadDir = path.join(stagingDir, "download");
      const archives = await downloadAll(pkg.urls, downloadDir);
      const extractDir = path.join(stagingDir, "extract");
      for (const archive of archives) {
        info(`Extracting ${path.basename(archive)}`);
        extractInto(archive, pkg.archive, extractDir);
      }
      // Both archives unpack into one version-named subdirectory (instantclient_XX_Y);
      // flatten it away so `finalDir` always has a stable, version-independent layout.
      const entries = readdirSync(extractDir, { withFileTypes: true }).filter((e) => e.isDirectory());
      const sourceDir = entries.length === 1 ? path.join(extractDir, entries[0].name) : extractDir;
      renameSync(sourceDir, finalDir);
    } finally {
      rmSync(stagingDir, { recursive: true, force: true });
    }
  }
  const library = findLibrary(finalDir, DRIVERS.oracle.libraryPattern);
  if (!library) fail(`Could not find libsqora.(so|dylib) under ${finalDir} after extracting.`);
  return library;
}

async function installDb2(destDir, force) {
  const url = DB2_PERMALINKS[platformKey()];
  if (!url) fail(`IBM does not publish a Db2 clidriver for ${platformKey()} through this channel.`);
  const finalDir = path.join(destDir, "clidriver");
  if (existsSync(finalDir) && !force) {
    info(`${finalDir} already exists (use --force to redo). Looking for the driver library there…`);
  } else {
    rmSync(finalDir, { recursive: true, force: true });
    const stagingDir = mkdtempSync(path.join(destDir, ".staging-"));
    try {
      const downloadDir = path.join(stagingDir, "download");
      const [archive] = await downloadAll([url], downloadDir);
      const extractDir = path.join(stagingDir, "extract");
      info(`Extracting ${path.basename(archive)}`);
      extractInto(archive, "tar.gz", extractDir);
      // The tarball's own top-level entry is already named "clidriver".
      renameSync(path.join(extractDir, "clidriver"), finalDir);
    } finally {
      rmSync(stagingDir, { recursive: true, force: true });
    }
  }
  const library = findLibrary(finalDir, DRIVERS.db2.libraryPattern);
  if (!library) fail(`Could not find libdb2.(so|dylib) under ${finalDir} after extracting.`);
  return library;
}

async function main() {
  const [, , driverArg, ...flags] = process.argv;
  const force = flags.includes("--force");
  if (!driverArg || !(driverArg in DRIVERS)) {
    console.log("Usage: node install-db-driver.mjs <oracle|db2> [--force]");
    process.exit(driverArg ? 1 : 0);
  }
  if (process.platform !== "darwin" && process.platform !== "linux") {
    fail("This script is for macOS and Linux only: unixODBC/odbcinst is not a Windows thing.");
  }

  const driver = DRIVERS[driverArg];
  const destDir = path.join(INSTALL_ROOT, driverArg);
  mkdirSync(destDir, { recursive: true });

  info(`Installing the ${driverArg} ODBC driver into ${destDir} …`);
  const library = driverArg === "oracle" ? await installOracle(destDir, force) : await installDb2(destDir, force);

  info(`Registering "${driver.displayName}" with unixODBC (Driver=${library}) …`);
  registerWithOdbcinst(driver.displayName, driver.description, library);

  console.log(`\n✔ Done. In OpenIDEA's New Data Source ▸ ${driverArg === "oracle" ? "Oracle" : "IBM Db2"} dialog,`);
  console.log(`  set "ODBC driver" to:\n\n      ${driver.displayName}\n`);
}

main().catch((e) => fail(e.message ?? String(e)));
