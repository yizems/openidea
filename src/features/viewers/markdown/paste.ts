/**
 * Pasting into a Markdown file (Settings › Editor › Markdown Paste): images
 * are saved in the configured folder next to the file and embedded as
 * `![](link)`; image data gets a generated name. Any other file or folder
 * copied from the OS is pasted as its bare name, without copying it.
 */
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { registerEditorContribution } from "@/editor/api";
import { executeAction } from "@/platform/actions";
import { editorPrefs, type EditorExtras } from "@/platform/editorSettings";
import { tx } from "@/platform/i18n";
import { call, commands } from "@/platform/ipc";
import { isMarkdownPath } from "../fileKinds";
import { joinPath } from "./render";

export type MarkdownPasteSettings = EditorExtras["markdownPaste"];

const IMAGE_NAME = /\.(png|jpe?g|gif|webp|bmp|svg|avif|ico|tiff?)$/i;
const MIME_EXTENSIONS: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/bmp": "bmp",
  "image/svg+xml": "svg",
  "image/avif": "avif",
  "image/tiff": "tiff",
  "image/x-icon": "ico",
};
const DEFAULT_DIRECTORY = "./assets";
const DEFAULT_PREFIX = "image";

const slashed = (p: string) => p.replace(/\\/g, "/");
const dirOf = (file: string) => file.slice(0, Math.max(0, file.lastIndexOf("/")));
const isAbsolute = (p: string) => p.startsWith("/") || /^[a-z]:\//i.test(p);

export const isImageName = (name: string) => IMAGE_NAME.test(name);

/** The folder pasted files go to, for the Markdown file `mdPath`. */
export function assetDirectory(mdPath: string, directory: string): string {
  const dir = slashed(directory.trim()) || DEFAULT_DIRECTORY;
  return isAbsolute(dir) ? dir.replace(/(.)\/+$/, "$1") : joinPath(dirOf(slashed(mdPath)), dir);
}

/** `image-20260101-093005.png`: a name for image data, which has none of its own. */
export function pastedImageName(prefix: string, mime: string, now = new Date()): string {
  const p = (n: number, len = 2) => String(n).padStart(len, "0");
  const stamp = `${now.getFullYear()}${p(now.getMonth() + 1)}${p(now.getDate())}-${p(now.getHours())}${p(now.getMinutes())}${p(now.getSeconds())}`;
  const safe = prefix.trim().replace(/[\\/:*?"<>|\s]+/g, "-") || DEFAULT_PREFIX;
  return `${safe}-${stamp}.${MIME_EXTENSIONS[mime.toLowerCase()] ?? "png"}`;
}

const encodeSegment = (s: string) =>
  /^[a-z]:$/i.test(s) ? s : encodeURIComponent(s).replace(/\(/g, "%28").replace(/\)/g, "%29");

/** The link target of `target` as written in the Markdown file `mdPath`: relative when possible. */
export function relativeLink(mdPath: string, target: string): string {
  const from = dirOf(slashed(mdPath)).split("/");
  const to = slashed(target).split("/");
  let common = 0;
  while (common < from.length && common < to.length - 1 && segmentEquals(from[common]!, to[common]!, common)) common++;
  if (common === 0) return to.map(encodeSegment).join("/");
  const up = from.length - common;
  const rel = [...Array<string>(up).fill(".."), ...to.slice(common)].map(encodeSegment).join("/");
  return up === 0 ? `./${rel}` : rel;
}

/** Drive letters compare case-insensitively. */
function segmentEquals(a: string, b: string, index: number): boolean {
  return a === b || (index === 0 && /^[a-z]:$/i.test(a) && a.toLowerCase() === b.toLowerCase());
}

const escapeLabel = (s: string) => s.replace(/[[\]\\]/g, "\\$&");

/** `![alt](link)` for a saved image. */
export function markdownLink(mdPath: string, savedPath: string): string {
  const name = savedPath.slice(savedPath.lastIndexOf("/") + 1);
  return `![${escapeLabel(name.replace(/\.[^.]+$/, ""))}](${relativeLink(mdPath, savedPath)})`;
}

/** Last component of a path, as pasted for files that are not images. */
export const baseName = (path: string) => slashed(path).replace(/\/+$/, "").split("/").pop() ?? "";

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

const hint = (text: string) => void executeAction("editor.showHint", { text });

/**
 * The text to insert, one line per pasted item. Images are saved into the
 * folder and embedded; any other file or folder is pasted as its bare name
 * and is not copied.
 */
async function pastedLines(
  mdPath: string,
  prefs: MarkdownPasteSettings,
  clipboardPaths: string[],
  files: File[],
): Promise<string[]> {
  const dir = assetDirectory(mdPath, prefs.directory);
  const lines: string[] = [];
  if (clipboardPaths.length) {
    for (const from of clipboardPaths) {
      if (isImageName(baseName(from))) lines.push(markdownLink(mdPath, await call(commands.fsCopyInto(from, dir))));
      else lines.push(baseName(from));
    }
    return lines;
  }
  for (const file of files) {
    if (!file.type.startsWith("image/") && !isImageName(file.name)) {
      if (file.name) lines.push(file.name);
      continue;
    }
    const name = pastedImageName(prefs.imagePrefix, file.type);
    const data = toBase64(new Uint8Array(await file.arrayBuffer()));
    lines.push(markdownLink(mdPath, await call(commands.fsSavePasted(dir, name, data))));
  }
  return lines;
}

async function pasteAssets(
  view: EditorView,
  mdPath: string,
  prefs: MarkdownPasteSettings,
  files: File[],
  text: string,
) {
  try {
    const clipboardPaths = await call(commands.clipboardFileList()).catch(() => [] as string[]);
    // Rich text from office tools carries an image next to its text: keep the text.
    if (!clipboardPaths.length && text.trim()) return insert(view, text);
    const lines = await pastedLines(mdPath, prefs, clipboardPaths, files);
    if (lines.length) insert(view, lines.join("\n"));
  } catch (e) {
    const message = e instanceof Error ? e.message : String((e as { message?: unknown })?.message ?? e);
    hint(tx`Cannot paste into the Markdown file: ${message}`);
  }
}

function insert(view: EditorView, text: string) {
  if (!view.dom.isConnected || view.state.readOnly) return;
  view.dispatch(view.state.replaceSelection(text), { userEvent: "input.paste", scrollIntoView: true });
  view.focus();
}

/** Whether the paste carries files (or a file list), which CodeMirror would otherwise drop. */
function hasFiles(data: DataTransfer): boolean {
  return data.files.length > 0 || data.types.includes("Files") || data.types.includes("text/uri-list");
}

export function markdownPasteExtension(mdPath: string): Extension {
  return EditorView.domEventHandlers({
    paste(event, view) {
      const prefs = editorPrefs().markdownPaste;
      const data = event.clipboardData;
      if (!prefs.enabled || !data || view.state.readOnly || !hasFiles(data)) return false;
      event.preventDefault();
      void pasteAssets(view, mdPath, prefs, [...data.files], data.getData("text/plain"));
      return true;
    },
  });
}

export function registerMarkdownPaste(): () => void {
  return registerEditorContribution({
    id: "viewers.markdownPaste",
    extensions: (ctx) => (!ctx.virtual && isMarkdownPath(ctx.path) ? [markdownPasteExtension(ctx.path)] : []),
  });
}
