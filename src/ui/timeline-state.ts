import { assistantFlowContinues } from "./message-groups";
import { messageProblem } from "../engine/messages";
import { messageVisible } from "./message";
import { partVisible } from "./parts";

import type { MessageEntry, SessionStatus } from "../engine/store";
import type { PartGroup } from "./message-groups";

/**
 * A spawned thread starts with a copy of its source; copied messages keep their times, so they are older than the
 * thread.
 */
export function copiedCount(entries: MessageEntry[], threadCreated: number) {
    const own = entries.findIndex((entry) => entry.info.createdAt >= threadCreated);
    return own < 0 ? entries.length : own;
}

export function timelineEntries(entries: MessageEntry[], activeMessageID?: string | null) {
    return entries.filter((entry) => entry.info.id === activeMessageID || messageVisible(entry));
}

export function timelinePitch(entry: MessageEntry, next?: MessageEntry) {
    if (!next) return "none" as const;
    return assistantFlowContinues(entry, next) ? ("part" as const) : ("turn" as const);
}

export function timelineParts(entry: MessageEntry, groups?: PartGroup[]) {
    if (entry.info.role !== "assistant" || !groups) return entry.parts;
    return groups.flatMap((group) => ("explored" in group ? group.explored : [group.part]));
}

export function timelineRowVisible(
    entry: MessageEntry,
    groups: PartGroup[] | undefined,
    next: MessageEntry | undefined,
    active?: string,
) {
    if (entry.info.role === "user") return true;
    // A failure the session has moved past (a retry, or a new prompt) is no longer news.
    if (failedAttempt(entry) && next) return false;

    const info = entry.info;

    return (
        !!groups?.length ||
        !!info.summary ||
        !!messageProblem(info) ||
        entry.info.id === active ||
        (!!info.finishedAt && next?.info.role !== "assistant")
    );
}

/** A reply that failed before showing anything: what the engine retries. */
export function failedAttempt(entry: MessageEntry) {
    if (entry.info.role !== "assistant") return false;
    const problem = messageProblem(entry.info);
    return !!problem && !problem.interrupted && !entry.parts.some(partVisible);
}

/** The retry line stays up while the attempt after a run of failures is in flight, until it fails or shows output. */
export function retryInFlight(
    entries: MessageEntry[],
    running?: string,
): Extract<SessionStatus, { type: "retry" }> | undefined {
    const index = entries.findIndex((entry) => entry.info.id === running);
    const current = entries[index];
    if (!current || current.info.role !== "assistant" || current.parts.some(partVisible)) return undefined;

    let attempt = 0;
    while (index - attempt - 1 >= 0 && failedAttempt(entries[index - attempt - 1])) attempt += 1;

    if (attempt === 0) return undefined;

    const last = entries[index - 1].info;

    return { type: "retry", attempt, message: messageProblem(last)?.text ?? "An error occurred", next: 0 };
}

export function thinkingAfterMessage(entries: MessageEntry[], status?: string) {
    return thinkingState(entries, status)?.messageID ?? null;
}

export function thinkingState(entries: MessageEntry[], status?: string) {
    if (status !== "busy" && status !== "retry") return null;

    const newestFirst = [...entries].reverse();
    const unfinished = newestFirst.find((entry) => entry.info.role === "assistant" && !entry.info.finishedAt);
    // A user turn newer than every reply anchors the indicator; otherwise the running reply does.
    const anchor =
        unfinished ?? newestFirst.find((entry) => entry.info.role === "user" || entry.info.role === "assistant");
    if (!anchor) return null;

    const assistants = anchor.info.role === "assistant" ? [anchor] : [];
    const error = assistants.find((entry) => {
        const problem = messageProblem(entry.info);
        return problem && !problem.interrupted;
    });
    // After a failure the turn is over, unless another attempt is already under way.
    if (status === "busy" && error && !(unfinished && !messageProblem(unfinished.info))) return null;

    const heading = assistants
        .flatMap((entry) => entry.parts)
        .map((part) => (part.type === "reasoning" && part.text ? reasoningHeading(part.text) : undefined))
        .find((value): value is string => !!value);
    const owner = unfinished ?? assistants.at(-1) ?? anchor;
    // The summary reply, or before it arrives the user boundary, carries the shimmer onto its divider.
    const compaction =
        owner.info.role === "assistant"
            ? !!(owner.info as { summary?: boolean }).summary
            : owner.parts.some((part) => part.type === "compaction");

    return { messageID: owner.info.id, heading, compaction };
}

/** Whether a row draws a compaction divider that can carry the shimmer; summaries only when collapsible. */
export function compactionThinkingRow(entry: MessageEntry, collapsible: boolean) {
    if (entry.info.role === "user") return entry.parts.some((part) => part.type === "compaction");
    return collapsible && !!entry.info.summary;
}

export function reasoningHeading(text: string) {
    const markdown = text.replace(/\r\n?/g, "\n");
    const html = markdown.match(/<h[1-6][^>]*>([\s\S]*?)<\/h[1-6]>/i);
    if (html?.[1]) {
        const value = cleanHeading(html[1].replace(/<[^>]+>/g, " "));
        if (value) return value;
    }

    const atx = markdown.match(/^\s{0,3}#{1,6}[ \t]+(.+?)(?:[ \t]+#+[ \t]*)?$/m);
    if (atx?.[1]) {
        const value = cleanHeading(atx[1]);
        if (value) return value;
    }

    const setext = markdown.match(/^([^\n]+)\n(?:=+|-+)\s*$/m);
    if (setext?.[1]) {
        const value = cleanHeading(setext[1]);
        if (value) return value;
    }

    const strong = markdown.match(/^\s*(?:\*\*|__)(.+?)(?:\*\*|__)\s*$/m);
    if (strong?.[1]) return cleanHeading(strong[1]) || undefined;
}

function cleanHeading(value: string) {
    return value
        .replace(/`([^`]+)`/g, "$1")
        .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
        .replace(/[*_~]+/g, "")
        .replace(/\s+/g, " ")
        .trim();
}
