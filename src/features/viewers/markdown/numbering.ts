/**
 * Number Headings, an item of the Markdown editor's context menu: ATX headings
 * get outline numbers (`1.`, `1.1`, `1.1.1`), replacing the ones a previous run
 * wrote. Formatting itself is left to Reformat Code (Prettier).
 */
import { activeEditor } from "@/editor/api";
import { registerActions } from "@/platform/actions";
import { registerMenuGroup } from "@/workbench/contributions";
import { isMarkdownPath } from "../fileKinds";

export const NUMBER_HEADINGS = "markdown.numberHeadings";

const HEADING = /^( {0,3}#{1,6})([ \t]+)(.*)$/;
const FENCE = /^ {0,3}(`{3,}|~{3,})/;
/** A number this formatter wrote: `1.` / `2.` or `1.2` / `1.2.3`. Plain "2024 plan" is left alone. */
const OWN_NUMBER = /^(?:\d+\.|\d+(?:\.\d+)+\.?)[ \t]+/;

/**
 * Prefixes ATX headings from `fromLevel` down with outline numbers, replacing
 * numbers written by a previous run. Code fences and front matter are skipped.
 */
export function numberHeadings(text: string, fromLevel = 1): string {
  const from = Math.min(6, Math.max(1, Math.round(fromLevel) || 1));
  const lines = text.split("\n");
  const counters: number[] = [];
  let fence: { char: string; size: number } | null = null;
  let start = 0;
  if (lines[0]?.trimEnd() === "---") {
    const end = lines.findIndex((l, i) => i > 0 && /^(---|\.\.\.)\s*$/.test(l));
    if (end > 0) start = end + 1;
  }
  for (let i = start; i < lines.length; i++) {
    const line = lines[i]!;
    const f = FENCE.exec(line);
    if (f) {
      const run = f[1]!;
      if (!fence) fence = { char: run[0]!, size: run.length };
      else if (run[0] === fence.char && run.length >= fence.size && !line.slice(f[0].length).trim()) fence = null;
      continue;
    }
    if (fence) continue;
    const m = HEADING.exec(line);
    if (!m) continue;
    const level = m[1]!.trim().length;
    if (level < from) continue;
    const content = m[3]!.replace(OWN_NUMBER, "");
    if (!content.trim()) continue;
    const depth = level - from;
    counters.length = depth + 1;
    for (let d = 0; d <= depth; d++) counters[d] = d < depth ? counters[d] || 1 : (counters[d] ?? 0) + 1;
    const label = depth === 0 ? `${counters[0]}.` : counters.join(".");
    lines[i] = `${m[1]}${m[2]}${label} ${content}`;
  }
  return lines.join("\n");
}

/** The single replacement turning `before` into `after`, without their common head and tail. */
export function replacement(before: string, after: string): { from: number; to: number; insert: string }[] {
  if (before === after) return [];
  const max = Math.min(before.length, after.length);
  let s = 0;
  while (s < max && before.charCodeAt(s) === after.charCodeAt(s)) s++;
  let eb = before.length;
  let ea = after.length;
  while (eb > s && ea > s && before.charCodeAt(eb - 1) === after.charCodeAt(ea - 1)) {
    eb--;
    ea--;
  }
  return [{ from: s, to: eb, insert: after.slice(s, ea) }];
}

function numberActiveHeadings(): void {
  const e = activeEditor();
  if (!e || e.ctx.virtual || !isMarkdownPath(e.ctx.path) || e.view.state.readOnly) return;
  const before = e.view.state.doc.toString();
  const changes = replacement(before, numberHeadings(before));
  if (changes.length) e.view.dispatch({ changes, userEvent: "input.format", scrollIntoView: true });
}

export function registerMarkdownNumbering(): () => void {
  const disposers = [
    registerActions([
      {
        id: NUMBER_HEADINGS,
        title: "Number Headings",
        category: "Markdown",
        keywords: ["markdown", "heading", "outline", "title", "numbering"],
        when: "textEditorActive && editorLangId == markdown",
        run: numberActiveHeadings,
      },
    ]),
    registerMenuGroup({
      id: "markdown.numbering",
      menus: ["editor"],
      order: 45,
      items: (_menu, ctx) => {
        const path = ctx.paths[0];
        if (!path || ctx.text === undefined || !isMarkdownPath(path)) return [];
        // The action works on the active editor's document.
        return activeEditor()?.ctx.path === path ? [{ kind: "action", action: NUMBER_HEADINGS }] : [];
      },
    }),
  ];
  return () => disposers.reverse().forEach((d) => d());
}
