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

    // A mention (`@src/app.tsx`) names the path after its sign; sentence punctuation after a path is not part of it.
    if (text[start] === "@") start++;
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

/** A whole span's text as one path, spaces and all, as inline code or a tooltip holds it; none when it is not one. */
export function wholePath(text: string): string | undefined {
    const path = text.trim().replace(/^@/, "").replace(lineReference, "");
    if (/[\n\t"'`<>|]/.test(path) || !/[\\/]/.test(path) || !/[a-z]/i.test(path) || url.test(path)) return undefined;

    // With spaces it must start like a path (`C:\Program Files\...`); a command with a path in it is not one.
    if (/\s/.test(path)) return drive.test(path) || rooted.test(path) ? path : undefined;
    return path;
}

/** A position in one of a line's pieces of text. */
export type Spot = { piece: number; offset: number };

/**
 * The path at `at` in a line drawn as several pieces of text (highlighted tokens, a muted folder and a bold name),
 * read as the one string they show. A piece that starts a new block is kept apart from the one before.
 */
export function pathAcross(pieces: { text: string; newBlock: boolean }[], at: Spot) {
    let joined = "";
    const starts: number[] = [];
    for (const piece of pieces) {
        if (piece.newBlock && joined) joined += "\n";
        starts.push(joined.length);
        joined += piece.text;
    }
    const found = pathAt(joined, starts[at.piece] + at.offset);
    if (!found) return undefined;

    // The piece a position falls in; an end on a boundary belongs to the piece before it.
    const spot = (index: number, end: boolean): Spot => {
        let piece = 0;
        starts.forEach((start, at) => {
            if (end ? start < index : start <= index) piece = at;
        });
        return { piece, offset: Math.min(index - starts[piece], pieces[piece].text.length) };
    };
    return { path: found.path, start: spot(found.start, false), end: spot(found.end, true) };
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

/** The nearest element around `node` that is laid out as its own box: the line a path cannot run past. */
function blockAround(node: Node, stop?: Element): Element {
    let element = node.parentElement!;
    while (element !== stop && element.parentElement) {
        const display = getComputedStyle(element).display;
        if (display !== "inline" && display !== "contents") return element;
        element = element.parentElement;
    }

    return element;
}

/** Every text node of `container` in order, each marked where it starts a new block. */
function pieces(container: Element) {
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    const nodes: Text[] = [];
    const parts: { text: string; newBlock: boolean }[] = [];
    let previous: Element | undefined;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const text = node as Text;
        const block = blockAround(text, container);
        nodes.push(text);
        parts.push({ text: text.data, newBlock: !!previous && block !== previous });
        previous = block;
    }

    return { nodes, parts };
}

/** The full path an element's tooltip gives for the shorter one it shows, when the two are the same file. */
function fullFromTooltip(node: Node, container: Element, shown: string) {
    const name = shown.split(/[\\/]/).at(-1) ?? "";
    for (let element = node.parentElement; element; element = element.parentElement) {
        const full = element.getAttribute("title");
        const path = full ? wholePath(full.split("\n")[0]) : undefined;
        if (path && name && path.replaceAll("\\", "/").endsWith(name)) return path;
        if (element === container) break;
    }

    return undefined;
}

/** Selects the text from `start` to `end` across the line's nodes. */
function select(nodes: Text[], start: Spot, end: Spot) {
    const range = document.createRange();
    range.setStart(nodes[start.piece], start.offset);
    range.setEnd(nodes[end.piece], end.offset);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
}

/** The path under a double-click: a whole inline code span, else the path in its line, made full from a tooltip. */
function pathUnder(node: Text, offset: number) {
    const code = node.parentElement?.closest("code");
    const whole = code && !code.closest("pre") ? wholePath(code.textContent ?? "") : undefined;
    if (code && whole) {
        const range = document.createRange();
        range.selectNodeContents(code);
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range);
        return whole;
    }

    const container = blockAround(node);
    const line = pieces(container);
    const found = pathAcross(line.parts, { piece: line.nodes.indexOf(node), offset });
    if (!found) return undefined;

    select(line.nodes, found.start, found.end);
    return fullFromTooltip(node, container, found.path) ?? found.path;
}

/** Listens for double-clicks on paths; `copied` hears each path copied, as written to the clipboard. */
export function bindPathCopy(copied: (path: string) => void, failed: (path: string) => void) {
    const onDoubleClick = (event: MouseEvent) => {
        // Editable text and links keep their own double-click; a path in a button (a tool row's title) still copies.
        if (event.target instanceof Element && event.target.closest("input, textarea, select, [contenteditable], a"))
            return;

        const caret = caretAt(event.clientX, event.clientY);
        const path = caret && pathUnder(caret.node, caret.offset);
        if (!path) return;

        const native = nativePath(path, windows);
        void navigator.clipboard.writeText(native).then(
            () => copied(native),
            () => failed(native),
        );
    };

    document.addEventListener("dblclick", onDoubleClick);
    return () => document.removeEventListener("dblclick", onDoubleClick);
}
