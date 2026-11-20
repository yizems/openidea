/**
 * Jump To: the editor's right-click "Jump To" group launches another IDE on
 * the current project, opening the same file at the same line. Which editors
 * appear and where their executables live are managed in Settings › Tools ›
 * Jump To. See `@/platform/jumpTo` for the app list and argument building.
 */
import { getContext } from "@/platform/context";
import { registerActions } from "@/platform/actions";
import { tx } from "@/platform/i18n";
import { useProject } from "@/platform/project";
import { JUMP_TO_APPS, jumpTo, jumpToConfig, type JumpToTarget } from "@/platform/jumpTo";
import { type MenuItem } from "@/ui";
import { notifications, registerMenuGroup } from "@/workbench/contributions";
import { activateSettingsPages } from "./settingsPages";

export const JUMP_TO_OPEN = "jumpTo.open";
export { JUMP_TO_SETTINGS_PAGE } from "./settingsPages";

function targetFromArg(arg: unknown): JumpToTarget | null {
  const t = arg as Partial<JumpToTarget> | undefined;
  if (!t || typeof t !== "object") return null;
  if (typeof t.appId !== "string" || typeof t.file !== "string") return null;
  const folder = typeof t.folder === "string" ? t.folder : (useProject.getState().project?.root ?? "");
  if (!folder) return null;
  return { appId: t.appId, folder, file: t.file, line: typeof t.line === "number" ? t.line : 0 };
}

function jumpToMenu(path: string, line: number): MenuItem {
  const config = jumpToConfig();
  const folder = useProject.getState().project?.root ?? "";
  const items: MenuItem[] = JUMP_TO_APPS.filter((app) => config[app.id]?.show).map((app) => ({
    kind: "action",
    id: `jumpTo.${app.id}`,
    action: JUMP_TO_OPEN,
    arg: { appId: app.id, folder, file: path, line } satisfies JumpToTarget,
    label: app.name,
  }));
  return {
    label: "Jump To",
    icon: "arrow-right",
    submenu: items.length ? items : [{ kind: "item", label: tx`No editors configured`, disabled: true }],
  };
}

export function activate(): () => void {
  const disposers = [
    registerActions([
      {
        id: JUMP_TO_OPEN,
        title: "Open in Another IDE",
        description: "Open the current project and file in another IDE, at the same line",
        category: "Tools",
        icon: "arrow-right",
        when: "projectOpen && textEditorActive",
        keywords: ["jetbrains", "idea", "vscode", "code", "open in", "jump"],
        run: ({ arg }) => {
          const target = targetFromArg(arg);
          if (target) return openIn(target);
          // Invoked from the action palette / keyboard: use the focused editor.
          const file = getContext().editorPath;
          const folder = useProject.getState().project?.root ?? "";
          if (typeof file === "string" && folder) return jumpToTargetFromActive(file, folder);
        },
      },
    ]),
    registerMenuGroup({
      id: "jumpTo.editor",
      menus: ["editor"],
      order: 6,
      items: (_menu, ctx) => {
        const path = ctx.paths[0];
        if (!path || ctx.text === undefined) return [];
        return [jumpToMenu(path, typeof ctx.line === "number" ? ctx.line : 0)];
      },
    }),
    activateSettingsPages(),
  ];
  return () => disposers.reverse().forEach((d) => d());
}

async function openIn(target: JumpToTarget): Promise<void> {
  try {
    await jumpTo(target);
  } catch (e) {
    notifications().notify({
      level: "error",
      title: tx("Cannot open in another IDE"),
      body: e instanceof Error ? e.message : String(e),
      group: "jumpTo",
    });
  }
}

/** Opens the focused file in the first configured app (for palette / key triggering). */
async function jumpToTargetFromActive(file: string, folder: string): Promise<void> {
  const config = jumpToConfig();
  const first = JUMP_TO_APPS.find((app) => config[app.id]?.show);
  if (!first) return;
  await openIn({ appId: first.id, folder, file, line: 0 });
}
