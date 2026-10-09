/** Searches message data so virtualized rows can be found even when they have no mounted DOM. */

import { clarificationAnswer } from "../ui/clarification-answer";

import type { MessageEntry } from "../engine/store";

export type TranscriptMatch = { messageId: string; count: number };

/** A single occurrence: which message it is in and which occurrence within that message. */
export type TranscriptOccurrence = { messageId: string; index: number };

/** Returns readable message text, reasoning and finished tool output; engine-only parts such as worker results are left out. */
export function entrySearchText(entry: MessageEntry) {
    const clarification = clarificationAnswer(entry);
    if (clarification) return clarification.text;

    const parts: string[] = [];
    for (const part of entry.parts) {
        if (part.type === "text" || part.type === "reasoning" || part.type === "nudge") {
            if (part.text) parts.push(part.text);
            continue;
        }
        if (part.type === "tool_call" && part.status === "done" && typeof part.output === "string")
            parts.push(part.output);
    }

    return parts.join("\n");
}

/** Caches lowercased text between keystrokes, using a length fingerprint for streaming changes. */
const loweredCache = new WeakMap<MessageEntry, { fingerprint: number | string; lower: string }>();

function textFingerprint(entry: MessageEntry) {
    let total = entry.parts.length;

    for (const part of entry.parts) {
        if (part.type === "text" || part.type === "reasoning" || part.type === "nudge") total += part.text.length;
        else if (part.type === "tool_call" && part.status === "done" && typeof part.output === "string")
            total += part.output.length;
    }

    return total;
}

function loweredSearchText(entry: MessageEntry) {
    const clarification = clarificationAnswer(entry);
    // Clarification metadata can change independently of the stored protocol text.
    const fingerprint = clarification?.text ?? textFingerprint(entry);
    const cached = loweredCache.get(entry);
    if (cached && cached.fingerprint === fingerprint) return cached.lower;

    const lower = (clarification?.text ?? entrySearchText(entry)).toLowerCase();
    loweredCache.set(entry, { fingerprint, lower });

    return lower;
}

function countIn(lowerHaystack: string, lowerNeedle: string) {
    let count = 0;
    for (
        let at = lowerHaystack.indexOf(lowerNeedle);
        at !== -1;
        at = lowerHaystack.indexOf(lowerNeedle, at + lowerNeedle.length)
    )
        count++;
    return count;
}

/** Messages containing `query`, in transcript order, with how many times each one matches. */
export function transcriptMatches(entries: MessageEntry[], query: string): TranscriptMatch[] {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    const found: TranscriptMatch[] = [];
    for (const entry of entries) {
        const count = countIn(loweredSearchText(entry), needle);
        if (count > 0) found.push({ messageId: entry.info.id, count });
    }
    return found;
}

export function totalMatches(matches: TranscriptMatch[]) {
    return matches.reduce((total, match) => total + match.count, 0);
}

/** Resolves a flat occurrence index without skipping repeated matches inside one message. */
export function occurrenceAt(matches: TranscriptMatch[], cursor: number): TranscriptOccurrence | undefined {
    if (cursor < 0) return undefined;

    let before = 0;
    for (const match of matches) {
        if (cursor < before + match.count) return { messageId: match.messageId, index: cursor - before };
        before += match.count;
    }
    return undefined;
}

/** Moves the occurrence cursor with wrapping; an empty result set has no cursor. */
export function stepMatch(current: number, total: number, step: number) {
    if (total <= 0) return -1;
    return (((current + step) % total) + total) % total;
}

/** Reanchors the occurrence cursor by message id when streaming or paging changes the results. */
export function reanchorMatch(
    matches: TranscriptMatch[],
    previous: TranscriptOccurrence | undefined,
    fallback: number,
) {
    const total = totalMatches(matches);
    if (!total) return -1;
    if (previous) {
        let before = 0;
        for (const match of matches) {
            if (match.messageId === previous.messageId) return before + Math.min(previous.index, match.count - 1);
            before += match.count;
        }
    }
    return Math.min(Math.max(fallback, 0), total - 1);
}
