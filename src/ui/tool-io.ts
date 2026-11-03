const acronyms = new Set(["id", "url", "uri", "api", "pr", "html", "json", "sql", "ui"]);

export type ToolField = { key: string; label: string; text: string; structured: boolean };
export type ToolOutputView = { kind: "json" | "markdown" | "text"; text: string };

// Headings, list items, fences, quotes, table rows, bold runs and links: text written to be read as Markdown.
const markdownSigns = /^(#{1,6} |\s*[-*+] |\s*\d+\. |```|> |\|.*\|\s*$)|\*\*[^*\n]+\*\*|\[[^\]\n]+\]\([^)\s]+\)/m;

/** A field name as words: `case_number` and `caseNumber` both read "Case number", `work_item_id` "Work item ID". */
function fieldLabel(key: string) {
    const words = key
        .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
        .split(/[\s_-]+/)
        .filter(Boolean)
        .map((word) => (acronyms.has(word.toLowerCase()) ? word.toUpperCase() : word.toLowerCase()));
    const text = words.join(" ");

    return text.charAt(0).toUpperCase() + text.slice(1);
}

/** A call's arguments as labelled rows; empty ones are left out, nested ones keep their JSON shape. */
export function inputFields(input: Record<string, unknown>): ToolField[] {
    return Object.entries(input).flatMap(([key, value]) => {
        if (value === undefined || value === null) return [];
        const text = fieldText(value);
        if (!text) return [];

        return [{ key, label: fieldLabel(key), text, structured: typeof value === "object" && !isFlatList(value) }];
    });
}

/** How a tool's text result reads best: JSON indented, Markdown rendered, anything else as written. */
export function outputView(output: string): ToolOutputView {
    const text = output.trim();
    const json = prettyJson(text);
    if (json !== undefined) return { kind: "json", text: json };

    return { kind: markdownSigns.test(text) ? "markdown" : "text", text };
}

function fieldText(value: unknown) {
    if (typeof value === "string") return value;
    if (typeof value !== "object") return String(value);
    if (isFlatList(value)) return (value as unknown[]).join(", ");

    return JSON.stringify(value, null, 2);
}

function isFlatList(value: unknown) {
    return Array.isArray(value) && value.every((item) => item === null || typeof item !== "object");
}

function prettyJson(text: string) {
    if (!/^[[{]/.test(text)) return undefined;
    try {
        return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
        return undefined;
    }
}
