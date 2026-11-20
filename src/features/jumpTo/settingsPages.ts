import { lazyComponent } from "@/ui";
import { registerSettingsPage } from "@/workbench/contributions";

export const JUMP_TO_SETTINGS_PAGE = "tools.jumpTo";

/** Settings › Tools › Jump To (also used by the native Settings window). */
export function activateSettingsPages(): () => void {
  return registerSettingsPage({
    id: JUMP_TO_SETTINGS_PAGE,
    title: "Jump To",
    group: "Tools",
    order: 900,
    keywords: [
      "jetbrains",
      "idea",
      "pycharm",
      "webstorm",
      "goland",
      "clion",
      "rider",
      "rustrover",
      "vscode",
      "cursor",
      "external editor",
      "open in",
    ],
    component: lazyComponent(() => import("./SettingsPage").then((m) => m.JumpToSettingsPage)),
  });
}
