/**
 * Open In: the editor's "Jump To" submenu launches another IDE on the current
 * project and focuses the current file and line in it. JetBrains IDEs and
 * VS Code-family editors take different command lines, so each app knows how
 * to build its arguments. The executable paths and which apps appear in the
 * menu are user settings, stored as one value at `plugins.jumpTo`.
 */
import { call, commands } from "./ipc";
import { usePlatform } from "./platformInfo";
import { useProject } from "./project";

export type JumpToFamily = "jetbrains" | "vscode";

export interface JumpToApp {
  id: string;
  name: string;
  family: JumpToFamily;
  /** Bare executable name by OS, used when no path is configured. */
  executables: { windows: string; macos: string; linux: string };
}

/** JetBrains IDEs first, then the VS Code family. Order is the menu order. */
const cli = (name: string) => ({ windows: `${name}64.exe`, macos: name, linux: name });
const vscodeCli = (windows: string, name: string) => ({ windows, macos: name, linux: name });

export const JUMP_TO_APPS: JumpToApp[] = [
  { id: "idea", name: "IntelliJ IDEA", family: "jetbrains", executables: cli("idea") },
  { id: "pycharm", name: "PyCharm", family: "jetbrains", executables: cli("pycharm") },
  { id: "webstorm", name: "WebStorm", family: "jetbrains", executables: cli("webstorm") },
  { id: "goland", name: "GoLand", family: "jetbrains", executables: cli("goland") },
  { id: "clion", name: "CLion", family: "jetbrains", executables: cli("clion") },
  { id: "rider", name: "Rider", family: "jetbrains", executables: cli("rider") },
  { id: "rubymine", name: "RubyMine", family: "jetbrains", executables: cli("rubymine") },
  { id: "phpstorm", name: "PhpStorm", family: "jetbrains", executables: cli("phpstorm") },
  { id: "datagrip", name: "DataGrip", family: "jetbrains", executables: cli("datagrip") },
  { id: "rustrover", name: "RustRover", family: "jetbrains", executables: cli("rustrover") },
  { id: "androidStudio", name: "Android Studio", family: "jetbrains", executables: cli("studio") },
  { id: "dataSpell", name: "DataSpell", family: "jetbrains", executables: cli("dataspell") },
  { id: "aqua", name: "Aqua", family: "jetbrains", executables: cli("aqua") },
  { id: "appcode", name: "AppCode", family: "jetbrains", executables: cli("appcode") },
  { id: "vscode", name: "Visual Studio Code", family: "vscode", executables: vscodeCli("Code.exe", "code") },
  { id: "cursor", name: "Cursor", family: "vscode", executables: vscodeCli("Cursor.exe", "cursor") },
  { id: "vscodium", name: "VSCodium", family: "vscode", executables: vscodeCli("VSCodium.exe", "codium") },
];

/** The launcher name for the current OS, used when no path is configured. */
export function defaultExecutable(app: JumpToApp, os = usePlatform.getState().os): string {
  return app.executables[os === "windows" ? "windows" : os === "linux" ? "linux" : "macos"];
}

export const JUMP_TO_APP_BY_ID = new Map(JUMP_TO_APPS.map((a) => [a.id, a]));

/** One app's persisted settings: an absolute executable path and menu visibility. */
export interface JumpToAppConfig {
  path: string;
  show: boolean;
}

export type JumpToConfig = Record<string, JumpToAppConfig>;

/** Shown by default when no setting exists: the mainstream IDEs. */
export const DEFAULT_JUMP_TO_CONFIG: JumpToConfig = Object.fromEntries(
  JUMP_TO_APPS.map((a) => [
    a.id,
    { path: "", show: ["idea", "pycharm", "webstorm", "goland", "clion", "rustrover", "vscode"].includes(a.id) },
  ]),
);

const asConfig = (raw: unknown): JumpToConfig => {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const out: JumpToConfig = {};
  for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!JUMP_TO_APP_BY_ID.has(id) || !value || typeof value !== "object") continue;
    const v = value as Record<string, unknown>;
    out[id] = { path: typeof v.path === "string" ? v.path : "", show: typeof v.show === "boolean" ? v.show : false };
  }
  return out;
};

/** The effective config: defaults overlaid with the user's values. */
export function jumpToConfig(): JumpToConfig {
  const stored = asConfig(useProject.getState().settings?.plugins?.jumpTo);
  const merged: JumpToConfig = {};
  for (const app of JUMP_TO_APPS) merged[app.id] = { ...DEFAULT_JUMP_TO_CONFIG[app.id]!, ...stored[app.id] };
  return merged;
}

/**
 * Command-line arguments that open `folder` as the project and focus `file` at
 * `line` (0-based, like IPC lines). JetBrains launchers take the project directory
 * followed by `--line`/`--column` and the file path (the form EditorJumper uses):
 *   idea64.exe <folder> --line <line> --column <column> <file>
 * The old `<folder> <file>:<line>` form makes IDEs reject `file:line` as a directory.
 * VS Code family: `code <folder> --goto <file>:<line>:<column>`.
 */
export function jumpToArgs(family: JumpToFamily, folder: string, file: string, line: number): string[] {
  const at = Math.max(1, Math.floor(line) + 1);
  return family === "jetbrains"
    ? [folder, "--line", String(at), "--column", "1", file]
    : [folder, "--goto", `${file}:${at}:1`];
}

export interface JumpToTarget {
  appId: string;
  folder: string;
  file: string;
  line: number;
}

/** Resolve a configured app and launch it on the target. */
export async function jumpTo(target: JumpToTarget): Promise<void> {
  const app = JUMP_TO_APP_BY_ID.get(target.appId);
  if (!app) return;
  const config = jumpToConfig()[target.appId];
  const executable = config?.path?.trim() || defaultExecutable(app);
  const args = jumpToArgs(app.family, target.folder, target.file, target.line);
  await call(commands.jumpToOpen(executable, args));
}
