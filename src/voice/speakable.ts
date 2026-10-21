/** What stands in for parts of a reply that are named rather than read. */
export type SpeakableLabels = { code: string; table: string };

const fence = /^\s{0,3}(`{3,}|~{3,})/;
const tableRow = /^\s*\|.*\|\s*$/;
const tableRule = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const rule = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
/** A path with a directory or an extension, optionally with a line, as replies cite files. */
const filePath = /(?:[A-Za-z]:)?(?:[\w@.-]+[\\/])+[\w@.-]+(?::\d+(?::\d+)?)?|\b[\w-]+\.[A-Za-z][\w]{0,5}:\d+(?::\d+)?/g;

function fileName(path: string) {
    return (path.split(/[\\/]/).at(-1) ?? path).replace(/:\d+(?::\d+)?$/, "");
}

function looksLikeFile(text: string) {
    return /[\\/]/.test(text) || /^[\w@.-]+\.[A-Za-z][\w]{0,5}(?::\d+(?::\d+)?)?$/.test(text);
}

/** One line of prose without its Markdown: links by their text, code spans and file paths by file name. */
function speakLine(line: string) {
    return line
        .replace(/^\s{0,3}#{1,6}\s+/, "")
        .replace(/^\s*>\s?/, "")
        .replace(/^\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?/, "")
        .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
        .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
        .replace(/`([^`]+)`/g, (_, code: string) => (looksLikeFile(code.trim()) ? fileName(code.trim()) : code))
        .replace(/https?:\/\/([^/\s)]+)\S*/g, "$1")
        .replace(filePath, fileName)
        .replace(/<[^>]+>/g, "")
        .replace(/(\*\*|__)(.+?)\1/g, "$2")
        .replace(/(^|[\s(])[*_]([^*_\s][^*_]*?)[*_](?=[\s).,;:!?]|$)/g, "$1$2")
        .replace(/~~(.+?)~~/g, "$1")
        .replace(/\s+/g, " ")
        .trim();
}

/** Where a block that is named rather than read ends, or undefined when `index` starts no such block. */
function namedBlockEnd(lines: string[], index: number) {
    const opener = fence.exec(lines[index]!)?.[1];
    if (opener) {
        let end = index + 1;
        while (end < lines.length && !closesFence(lines[end]!, opener)) end += 1;
        return { label: "code" as const, end };
    }
    if (!tableRow.test(lines[index]!) || !tableRule.test(lines[index + 1] ?? "")) return undefined;

    let end = index + 1;
    while (end + 1 < lines.length && tableRow.test(lines[end + 1]!)) end += 1;
    return { label: "table" as const, end };
}

function closesFence(line: string, opener: string) {
    const closer = fence.exec(line)?.[1];
    return !!closer && closer[0] === opener[0] && closer.length >= opener.length;
}

/** Turns a reply's Markdown into the prose worth speaking; code blocks and tables are named, not read. */
export function speakableText(markdown: string, labels: SpeakableLabels) {
    const blocks: string[] = [];
    let paragraph: string[] = [];
    const close = () => {
        const text = paragraph.join(" ");
        if (text) blocks.push(/[.!?:;]$/.test(text) ? text : `${text}.`);
        paragraph = [];
    };
    const lines = markdown.replace(/\r\n?/g, "\n").split("\n");

    for (let index = 0; index < lines.length; index += 1) {
        const line = lines[index]!;
        const named = namedBlockEnd(lines, index);
        if (named || !line.trim() || rule.test(line)) close();
        if (named) {
            blocks.push(`${labels[named.label]}.`);
            index = named.end;
            continue;
        }
        if (/^\s*(?:[-*+]|\d+[.)]|#{1,6})\s/.test(line)) close();
        const spoken = speakLine(line);
        if (spoken && !rule.test(line)) paragraph.push(spoken);
    }
    close();

    return blocks.join("\n\n");
}

/** Splits speech into pieces no longer than `max`: by paragraph, then sentence, then word. */
export function speechChunks(text: string, max: number) {
    const pieces = text
        .split(/\n{2,}/)
        .flatMap((paragraph) => (paragraph.length <= max ? [paragraph] : paragraph.split(/(?<=[.!?])\s+/)))
        .flatMap((sentence) => (sentence.length <= max ? [sentence] : byWords(sentence, max)));
    const chunks: string[] = [];
    for (const piece of pieces.map((piece) => piece.trim()).filter(Boolean)) {
        const last = chunks.at(-1);
        if (last !== undefined && last.length + piece.length + 2 <= max)
            chunks[chunks.length - 1] = `${last}\n\n${piece}`;
        else chunks.push(piece);
    }
    return chunks;
}

function byWords(sentence: string, max: number) {
    const parts: string[] = [];
    let rest = sentence;
    while (rest.length > max) {
        const cut = rest.lastIndexOf(" ", max);
        const at = cut > 0 ? cut : max;
        parts.push(rest.slice(0, at));
        rest = rest.slice(at).trimStart();
    }
    parts.push(rest);
    return parts;
}
