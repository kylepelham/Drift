/** Double-clicking a path anywhere in Drift selects the whole of it and copies it with this platform's separators. */

/** What a path cannot contain: whitespace, quotes, brackets and the like end it. */
const outsidePath = /[\s"'`<>|()[\]{},;*?]/;
const url = /^[a-z][a-z\d+.-]*:\/\//i;
const drive = /^[a-z]:[\\/]/i;
const rooted = /^(~|\.{1,2})?[\\/]/;
const extension = /\.[a-z\d]{1,8}$/i;
/** A `:line` or `:line:col` reference after a path, which a file manager or shell would not take. */
const lineReference = /:\d+(:\d+)?$/;

export type FoundPath = { start: number; end: number; path: string };

/** The path in `text` around `offset`, with where it starts and ends; none when the text there is not one. */
export function pathAt(text: string, offset: number): FoundPath | undefined {
    let start = Math.min(offset, text.length);
    let end = start;
    while (start > 0 && !outsidePath.test(text[start - 1])) start--;
    while (end < text.length && !outsidePath.test(text[end])) end++;

    // Sentence punctuation after a path is not part of it.
    while (end > start && /[.,:;!]/.test(text[end - 1])) end--;
    const candidate = text.slice(start, end);
    const path = candidate.replace(lineReference, "");
    if (!looksLikePath(path)) return undefined;

    return { start, end: start + path.length, path };
}

function looksLikePath(text: string) {
    if (!/[\\/]/.test(text) || !/[a-z]/i.test(text) || url.test(text)) return false;
    if (drive.test(text) || rooted.test(text)) return true;

    // A bare `a/b` is too often prose ("and/or"); a file name or a longer chain is a path.
    const separators = text.match(/[\\/]/g)?.length ?? 0;
    return extension.test(text) || separators >= 2;
}

/** `path` as this platform writes it: backslashes on Windows, where `/c/Users` is `C:\Users`; slashes elsewhere. */
export function nativePath(path: string, windows: boolean) {
    if (!windows) return path.replaceAll("\\", "/");

    const unixDrive = /^\/([a-z])(\/|$)/i.exec(path);
    const withDrive = unixDrive ? `${unixDrive[1].toUpperCase()}:\\${path.slice(unixDrive[0].length)}` : path;
    return withDrive.replaceAll("/", "\\");
}

const windows = typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

/** The text node and offset under the pointer. */
function caretAt(x: number, y: number): { node: Text; offset: number } | undefined {
    const position = document.caretPositionFromPoint?.(x, y);
    const node = position?.offsetNode ?? document.caretRangeFromPoint?.(x, y)?.startContainer;
    const offset = position?.offset ?? document.caretRangeFromPoint?.(x, y)?.startOffset ?? 0;

    return node instanceof Text ? { node, offset } : undefined;
}

/** Listens for double-clicks on paths; `copied` hears each path copied, as written to the clipboard. */
export function bindPathCopy(copied: (path: string) => void) {
    const onDoubleClick = (event: MouseEvent) => {
        // Editable text and controls keep their own double-click.
        if (event.target instanceof Element && event.target.closest("input, textarea, [contenteditable], button, a"))
            return;

        const caret = caretAt(event.clientX, event.clientY);
        const found = caret && pathAt(caret.node.data, caret.offset);
        if (!caret || !found) return;

        const range = document.createRange();
        range.setStart(caret.node, found.start);
        range.setEnd(caret.node, found.end);
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range);

        const native = nativePath(found.path, windows);
        void navigator.clipboard.writeText(native).then(
            () => copied(native),
            () => undefined,
        );
    };

    document.addEventListener("dblclick", onDoubleClick);
    return () => document.removeEventListener("dblclick", onDoubleClick);
}
