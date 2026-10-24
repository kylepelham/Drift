import { resolveMarkdownCitation } from "./markdown-links";

type Groups = readonly (readonly string[])[];
type Context = () => { directory?: string; fileGroups?: () => Groups };

const location = /(:[1-9]\d*(?::[1-9]\d*)?)$/;
const roots = new WeakMap<Element, Context>();
let listening = false;

/** Inline code in prose, not a line of a code block. */
function inlineCode(node: Node | null) {
    const element = node instanceof Element ? node : node?.parentElement;
    const code = element?.closest("code");

    return code && !code.closest("pre") ? code : undefined;
}

/** A double click selects a whole inline code span without spaces, a path or a name, not the word under it. */
export function selectInlineCode(event: MouseEvent) {
    const code = inlineCode(event.target as Node);
    const text = code?.textContent ?? "";
    if (!code || !text || /\s/.test(text)) return;

    event.preventDefault();
    window.getSelection()?.selectAllChildren(code);
}

/** Copying one whole inline code path in `root` copies its full path; the document hears it, as prose takes no focus. */
export function copyInlinePaths(root: Element, context: Context) {
    roots.set(root, context);
    if (listening) return;

    listening = true;
    document.addEventListener("copy", copyInlinePath);
}

function copyInlinePath(event: ClipboardEvent) {
    const selection = window.getSelection();
    if (!selection?.rangeCount || !event.clipboardData) return;
    const code = inlineCode(selection.getRangeAt(0).commonAncestorContainer);
    const text = code?.textContent?.trim();
    const root = code?.closest(".md");
    const context = root && roots.get(root)?.();
    if (!text || !context || selection.toString().trim() !== text) return;

    const path = absolutePath(text, context.directory, context.fileGroups?.() ?? []);
    if (!path) return;

    event.preventDefault();
    event.clipboardData.setData("text/plain", path);
}

/** The full path an inline code span names, with its `:line` kept; `undefined` when it does not name one. */
export function absolutePath(text: string, directory: string | undefined, groups: Groups) {
    const suffix = location.exec(text)?.[1] ?? "";
    const target = text.slice(0, text.length - suffix.length);
    if (!directory || !target || /[\s<>"|?*]/.test(target.replace(/^[a-z]:/i, ""))) return;

    const href = target
        .split(/([/\\])/)
        .map((segment) => (segment === "/" || segment === "\\" ? segment : encodeURIComponent(segment)))
        .join("")
        .replace(/^([a-z])%3A([/\\])/i, "$1:$2");
    let link;
    try {
        link = resolveMarkdownCitation(href, directory, groups);
    } catch {
        return;
    }
    if (link.kind !== "file") return;
    // A bare name could be any identifier; it is a path only when the chat touched a file by that name.
    const key = (path: string) => path.replaceAll("\\", "/").toLowerCase();
    const touched = groups.some((files) => files.some((file) => key(file) === key(link.path)));
    if (!/[/\\]/.test(target) && !touched) return;

    return native(link.path, directory) + suffix;
}

/** Back to the folder's own separators: backslashes on Windows. */
function native(path: string, directory: string) {
    return /^[a-z]:[/\\]|^[/\\]{2}/i.test(directory) ? path.replaceAll("/", "\\") : path;
}
