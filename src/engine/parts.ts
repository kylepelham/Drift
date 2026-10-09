import type { components } from "./native/types";

export type Part = components["schemas"]["PartRow"];
export type ToolPart = Extract<Part, { type: "tool_call" }>;
export type FilePart = Extract<Part, { type: "file" }>;
export type ContextPart = Extract<Part, { type: "context" }>;
export type ReasoningPart = Extract<Part, { type: "reasoning" }>;

/** Text shown or restored as the user's words, excluding delivered worker results. */
export function promptPartText(part: Part) {
    if (part.type === "text" || part.type === "nudge") return part.text;
    if (part.type === "clarification") {
        return part.items.map((item) => `${item.question}\nAnswer: ${item.answers.join(", ")}`).join("\n\n");
    }

    return undefined;
}

export function toolInput(part: ToolPart): Record<string, unknown> {
    const input = part.input && typeof part.input === "object" ? { ...part.input } : { value: part.input };
    const fields = input as Record<string, unknown>;
    const written = part.metadata?.files?.find((file): file is string => typeof file === "string");

    if (
        ["read", "edit", "write"].includes(part.name) &&
        typeof fields.path === "string" &&
        fields.filePath === undefined
    )
        fields.filePath = written ?? fields.path;
    if (part.name === "apply_patch" && fields.patchText === undefined && typeof fields.patch === "string")
        fields.patchText = fields.patch;

    return fields;
}

/** Per-file changes for review, kept separate from the undo record in metadata.changes. */
export function toolMetadata(part: ToolPart) {
    const metadata = { ...part.metadata };
    if (part.name !== "apply_patch") return metadata;

    const changes = metadata.fileChanges ?? [];
    if (changes.length) metadata.files = changes;
    const only = changes.length === 1 ? changes[0].patch : undefined;
    if (typeof only === "string" && metadata.diff === undefined) metadata.diff = only;

    return metadata;
}
