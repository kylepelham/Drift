/** Searches what the transcript draws, part by part, so rows that are not mounted can be found and counted. */

import { clarificationAnswer } from "../ui/clarification-answer";
import { toolInfo } from "../ui/tool-labels";
import { showReasoning } from "./prefs";

import type { MessageEntry } from "../engine/store";
import type { Part } from "../engine/parts";

/** A searchable piece of a row: a user message whole, or one part of an assistant row (`unit` is its id). */
export type TranscriptMatch = { rowId: string; unit: string; messageId: string; count: number };

/** A single occurrence: which unit it is in and which occurrence within that unit. */
export type TranscriptOccurrence = { rowId: string; unit: string; messageId: string; index: number };

/** What the drawn unit really holds, once it has been on screen, keyed by unit; stale once its text changes. */
export type FindOverrides = ReadonlyMap<string, { fingerprint: string; count: number }>;

const hiddenTools = new Set(["todowrite", "todoread"]);

/** A user message's readable text; engine-only parts such as worker results are left out. */
export function entrySearchText(entry: MessageEntry) {
    const clarification = clarificationAnswer(entry);
    if (clarification) return clarification.text;

    return entry.parts
        .map(partSearchText)
        .filter((text) => !!text)
        .join("\n");
}

/** The text a part shows: prose, reasoning while it is shown, a tool row's header and its result. */
function partSearchText(part: Part) {
    if (part.type === "text" || part.type === "nudge") return part.text;
    if (part.type === "reasoning") return showReasoning() ? part.text : "";
    if (part.type !== "tool_call" || hiddenTools.has(part.name)) return "";

    const info = toolInfo(part);
    const diff = part.metadata?.diff;
    const ended = part.status === "done" || part.status === "error";
    return [info.called ?? info.title, info.subtitle, ended ? part.output : "", typeof diff === "string" ? diff : ""]
        .filter((text) => !!text)
        .join("\n");
}

function partFingerprint(part: Part) {
    if (part.type === "tool_call") return `${part.status}:${part.output?.length ?? 0}`;
    if (part.type === "reasoning") return `${showReasoning()}:${part.text.length}`;

    return "text" in part && typeof part.text === "string" ? `${part.text.length}` : "";
}

type Unit = { rowId: string; unit: string; messageId: string; fingerprint: string; text: () => string };

/** Caches lowered text between keystrokes, keyed by the unit's source and checked by a fingerprint for streaming. */
const loweredCache = new WeakMap<object, { fingerprint: string; lower: string }>();

function lowered(source: object, unit: Unit) {
    const cached = loweredCache.get(source);
    if (cached && cached.fingerprint === unit.fingerprint) return cached.lower;

    const lower = unit.text().toLowerCase();
    loweredCache.set(source, { fingerprint: unit.fingerprint, lower });
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

/** A user row is one unit; an assistant row is a unit per part it draws, which may come from a later step. */
function rowUnits(entry: MessageEntry, partsOf: (entry: MessageEntry) => Part[]) {
    const rowId = entry.info.id;
    if (entry.info.role === "user") {
        const clarification = clarificationAnswer(entry)?.text;
        const fingerprint = clarification ?? entry.parts.map(partFingerprint).join("|");
        const text = () => clarification ?? entrySearchText(entry);
        return [{ source: entry as object, unit: { rowId, unit: rowId, messageId: rowId, fingerprint, text } }];
    }

    return partsOf(entry).map((part) => ({
        source: part as object,
        unit: {
            rowId,
            unit: part.id,
            messageId: part.messageId,
            fingerprint: partFingerprint(part),
            text: () => partSearchText(part),
        },
    }));
}

/**
 * Units containing `query`, in transcript order, with how many times each matches. `partsOf` names the parts each row
 * draws; `overrides` holds what a drawn unit was found to show, which wins while its text is unchanged.
 */
export function transcriptMatches(
    rows: MessageEntry[],
    query: string,
    partsOf: (entry: MessageEntry) => Part[] = (entry) => entry.parts,
    overrides: FindOverrides = new Map(),
): TranscriptMatch[] {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];

    const found: TranscriptMatch[] = [];
    for (const entry of rows) {
        for (const { source, unit } of rowUnits(entry, partsOf)) {
            const known = overrides.get(unit.unit);
            const count =
                known?.fingerprint === unit.fingerprint ? known.count : countIn(lowered(source, unit), needle);
            if (count > 0) found.push({ rowId: unit.rowId, unit: unit.unit, messageId: unit.messageId, count });
        }
    }
    return found;
}

/** The fingerprint a unit's count was taken at, so an override is dropped once the unit changes. */
export function unitFingerprints(rows: MessageEntry[], partsOf: (entry: MessageEntry) => Part[]) {
    const fingerprints = new Map<string, string>();
    for (const entry of rows)
        for (const { unit } of rowUnits(entry, partsOf)) fingerprints.set(unit.unit, unit.fingerprint);
    return fingerprints;
}

export function totalMatches(matches: TranscriptMatch[]) {
    return matches.reduce((total, match) => total + match.count, 0);
}

/** Resolves a flat occurrence index without skipping repeated matches inside one unit. */
export function occurrenceAt(matches: TranscriptMatch[], cursor: number): TranscriptOccurrence | undefined {
    if (cursor < 0) return undefined;

    let before = 0;
    for (const match of matches) {
        if (cursor < before + match.count)
            return { rowId: match.rowId, unit: match.unit, messageId: match.messageId, index: cursor - before };
        before += match.count;
    }
    return undefined;
}

/** The first occurrence in `messageId`, whether it is the row's own message or a step drawn inside another row. */
export function firstOccurrenceIn(matches: TranscriptMatch[], messageId: string) {
    let before = 0;
    for (const match of matches) {
        if (match.messageId === messageId || match.rowId === messageId) return before;
        before += match.count;
    }
    return -1;
}

/** Where a new search starts: the first occurrence from the row at the top of the view down, else the last above it. */
export function occurrenceFromRow(matches: TranscriptMatch[], rows: MessageEntry[], top: number) {
    const order = new Map(rows.map((row, index) => [row.info.id, index]));
    let before = 0;
    for (const match of matches) {
        if ((order.get(match.rowId) ?? 0) >= top) return before;
        before += match.count;
    }
    return before - 1;
}

/** Moves the occurrence cursor with wrapping; an empty result set has no cursor. */
export function stepMatch(current: number, total: number, step: number) {
    if (total <= 0) return -1;
    return (((current + step) % total) + total) % total;
}

/**
 * Reanchors the cursor by unit when streaming, paging or a recount changes the results. A unit that no longer matches
 * leaves the cursor where it was, which is the next occurrence; going back, the one before it.
 */
export function reanchorMatch(
    matches: TranscriptMatch[],
    previous: TranscriptOccurrence | undefined,
    fallback: number,
    direction = 1,
) {
    const total = totalMatches(matches);
    if (!total) return -1;
    if (!previous) return Math.min(Math.max(fallback, 0), total - 1);

    let before = 0;
    for (const match of matches) {
        if (match.unit === previous.unit) return before + Math.min(previous.index, match.count - 1);
        before += match.count;
    }
    const moved = direction < 0 ? fallback - 1 : fallback;
    return ((moved % total) + total) % total;
}
