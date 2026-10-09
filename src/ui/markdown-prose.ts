const numberedLinePattern = /^\s{0,3}(\d{1,9})\.(?=\s|$)/;
const urlPattern = /https?:\/\/[^\s<>"')\]]+/g;
const accidentalEntities: Record<string, string> = { "-": "&#45;", "=": "&#61;", "*": "&#42;", _: "&#95;" };
const voidHtml = new Set([
    "area",
    "base",
    "br",
    "col",
    "embed",
    "hr",
    "img",
    "input",
    "link",
    "meta",
    "param",
    "source",
    "track",
    "wbr",
]);

/** Reports whether text ends inside an unterminated fenced block, using CommonMark fence rules. */
export function endsInsideFence(text: string) {
    let marker = "";
    let markerLength = 0;

    for (const line of text.split("\n")) {
        let start = 0;
        while (start < 3 && line[start] === " ") start++;

        const character = line[start];
        if (character !== "`" && character !== "~") continue;

        let end = start;
        while (line[end] === character) end++;

        const run = end - start;
        if (marker) {
            if (character === marker && run >= markerLength && line.slice(end).trim() === "") {
                marker = "";
                markerLength = 0;
            }
            continue;
        }
        if (run < 3) continue;
        if (character === "`" && line.slice(end).includes("`")) continue;

        marker = character;
        markerLength = run;
    }

    return marker !== "";
}

/** Transforms prose while preserving fenced blocks and exact-length inline code spans. */
function mapProseChunks(text: string, transform: (chunk: string) => string) {
    let result = "";
    let proseStart = 0;
    let index = 0;

    const preserve = (start: number, end: number) => {
        result += transform(text.slice(proseStart, start)) + text.slice(start, end);
        proseStart = end;
        index = end;
    };

    while (index < text.length) {
        const lineStart = index === 0 || text[index - 1] === "\n";
        const fenceEnd = lineStart ? proseFenceEnd(text, index) : undefined;
        if (fenceEnd !== undefined) {
            preserve(index, fenceEnd);
            continue;
        }

        if (text[index] === "`") {
            const openerEnd = markerRunEnd(text, index, "`");
            const closeEnd = proseCodeEnd(text, openerEnd, openerEnd - index);
            if (closeEnd !== undefined) preserve(index, closeEnd);
            else index = openerEnd;
            continue;
        }
        index++;
    }

    return result + transform(text.slice(proseStart));
}

function indentedMarkerStart(text: string, start: number) {
    let index = start;
    while (index < start + 3 && text[index] === " ") index++;

    return index;
}

function markerRunEnd(text: string, start: number, marker: string) {
    let end = start;
    while (text[end] === marker) end++;

    return end;
}

function proseFenceEnd(text: string, start: number) {
    const markerStart = indentedMarkerStart(text, start);
    const marker = text[markerStart];
    if (marker !== "`" && marker !== "~") return;

    const markerEnd = markerRunEnd(text, markerStart, marker);
    const length = markerEnd - markerStart;
    const openerEnd = text.indexOf("\n", markerEnd);
    const infoEnd = openerEnd < 0 ? text.length : openerEnd;
    if (length < 3 || (marker === "`" && text.slice(markerEnd, infoEnd).includes("`"))) return;

    return closingFenceEnd(text, openerEnd < 0 ? text.length : openerEnd + 1, marker, length);
}

function closingFenceEnd(text: string, start: number, marker: string, length: number) {
    let closeStart = start;

    while (closeStart < text.length) {
        const markerStart = indentedMarkerStart(text, closeStart);
        const markerEnd = markerRunEnd(text, markerStart, marker);
        const lineEnd = text.indexOf("\n", markerEnd);
        const trailingEnd = lineEnd < 0 ? text.length : lineEnd;
        if (markerEnd - markerStart >= length && text.slice(markerEnd, trailingEnd).trim() === "")
            return lineEnd < 0 ? text.length : lineEnd + 1;

        const nextLine = text.indexOf("\n", closeStart);
        if (nextLine < 0) break;

        closeStart = nextLine + 1;
    }

    return text.length;
}

function proseCodeEnd(text: string, start: number, length: number) {
    let cursor = start;

    while (cursor < text.length) {
        const close = text.indexOf("`", cursor);
        if (close < 0) break;

        const closeEnd = markerRunEnd(text, close, "`");
        if (closeEnd - close === length) return closeEnd;

        cursor = closeEnd;
    }
}

// Path-ending backslashes can escape emphasis delimiters in model output such as **C:\**.
export function fixEscapedEmphasis(text: string) {
    return mapProseChunks(text, (chunk) =>
        chunk.replace(/(:)\\(?=\*\*?|__?)/g, "$1\\\\").replace(/(\\[\w .()-]+)\\(?=\*\*?|__?)/g, "$1\\\\"),
    );
}

/** Keeps lone large numbers as prose; zero, one, and sequential sibling numbers remain lists. */
function escapeLoneNumberedLines(text: string) {
    // Inline code must not split the document-wide check for sequential numbered lines.
    const lines = text.split("\n");
    const fenced = fencedLines(text);
    const numbers = lines.map((line, index) => (fenced.has(index) ? undefined : numberedLinePattern.exec(line)?.[1]));
    const numbered = numbers.flatMap((value, index) => (value === undefined ? [] : [index]));
    let position = -1;

    return lines
        .map((line, index) => {
            const value = numbers[index];
            if (value === undefined) return line;

            position += 1;
            const start = Number(value);
            if (start <= 1) return line;

            const previous = position > 0 ? Number(numbers[numbered[position - 1]]) : undefined;
            const next = position < numbered.length - 1 ? Number(numbers[numbered[position + 1]]) : undefined;
            if (previous === start - 1 || next === start + 1) return line;

            return line.replace(".", "\\.");
        })
        .join("\n");
}

/** Finds line indices inside fences using the same rules as mapProseChunks. */
function fencedLines(text: string) {
    const inside = new Set<number>();
    const lines = text.split("\n");
    let index = 0;

    while (index < lines.length) {
        const opener = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(lines[index]);
        if (!opener || (opener[1][0] === "`" && opener[2].includes("`"))) {
            index++;
            continue;
        }

        const marker = opener[1];
        let close = index + 1;
        while (close < lines.length) {
            const closer = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(lines[close]);
            if (closer && closer[1][0] === marker[0] && closer[1].length >= marker.length) break;

            close++;
        }

        for (let line = index; line <= Math.min(close, lines.length - 1); line++) inside.add(line);
        index = close + 1;
    }

    return inside;
}

function humanizeProse(text: string) {
    return mapProseChunks(text, (chunk) => {
        const literal = chunk.replaceAll("\\", "&#92;").replaceAll("<", "&lt;");

        return literal.split("\n").map(neutralizeProseLine).join("\n");
    });
}

/** Neutralizes block and emphasis syntax commonly pasted from terminals or prose. */
function neutralizeProseLine(line: string) {
    let result = line.replace(/^(\s{0,3})>/, "$1&gt;").replace(/^(\s{0,3})#(?=#{0,5}(\s|$))/, "$1&#35;");
    if (/^\s{0,3}[-=*_](\s*[-=*_])*\s*$/.test(result))
        result = result.replace(/[-=*_]/, (marker) => accidentalEntities[marker]);

    let escaped = "";
    let cursor = 0;
    for (const match of result.matchAll(urlPattern)) {
        escaped += escapeEmphasis(result.slice(cursor, match.index)) + match[0];
        cursor = match.index + match[0].length;
    }

    return escaped + escapeEmphasis(result.slice(cursor));
}

function escapeEmphasis(text: string) {
    return text.replaceAll("*", "&#42;").replaceAll("_", "&#95;").replaceAll("~~", "&#126;&#126;");
}

function escapeUnbalancedHtml(text: string) {
    return mapProseChunks(text, escapeUnbalancedHtmlChunk);
}

/** Escapes unmatched tags so streamed examples cannot turn the rest of a response into HTML. */
function escapeUnbalancedHtmlChunk(text: string) {
    const tags = [...text.matchAll(/<!--[^]*?-->|<\/?[A-Za-z][^>\n]*>/g)].map((match) => ({
        start: match.index,
        end: match.index + match[0].length,
        value: match[0],
        name: match[0].match(/^<\/?\s*([A-Za-z][\w:-]*)/)?.[1]?.toLowerCase(),
        closing: /^<\//.test(match[0]),
        matched: false,
    }));

    /** Indices of opening tags still waiting for a closing tag. */
    const stack: number[] = [];
    for (let index = 0; index < tags.length; index++) {
        const tag = tags[index];
        if (!tag.name || tag.value.startsWith("<!--") || voidHtml.has(tag.name) || /\/\s*>$/.test(tag.value)) {
            tag.matched = true;
            continue;
        }
        if (!tag.closing) {
            stack.push(index);
            continue;
        }

        // Searching beyond the stack's top preserves individually paired, incorrectly nested tags.
        let opener = -1;
        for (let position = stack.length - 1; position >= 0; position--) {
            if (tags[stack[position]].name !== tag.name) continue;

            opener = position;
            break;
        }
        if (opener < 0) continue;

        const openIndex = stack[opener];
        tags[openIndex].matched = true;
        tag.matched = true;
        stack.splice(opener, 1);
    }

    let result = "";
    let cursor = 0;
    for (const tag of tags) {
        result += text.slice(cursor, tag.start);
        result += tag.matched ? tag.value : tag.value.replaceAll("<", "&lt;").replaceAll(">", "&gt;");
        cursor = tag.end;
    }

    return result + text.slice(cursor);
}

export function prepareMarkdown(text: string, humanAuthored = false) {
    const prose = humanAuthored ? humanizeProse(text) : fixEscapedEmphasis(text);
    const numbered = escapeLoneNumberedLines(prose);

    return escapeUnbalancedHtml(numbered);
}
