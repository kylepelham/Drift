const maxPreviewRows = 30;
const maxPreviewChars = 20_000;

function parseDelimitedRows(text: string, delimiter: string) {
    const rows: string[][] = [];
    let row: string[] = [];
    let field = "";
    let quoted = false;

    for (let index = 0; index < text.length; index++) {
        const character = text[index];
        if (character === '"') {
            if (quoted && text[index + 1] === '"') {
                field += '"';
                index++;
            } else quoted = !quoted;
            continue;
        }
        if (character === delimiter && !quoted) {
            row.push(field);
            field = "";
            continue;
        }
        if (isRowBreak(character, quoted)) {
            if (character === "\r" && text[index + 1] === "\n") index++;

            row.push(field);
            rows.push(row);
            row = [];
            field = "";
            continue;
        }
        field += character;
    }

    if (field || row.length || (!rows.length && text.length)) {
        row.push(field);
        rows.push(row);
    }

    return rows;
}

function isRowBreak(character: string, quoted: boolean) {
    return (character === "\n" || character === "\r") && !quoted;
}

function sniffDelimiter(text: string) {
    const sample = text
        .split(/\r\n?|\n/)
        .filter(Boolean)
        .slice(0, 10)
        .join("\n");
    const candidates = [",", "\t", ";", "|"];
    let best = ",";
    let score = 0;

    for (const candidate of candidates) {
        const rows = parseDelimitedRows(sample, candidate);
        const widths = rows.map((row) => row.length);
        const common = Math.max(0, ...widths.map((width) => widths.filter((value) => value === width).length));
        const next = Math.max(0, ...widths) > 1 ? common * Math.max(...widths) : 0;
        if (next > score) {
            best = candidate;
            score = next;
        }
    }

    return best;
}

function markdownCell(value: string) {
    const clipped = value.length > 240 ? value.slice(0, 240) + "..." : value;

    return clipped.replaceAll("|", "\\|").replace(/\r\n?|\n/g, " ");
}

export function parseCsvAttachment(text: string, rowLimit = maxPreviewRows, charLimit = maxPreviewChars) {
    const delimiter = sniffDelimiter(text);
    const rows = parseDelimitedRows(text, delimiter);
    const columns = Math.max(0, ...rows.map((row) => row.length));
    const normalized = rows.map((row) => Array.from({ length: columns }, (_, index) => markdownCell(row[index] ?? "")));

    const header = normalized[0] ?? [];
    const separator = header.map(() => "---");
    const lines = [header, separator, ...normalized.slice(1, rowLimit)].map((row) => `| ${row.join(" | ")} |`);
    let preview = lines.join("\n");
    let truncated = rows.length > rowLimit;
    if (preview.length > charLimit) {
        preview = preview.slice(0, charLimit);
        truncated = true;
    }
    if (truncated) preview += "\n\n[Table preview truncated]";

    return { delimiter, rows: rows.length, columns, preview, truncated };
}

export function formatCsvAttachment(filename: string, parsed: ReturnType<typeof parseCsvAttachment>) {
    const note = parsed.truncated ? "; preview truncated" : "";

    return `[Attachment: ${filename} (table, ${parsed.rows} rows x ${parsed.columns} columns${note})]\n\n${parsed.preview}`;
}
