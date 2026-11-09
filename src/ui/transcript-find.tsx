import { createEffect, createMemo, createRoot, createSignal, on, onCleanup, onMount, Show, untrack } from "solid-js";
import { IconArrowDown, IconArrowUp, IconSearch, IconX } from "./icons";
import { selectedSession } from "../state/selection";
import { onKeybind } from "../state/keybinds";
import { t } from "../state/i18n";
import {
    firstOccurrenceIn,
    occurrenceAt,
    occurrenceFromRow,
    reanchorMatch,
    stepMatch,
    totalMatches,
    transcriptMatches,
    unitFingerprints,
    type FindOverrides,
    type TranscriptMatch,
    type TranscriptOccurrence,
} from "../state/transcript-search";

import type { MessageEntry } from "../engine/store";
import type { Part } from "../engine/parts";

// The bar belongs to the header while the matches belong to the transcript, so the query lives
// beside both. Only one conversation is open at a time, so a single value is enough.
const [open, setOpen] = createSignal(false);
const [query, setQuery] = createSignal("");
// What the search actually runs on. It trails the input so a fast typist does not pay for a full
// transcript scan, a scroll jump, and a highlight repaint on every keystroke.
const [needle, setNeedleValue] = createSignal("");
const [cursor, setCursor] = createSignal(-1);
const [matches, setMatches] = createSignal<TranscriptMatch[]>([]);
const [overrides, setOverrides] = createSignal<FindOverrides>(new Map());

export const findDebounceMs = 150;
let debounce: ReturnType<typeof setTimeout> | undefined;
let syncedNeedle = "";
let direction = 1;
// The message a search from the sidebar landed on, so its first occurrence is where the cursor starts.
let startAt: string | undefined;
let request: { sessionId: string; query: string; messageId?: string } | undefined;
let rows: MessageEntry[] = [];
let partsOfRow: (entry: MessageEntry) => Part[] = (entry) => entry.parts;

/** Counts belong to a query, so a new one starts from what the data says. */
function setNeedle(value: string) {
    setOverrides(new Map());
    setNeedleValue(value);
}

function settle(value: string) {
    if (debounce !== undefined) clearTimeout(debounce);
    debounce = undefined;
    startAt = undefined;
    // Clearing is instant so Escape and the clear button never leave stale highlights behind.
    if (!value.trim()) {
        setNeedle("");
        return;
    }
    debounce = setTimeout(() => setNeedle(value), findDebounceMs);
}

/** Runs a query still waiting out its pause; true when there was one, so Enter lands on its first match. */
function flushPending() {
    if (debounce === undefined) return false;
    clearTimeout(debounce);
    debounce = undefined;
    setNeedle(query());
    return true;
}

export function transcriptFindOpen() {
    return open();
}

export function transcriptFindNeedle() {
    return open() ? needle().trim() : "";
}

export function transcriptFindCursor() {
    return cursor();
}

/** The occurrence the cursor sits on; the timeline scrolls to it and paints it distinctly. */
export function activeFindOccurrence() {
    return occurrenceAt(matches(), cursor());
}

const forcedUnit = createRoot(() => createMemo(() => (open() ? activeFindOccurrence()?.unit : undefined)));

/** Whether a collapsed row holds the occurrence the cursor is on, so it opens until the cursor moves on. */
export function findOpens(unit: string) {
    return forcedUnit() === unit;
}

export function openTranscriptFind() {
    setOpen(true);
}

/** Opens the find bar on `sessionId` with `text`, starting at `messageId`: a sidebar search result being opened. */
export function findInConversation(sessionId: string, text: string, messageId?: string) {
    request = { sessionId, query: text, messageId };
    if (untrack(selectedSession) === sessionId) applyRequest(sessionId);
}

function applyRequest(sessionId: string | null) {
    const wanted = request;
    request = undefined;
    if (!wanted || wanted.sessionId !== sessionId) return;
    startAt = wanted.messageId;
    setOpen(true);
    setQuery(wanted.query);
    setNeedle(wanted.query);
}

export function closeTranscriptFind() {
    if (debounce !== undefined) clearTimeout(debounce);
    debounce = undefined;
    syncedNeedle = "";
    startAt = undefined;
    closeOpenedDetails();
    setOpen(false);
    setQuery("");
    setNeedle("");
    setMatches([]);
    setCursor(-1);
}

/**
 * Recomputes matches from the rows the timeline draws, `partsOf` naming the parts each draws and `viewTop` the
 * index of the row at the top of the view.
 *
 * Called by the timeline whenever its rows or the settled query change, so results follow a streaming reply and any
 * older page that loads. A new query starts from the first occurrence in view (or the first in a message a sidebar
 * search opened); a change under the same query keeps the cursor on the occurrence it was already at.
 *
 * The anchor reads run untracked: this executes inside the caller's effect, and reading the very signals it is about
 * to write would make each run retrigger the next forever.
 */
export function syncTranscriptMatches(
    timeline: MessageEntry[],
    partsOf: (entry: MessageEntry) => Part[],
    viewTop: () => number,
) {
    if (!open()) return;
    const value = needle();
    const found = transcriptMatches(timeline, value, partsOf, overrides());
    untrack(() => {
        rows = timeline;
        partsOfRow = partsOf;
        const fresh = value !== syncedNeedle;
        syncedNeedle = value;
        const anchor = fresh ? undefined : activeFindOccurrence();
        setMatches(found);
        const start = startAt === undefined ? -1 : firstOccurrenceIn(found, startAt);
        if (start >= 0) {
            startAt = undefined;
            setCursor(start);
            return;
        }
        setCursor(
            fresh ? occurrenceFromRow(found, timeline, viewTop()) : reanchorMatch(found, anchor, cursor(), direction),
        );
    });
}

/** Takes what the drawn units really show as their counts: markdown, clipped output and headers differ from the data. */
export function reconcileFindCounts(counts: ReadonlyMap<string, number>) {
    const current = new Map(untrack(matches).map((match) => [match.unit, match.count]));
    const changed = [...counts].filter(([unit, count]) => (current.get(unit) ?? 0) !== count);
    if (!changed.length) return;

    const fingerprints = unitFingerprints(rows, partsOfRow);
    const next = new Map(untrack(overrides));
    for (const [unit, count] of changed) {
        const fingerprint = fingerprints.get(unit);
        if (fingerprint !== undefined) next.set(unit, { fingerprint, count });
    }
    if (changed.some(([unit]) => fingerprints.has(unit))) setOverrides(next);
}

export function stepTranscriptFind(step: number) {
    direction = step;
    startAt = undefined;
    setCursor((current) => stepMatch(current, totalMatches(matches()), step));
}

const FIND_HIGHLIGHT = "drift-find";
const FIND_ACTIVE_HIGHLIGHT = "drift-find-active";

function highlightRegistry() {
    return typeof Highlight === "undefined" ? undefined : CSS.highlights;
}

export function clearFindHighlights() {
    const registry = highlightRegistry();
    registry?.delete(FIND_HIGHLIGHT);
    registry?.delete(FIND_ACTIVE_HIGHLIGHT);
}

/** A unit's own text nodes, joined, so a match split across markdown or syntax spans is still found. */
function unitText(unit: HTMLElement) {
    const nodes: Text[] = [];
    const starts: number[] = [];
    let text = "";
    const walker = document.createTreeWalker(unit, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const parent = node.parentElement;
        if (!parent || parent.closest("[data-find-ignore]") || parent.closest("[data-find-unit]") !== unit) continue;
        starts.push(text.length);
        nodes.push(node as Text);
        text += (node.nodeValue ?? "").toLowerCase();
    }
    return { nodes, starts, text };
}

/** Every occurrence of `value` in a unit, as ranges that may span several text nodes. */
export function unitRanges(unit: HTMLElement, value: string) {
    const { nodes, starts, text } = unitText(unit);
    const ranges: Range[] = [];
    let node = 0;
    const locate = (offset: number, end: boolean) => {
        while (node + 1 < nodes.length && (end ? starts[node + 1] < offset : starts[node + 1] <= offset)) node++;
        return { node: nodes[node], offset: offset - starts[node] };
    };
    for (let at = text.indexOf(value); at !== -1; at = text.indexOf(value, at + value.length)) {
        const start = locate(at, false);
        const end = locate(at + value.length, true);
        const range = new Range();
        range.setStart(start.node, start.offset);
        range.setEnd(end.node, end.offset);
        ranges.push(range);
    }
    return ranges;
}

/** A unit shows all it holds unless it, or a row inside it, is folded. */
function unitComplete(unit: HTMLElement) {
    return unit.dataset.findOpen !== "false" && !unit.querySelector('[data-find-open="false"]');
}

/**
 * Paints every occurrence in the mounted units and returns the active one, with what each fully drawn unit holds.
 *
 * The timeline is virtualized, so this only walks the rows that exist; scrolling and redraws repaint. Highlights are
 * ranges in the CSS highlight registry rather than injected markup, so streamed markdown is never mutated. A cursor
 * past what the unit shows lands on its last occurrence until the recount moves it on.
 */
export function paintFindHighlights(container: HTMLElement, needle: string, active: TranscriptOccurrence | undefined) {
    const counts = new Map<string, number>();
    const registry = highlightRegistry();
    const value = needle.toLowerCase();
    if (!registry || !value) {
        clearFindHighlights();
        return { counts };
    }

    const ranges: Range[] = [];
    let activeRange: Range | undefined;
    let activeUnit: HTMLElement | undefined;
    for (const unit of container.querySelectorAll<HTMLElement>("[data-find-unit]")) {
        const found = unitRanges(unit, value);
        ranges.push(...found);
        const isActive = unit.dataset.findUnit === active?.unit;
        if (isActive || unitComplete(unit)) counts.set(unit.dataset.findUnit!, found.length);
        if (!isActive) continue;
        activeUnit = unit;
        activeRange = found[Math.min(active!.index, found.length - 1)];
    }
    registry.set(FIND_HIGHLIGHT, new Highlight(...ranges));
    if (activeRange) registry.set(FIND_ACTIVE_HIGHLIGHT, new Highlight(activeRange));
    else registry.delete(FIND_ACTIVE_HIGHLIGHT);
    return { active: activeRange, unit: activeUnit, counts };
}

// Disclosures find opened to show an occurrence; they close again once the cursor moves out of them.
let openedDetails: HTMLDetailsElement[] = [];

function closeOpenedDetails(keep?: Node) {
    openedDetails = openedDetails.filter((details) => {
        if (keep && details.contains(keep)) return true;
        details.open = false;
        return false;
    });
}

/** Opens what folds the range away, then centres it in each box that scrolls it, the transcript last. */
export function revealFindRange(range: Range, scroller: HTMLElement, bottomInset: number) {
    const parent = range.startContainer.parentElement;
    closeOpenedDetails(parent ?? undefined);
    for (
        let details = parent?.closest<HTMLDetailsElement>("details:not([open])");
        details;
        details = parent?.closest<HTMLDetailsElement>("details:not([open])")
    ) {
        details.open = true;
        openedDetails.push(details);
    }

    for (let box = parent; box && box !== scroller; box = box.parentElement) {
        if (box.scrollHeight > box.clientHeight + 1 || box.scrollWidth > box.clientWidth + 1) centre(box, range, 0);
    }
    centre(scroller, range, bottomInset);
}

function centre(box: HTMLElement, range: Range, bottomInset: number) {
    const target = range.getBoundingClientRect();
    const frame = box.getBoundingClientRect();
    if (box.scrollHeight > box.clientHeight + 1) {
        const visible = box.clientHeight - bottomInset;
        box.scrollTop += target.top - frame.top - (visible - target.height) / 2;
    }
    if (box.scrollWidth > box.clientWidth + 1) box.scrollLeft += target.left - frame.left - box.clientWidth / 3;
}

/** Whether the range shows inside the transcript, above the dock. */
export function findRangeVisible(range: Range, scroller: HTMLElement, bottomInset: number) {
    const target = range.getBoundingClientRect();
    const frame = scroller.getBoundingClientRect();
    return target.height > 0 && target.top >= frame.top && target.bottom <= frame.bottom - bottomInset;
}

export function TranscriptFindBar() {
    let input: HTMLInputElement | undefined;

    onMount(() => {
        onKeybind("findInSession", () => {
            if (!selectedSession()) return;
            setOpen(true);
            queueMicrotask(() => {
                input?.focus();
                input?.select();
            });
        });
    });

    // Switching conversations discards the search: its matches belong to a transcript that is gone.
    createEffect(() => {
        const id = selectedSession();
        closeTranscriptFind();
        applyRequest(id);
    });

    createEffect(
        on(open, (value) => {
            if (value) queueMicrotask(() => input?.focus());
        }),
    );
    onCleanup(closeTranscriptFind);

    const total = createMemo(() => totalMatches(matches()));
    const position = createMemo(() => (cursor() >= 0 ? cursor() + 1 : 0));

    return (
        <Show when={open()}>
            <div class="transcript-find pointer-events-auto flex items-center gap-1 rounded-md border border-edge bg-overlay px-1.5 py-0.5 shadow-lg shadow-black/20">
                <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                <input
                    ref={input}
                    class="w-40 bg-transparent py-0.5 text-xs text-ink outline-none placeholder:text-ink-faint"
                    placeholder={t("drift.search.transcript.placeholder")}
                    value={query()}
                    onInput={(event) => {
                        setQuery(event.currentTarget.value);
                        settle(event.currentTarget.value);
                    }}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") {
                            event.preventDefault();
                            closeTranscriptFind();
                            return;
                        }
                        if (event.key !== "Enter") return;
                        event.preventDefault();
                        if (!flushPending()) stepTranscriptFind(event.shiftKey ? -1 : 1);
                    }}
                />
                <span class="shrink-0 tabular-nums text-[length:calc(0.65rem*var(--ui-text-scale,1))] text-ink-faint">
                    {needle().trim() ? `${position()}/${total()}` : ""}
                </span>
                <button
                    class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
                    title={t("drift.search.previous")}
                    disabled={!total()}
                    onClick={() => stepTranscriptFind(-1)}
                >
                    <IconArrowUp class="size-3" />
                </button>
                <button
                    class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
                    title={t("drift.search.next")}
                    disabled={!total()}
                    onClick={() => stepTranscriptFind(1)}
                >
                    <IconArrowDown class="size-3" />
                </button>
                <button
                    class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink"
                    title={t("drift.search.close")}
                    onClick={closeTranscriptFind}
                >
                    <IconX class="size-3" />
                </button>
            </div>
        </Show>
    );
}
