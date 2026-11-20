/**
 * Settings › Tools › Jump To: one row per IDE with its executable path and a
 * Show switch. Paths are picked with the native file chooser (or typed in a
 * browser window); edits go through the settings dialog's staged value so
 * Apply / Cancel behave like every other page.
 */
import { useCallback } from "react";
import { Button, Checkbox, Input, T } from "@/ui";
import { choosePath } from "@/platform/choosePath";
import { JUMP_TO_APPS, DEFAULT_JUMP_TO_CONFIG, defaultExecutable, type JumpToConfig } from "@/platform/jumpTo";
import { Section } from "@/features/settings/components/Form";
import { useSetting } from "@/features/settings/components/context";
import styles from "./SettingsPage.module.css";

const blank = (): JumpToConfig => Object.fromEntries(JUMP_TO_APPS.map((app) => [app.id, { path: "", show: false }]));

export function JumpToSettingsPage() {
  const [stored, setConfig] = useSetting<JumpToConfig>("plugins.jumpTo", "global");
  // Fill missing apps from defaults so a fresh install shows the mainstream IDEs.
  const effective: JumpToConfig = blank();
  for (const app of JUMP_TO_APPS) {
    effective[app.id] = { ...DEFAULT_JUMP_TO_CONFIG[app.id]!, ...stored?.[app.id] };
  }

  const set = useCallback(
    (id: string, patch: { path?: string; show?: boolean }) => {
      const next: JumpToConfig = {};
      for (const app of JUMP_TO_APPS) {
        const v = effective[app.id]!;
        next[app.id] = { path: v.path, show: v.show };
      }
      const cur = next[id]!;
      next[id] = { path: patch.path ?? cur.path, show: patch.show ?? cur.show };
      setConfig(next);
    },
    [effective, setConfig],
  );

  const browse = useCallback(
    async (id: string) => {
      const picked = await choosePath({
        directory: false,
        title: "Choose IDE executable",
        defaultPath: effective[id]?.path,
      });
      if (picked) set(id, { path: picked });
    },
    [effective, set],
  );

  return (
    <div className={styles.page}>
      <p className={styles.intro}>
        <T>
          Right-click a file in the editor and choose Jump To to open the same project and file, at the same line, in
          another IDE. Set each editor's executable and turn it on to show it in the menu; leave the path empty to use
          the default command on PATH.
        </T>
      </p>
      <Section>
        <table className={styles.table}>
          <thead>
            <tr>
              <th className={styles.name}>Editor</th>
              <th>Executable path</th>
              <th className={styles.browse}>Browse</th>
              <th className={styles.show}>Show</th>
            </tr>
          </thead>
          <tbody>
            {JUMP_TO_APPS.map((app) => {
              const value = effective[app.id]!;
              const inputId = `jump-to-${app.id}`;
              return (
                <tr key={app.id}>
                  <td className={styles.name}>
                    <label htmlFor={inputId}>{app.name}</label>
                  </td>
                  <td className={styles.pathCell}>
                    <Input
                      id={inputId}
                      className={styles.path}
                      value={value.path}
                      placeholder={defaultExecutable(app)}
                      aria-label="Executable path"
                      onChange={(e) => set(app.id, { path: e.target.value })}
                    />
                  </td>
                  <td className={styles.browse}>
                    <Button
                      icon="folder-open"
                      size="small"
                      aria-label="Browse for executable"
                      onClick={() => void browse(app.id)}
                    />
                  </td>
                  <td className={styles.show}>
                    <Checkbox
                      checked={value.show}
                      aria-label="Show in menu"
                      onChange={(show) => set(app.id, { show })}
                    />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </Section>
    </div>
  );
}
