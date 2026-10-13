import { promptPartText, type ToolPart } from "./parts";

import type { MessageEntry } from "./store";

/** What a request carried that the one before it did not: the likeliest place a safety refusal was set off. */
export type RefusalTrigger = { prompts: string[]; files: string[]; tools: ToolPart[] };

/**
 * Walks back from the refused reply to the last reply that went through. Everything after it is new in the
 * refused request: the user's messages since, or the results of the calls that reply made.
 */
export function refusalTrigger(entries: MessageEntry[], messageId: string): RefusalTrigger | undefined {
    const index = entries.findIndex((entry) => entry.info.id === messageId);
    if (index < 0) return undefined;

    const trigger: RefusalTrigger = { prompts: [], files: [], tools: [] };
    for (const entry of entries.slice(0, index).reverse()) {
        if (entry.info.role === "user") {
            addPrompt(trigger, entry);
            continue;
        }

        trigger.tools = entry.parts.filter((part): part is ToolPart => part.type === "tool_call");
        // A reply that failed before it said anything was never accepted either; keep looking past it.
        if (trigger.tools.length || entry.info.status === "done") break;
    }

    trigger.prompts.reverse();
    const empty = !trigger.prompts.length && !trigger.files.length && !trigger.tools.length;

    return empty ? undefined : trigger;
}

function addPrompt(trigger: RefusalTrigger, entry: MessageEntry) {
    const text = entry.parts
        .map(promptPartText)
        .filter((part): part is string => !!part?.trim())
        .join("\n\n");
    if (text) trigger.prompts.push(text);

    for (const part of entry.parts) if (part.type === "file") trigger.files.push(part.name);
}
