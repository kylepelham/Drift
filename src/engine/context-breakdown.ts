import { promptPartText, toolInput } from "./parts";

import type { MessageEntry } from "./store";
import type { Part } from "./parts";

export type BreakdownKey = "system" | "user" | "assistant" | "tool";
export type BreakdownSegment = { key: BreakdownKey; tokens: number };

// Rough chars-per-token ratio, the same heuristic upstream's context tab uses.
const charsPerToken = 4;
// Upstream's allowance per tool-call argument, since inputs are not re-serialized here.
const charsPerToolArgument = 16;

function userChars(part: Part) {
    if (part.type === "task_result")
        return `Background task "${part.description}" ${part.outcome}:\n\n${part.text}`.length;

    const text = promptPartText(part);
    if (text !== undefined) return text.length;
    if (part.type === "file" && part.path) return part.path.length + 1;
    return 0;
}

function assistantChars(part: Part) {
    const text = promptPartText(part);
    if (text !== undefined) return { assistant: text.length, tool: 0 };
    if (part.type === "reasoning") return { assistant: part.text.length, tool: 0 };
    if (part.type !== "tool_call") return { assistant: 0, tool: 0 };
    const input = Object.keys(toolInput(part)).length * charsPerToolArgument;
    if (part.status === "done") return { assistant: 0, tool: input + (part.output ?? "").length };
    if (part.status === "error" || part.status === "denied")
        return { assistant: 0, tool: input + (part.output ?? "Failed").length };
    return { assistant: 0, tool: input };
}

/** Only messages since the latest compaction summary are still in the model's context. */
function liveEntries(entries: MessageEntry[]) {
    for (let index = entries.length - 1; index >= 0; index--) {
        const info = entries[index]!.info;
        if (info.role === "assistant" && info.summary) return entries.slice(index);
    }
    return entries;
}

function characterCounts(entries: MessageEntry[]) {
    const counts = { user: 0, assistant: 0, tool: 0 };
    for (const entry of liveEntries(entries)) {
        for (const part of entry.parts) {
            if (entry.info.role === "user") counts.user += userChars(part);
            else {
                const next = assistantChars(part);
                counts.assistant += next.assistant;
                counts.tool += next.tool;
            }
        }
    }
    return counts;
}

// Transcript categories are estimates; whatever they leave unexplained is system prompt plus tool schemas.
export function estimateContextBreakdown(entries: MessageEntry[], total: number): BreakdownSegment[] {
    if (total <= 0) return [];
    const chars = characterCounts(entries);
    const estimated = {
        user: Math.ceil(chars.user / charsPerToken),
        assistant: Math.ceil(chars.assistant / charsPerToken),
        tool: Math.ceil(chars.tool / charsPerToken),
    };
    const sum = estimated.user + estimated.assistant + estimated.tool;
    const scale = sum > total ? total / sum : 1;
    const user = Math.floor(estimated.user * scale);
    const assistant = Math.floor(estimated.assistant * scale);
    const tool = Math.floor(estimated.tool * scale);
    const segments: BreakdownSegment[] = [
        { key: "system", tokens: Math.max(0, total - user - assistant - tool) },
        { key: "user", tokens: user },
        { key: "assistant", tokens: assistant },
        { key: "tool", tokens: tool },
    ];
    return segments.filter((segment) => segment.tokens > 0);
}
