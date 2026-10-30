/**
 * Reformat Code in Markdown files (⌥⌘L): prettier normalizes the text, then
 * headings get outline numbers (`1.`, `1.1`, `1.1.1`). Registered as the
 * editor's own formatter, so no language server is involved.
 */
import { EditorState } from "@codemirror/state";
import { CONTRIBUTED_FORMAT, registerEditorContribution, type ContributedFormatter } from "@/editor/api";
import { formatOptionsOf } from "@/editor/lsp/format";
import { isMarkdownPath } from "../fileKinds";

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

/** Prettier's Markdown output for `text`; loaded on first use. */
export async function prettifyMarkdown(text: string, tabWidth: number, useTabs: boolean): Promise<string> {
  const [{ format }, markdown] = await Promise.all([
    import("prettier/standalone"),
    import("prettier/plugins/markdown"),
  ]);
  return format(text, { parser: "markdown", plugins: [markdown], proseWrap: "preserve", tabWidth, useTabs });
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

export function markdownFormatter(): ContributedFormatter {
  return async (state, range) => {
    const { tabSize, insertSpaces } = formatOptionsOf(state);
    const pretty = (t: string) => prettifyMarkdown(t, tabSize, !insertSpaces);
    if (range) {
      // The selected lines alone: their place among the headings is unknown, so they are not numbered.
      const from = state.doc.lineAt(range.from).from;
      const to = state.doc.lineAt(range.to).to;
      const before = state.sliceDoc(from, to);
      const after = (await pretty(before)).replace(/\n+$/, "");
      return replacement(before, after).map((c) => ({ ...c, from: c.from + from, to: c.to + from }));
    }
    const before = state.doc.toString();
    const after = numberHeadings(await pretty(before));
    return replacement(before, after);
  };
}

export function registerMarkdownFormat(): () => void {
  return registerEditorContribution({
    id: "viewers.markdownFormat",
    extensions: (ctx) =>
      !ctx.virtual && isMarkdownPath(ctx.path)
        ? [EditorState.languageData.of(() => [{ [CONTRIBUTED_FORMAT]: markdownFormatter() }])]
        : [],
  });
}
