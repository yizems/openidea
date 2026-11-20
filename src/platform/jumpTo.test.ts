import { describe, expect, it } from "vitest";
import {
  DEFAULT_JUMP_TO_CONFIG,
  JUMP_TO_APP_BY_ID,
  JUMP_TO_APPS,
  defaultExecutable,
  jumpToArgs,
} from "./jumpTo";

describe("jumpToArgs", () => {
  it("opens the project and focuses the file with JetBrains' --line syntax", () => {
    // Lines are 0-based; JetBrains expects a 1-based --line value.
    expect(jumpToArgs("jetbrains", "/work/app", "/work/app/src/main.ts", 11)).toEqual([
      "/work/app",
      "--line",
      "12",
      "--column",
      "1",
      "/work/app/src/main.ts",
    ]);
  });

  it("uses --goto for the VS Code family", () => {
    expect(jumpToArgs("vscode", "/work/app", "/work/app/src/main.ts", 6)).toEqual([
      "/work/app",
      "--goto",
      "/work/app/src/main.ts:7:1",
    ]);
  });

  it("clamps negative lines to 1", () => {
    expect(jumpToArgs("jetbrains", "/w", "/w/f", -5)[2]).toBe("1");
    expect(jumpToArgs("vscode", "/w", "/w/f", -5)[2]).toBe("/w/f:1:1");
  });
});

describe("defaultExecutable", () => {
  const app = (id: string) => JUMP_TO_APP_BY_ID.get(id)!;

  it("uses platform-specific launcher names", () => {
    expect(defaultExecutable(app("rustrover"), "windows")).toBe("rustrover64.exe");
    expect(defaultExecutable(app("rustrover"), "macos")).toBe("rustrover");
    expect(defaultExecutable(app("rustrover"), "linux")).toBe("rustrover");
    expect(defaultExecutable(app("vscode"), "macos")).toBe("code");
    expect(defaultExecutable(app("vscode"), "linux")).toBe("code");
    expect(defaultExecutable(app("vscodium"), "linux")).toBe("codium");
    expect(defaultExecutable(app("androidStudio"), "macos")).toBe("studio");
  });
});

describe("jumpTo app catalog", () => {
  it("covers the JetBrains IDEs and the VS Code family with unique ids", () => {
    const ids = JUMP_TO_APPS.map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toContain("idea");
    expect(ids).toContain("rustrover");
    expect(ids).toContain("vscode");
    expect(JUMP_TO_APPS.some((a) => a.family === "vscode")).toBe(true);
    expect(JUMP_TO_APPS.some((a) => a.family === "jetbrains")).toBe(true);
  });

  it("shows a mainstream default set and hides the rest", () => {
    expect(DEFAULT_JUMP_TO_CONFIG["idea"]!.show).toBe(true);
    expect(DEFAULT_JUMP_TO_CONFIG["vscode"]!.show).toBe(true);
    expect(DEFAULT_JUMP_TO_CONFIG["aqua"]!.show).toBe(false);
    expect(DEFAULT_JUMP_TO_CONFIG["idea"]!.path).toBe("");
  });
});
