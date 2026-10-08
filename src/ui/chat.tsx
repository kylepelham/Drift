import { batch, createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, untrack } from "solid-js";
import { accumulatedWheelTarget, chatWheelEvent, normalizedWheelDelta, type ForwardedWheel } from "./chat-wheel";
import { compareMessages, messageRevisionKey, type MessageEntry } from "../engine/store";
import { collapseCompaction, compactionCollapsed } from "../state/prefs";
import { clearReveal, revealTarget } from "./session-search";
import { groupAssistantEntries } from "./message-groups";
import { createRevertBackfill } from "./revert-backfill";
import { selectedSession } from "../state/selection";
import { messageProblem } from "../engine/messages";
import { codeFontSize } from "../state/code";
import { TextShimmer } from "./text-shimmer";
import { messageVisible } from "./message";
import { EmptyState } from "./chat-empty";
import { IconArrowDown } from "./icons";
import { useEngine } from "../engine";
import { Row } from "./timeline-row";
import { t } from "../state/i18n";
import {
    estimatedRow,
    estimatedTimelineRow,
    resizeCompensation,
    scrollGestureSticks,
    shouldShowScrollToBottom,
    snapVirtualViewport,
    transcriptRevision,
    virtualRange,
} from "./timeline-virtual";
import {
    activeFindMessage,
    activeFindOccurrence,
    clearFindHighlights,
    paintFindHighlights,
    scrollFindOccurrence,
    syncTranscriptMatches,
    transcriptFindNeedle,
} from "./transcript-find";
import {
    copiedCount,
    retryInFlight,
    thinkingState,
    timelineEntries,
    timelineParts,
    timelineRowVisible,
} from "./timeline-state";

import type { Part } from "../engine/parts";

const loadOlderAt = 1200;
// How long after the last wheel event a gesture counts as ongoing, so momentum is not a stop.
const gestureWindowMs = 250;
// The transcript column is padded by pt-14; row offsets are measured from below that padding.
const headerOffset = 56;
// Breathing room above a message jumped to from search, so it does not sit under the header.
const findMargin = 24;

export function Chat() {
    const engine = useEngine();
    const [shownCopies, setShownCopies] = createSignal<ReadonlySet<string>>(new Set());
    const allEntries = createMemo(() => {
        const id = selectedSession();
        if (!id) return [];
        const revertedAt = engine.state.sessions[id]?.revert?.messageId;
        const transcript = engine.state.transcripts[id] ?? [];
        // Nested part replacement skips transcript memos; reading each revision key subscribes to it.
        for (const entry of transcript) engine.state.revisions[messageRevisionKey(id, entry.info.id)];
        const boundary = revertedAt ? transcript.find((entry) => entry.info.id === revertedAt) : undefined;
        const sorted = [...transcript]
            .filter((entry) => {
                if (!revertedAt) return true;
                if (boundary) return compareMessages(entry, boundary) < 0;
                return entry.info.id < revertedAt;
            })
            .sort(compareMessages);
        return sorted;
    });
    const spawnedCopy = createMemo(() => {
        const id = selectedSession();
        const session = id ? engine.state.sessions[id] : undefined;
        const source = id ? engine.state.links[id] : undefined;
        if (!id || !session || !source) return undefined;
        const copied = copiedCount(allEntries(), session.createdAt);
        if (!copied) return undefined;
        const list = allEntries();
        const shown = shownCopies().has(id);
        return {
            id,
            copied,
            ownID: list[copied]?.info.id,
            headerID: shown ? list[0].info.id : list[copied]?.info.id,
            copiedIDs: new Set(shown ? list.slice(0, copied).map((entry) => entry.info.id) : []),
            source: engine.state.sessions[source]?.title ?? "",
            shown,
        };
    });
    const entries = createMemo(() => {
        const copy = spawnedCopy();
        return copy?.ownID && !copy.shown ? allEntries().slice(copy.copied) : allEntries();
    });
    const toggleCopy = (id: string) =>
        setShownCopies((shown) => {
            const next = new Set(shown);
            if (!next.delete(id)) next.add(id);
            return next;
        });
    const sessionError = createMemo(() => {
        const id = selectedSession();
        if (!id) return null;
        const error = engine.state.errors[id];
        if (!error) return null;
        const latest = entries().at(-1);
        if (latest?.info.role !== "assistant") return error;
        const problem = messageProblem(latest.info);
        return problem && !problem.interrupted ? null : error;
    });
    const thinking = createMemo(() => {
        const id = selectedSession();
        return id ? thinkingState(entries(), engine.state.status[id]?.type) : null;
    });
    const retry = createMemo(() => {
        const id = selectedSession();
        const status = id ? engine.state.status[id] : undefined;
        if (status?.type === "retry") return status;
        return status?.type === "busy" ? retryInFlight(entries(), thinking()?.messageID) : undefined;
    });
    const timelineSource = createMemo(() => timelineEntries(entries(), thinking()?.messageID));
    const assistantGroups = createMemo(() => groupAssistantEntries(timelineSource()));
    const timeline = createMemo(() => {
        const source = timelineSource();
        const groups = assistantGroups();
        const active = thinking()?.messageID;
        return source.filter((entry, index) =>
            timelineRowVisible(entry, groups.get(entry.info.id), source[index + 1], active),
        );
    });
    const nextEntries = createMemo(() => {
        const list = timeline();
        return new Map(list.map((entry, index) => [entry.info.id, list[index + 1]]));
    });
    const thinkingOnly = (entry?: MessageEntry) =>
        !!entry && thinking()?.messageID === entry.info.id && !messageVisible(entry);
    const collapsedSummary = (entry: MessageEntry) =>
        entry.info.role === "assistant" && !!entry.info.summary && collapseCompaction() && compactionCollapsed();

    createEffect(() => {
        const id = selectedSession();
        const known = !!id && !!engine.state.sessions[id];
        if (known && engine.state.connection === "online") void engine.actions.openSession(id);
    });

    const revertBackfill = createRevertBackfill(engine, entries);

    let scroller!: HTMLDivElement;
    const [stick, setStick] = createSignal(true);
    const [awayFromBottom, setAwayFromBottom] = createSignal(false);
    const [viewTop, setViewTop] = createSignal(0);
    const [viewHeight, setViewHeight] = createSignal(800);
    const heights = new Map<string, number>();
    // Estimates are cached per message revision; offsets rebuild on every part delta.
    const estimates = new Map<
        string,
        { rev?: number; fontSize: number; thinking: boolean; collapsed: boolean; value: number }
    >();
    const [measured, setMeasured] = createSignal(0);
    let loadingOlder = false;

    function rowEstimate(entry: MessageEntry, parts: Part[], fontSize: number, thinking: boolean, collapsed: boolean) {
        const rev = engine.state.revisions[messageRevisionKey(entry.info.sessionId, entry.info.id)];
        const cached = estimates.get(entry.info.id);
        if (
            cached &&
            cached.rev === rev &&
            cached.fontSize === fontSize &&
            cached.thinking === thinking &&
            cached.collapsed === collapsed
        )
            return cached.value;
        const value = estimatedTimelineRow(entry, fontSize, parts, thinking, collapsed);
        estimates.set(entry.info.id, { rev, fontSize, thinking, collapsed, value });
        return value;
    }

    const offsets = createMemo(() => {
        measured();
        const list = timeline();
        const fontSize = codeFontSize();
        const result = new Array<number>(list.length + 1);
        result[0] = 0;
        const groups = assistantGroups();
        for (let index = 0; index < list.length; index++) {
            const entry = list[index];
            const parts = timelineParts(entry, groups.get(entry.info.id));
            result[index + 1] =
                result[index] +
                (heights.get(entry.info.id) ??
                    rowEstimate(entry, parts, fontSize, thinkingOnly(entry), collapsedSummary(entry)));
        }
        return result;
    });

    const range = createMemo(() => {
        return virtualRange(offsets(), viewTop(), viewHeight());
    });

    const slice = createMemo(() => timeline().slice(range().start, range().end));

    const observer = new ResizeObserver((observations) => {
        let deltaAbove = 0;
        let changed = false;
        const viewportTop = scroller.getBoundingClientRect().top;
        for (const observation of observations) {
            const row = observation.target as HTMLElement;
            const id = row.dataset.mid;
            if (!id) continue;
            const next = observation.borderBoxSize[0]?.blockSize ?? row.offsetHeight;
            if (next === 0) continue;
            const entry = untrack(timeline).find((item) => item.info.id === id);
            const parts = entry ? timelineParts(entry, untrack(assistantGroups).get(id)) : undefined;
            const previous =
                heights.get(id) ??
                (entry
                    ? estimatedTimelineRow(
                          entry,
                          untrack(codeFontSize),
                          parts,
                          untrack(() => thinkingOnly(entry)),
                          untrack(() => collapsedSummary(entry)),
                      )
                    : estimatedRow);
            if (Math.abs(next - previous) < 1) continue;
            heights.set(id, next);
            changed = true;
            deltaAbove += resizeCompensation(previous, next, row.getBoundingClientRect().bottom, viewportTop);
        }
        if (!changed) return;
        setMeasured((value) => value + 1);
        if (deltaAbove !== 0 && !untrack(stick)) scroller.scrollTop += deltaAbove;
    });
    onCleanup(() => observer.disconnect());

    const viewportObserver = new ResizeObserver(() => {
        // Snap before publishing: browser clamping as the dock grows would otherwise make the range jump.
        if (untrack(stick)) snapViewportToBottom();
        else publishViewport();
        const top = scroller.scrollTop;
        if (untrack(stick)) return;
        const distance = scroller.scrollHeight - top - scroller.clientHeight;
        setAwayFromBottom(shouldShowScrollToBottom(distance));
    });
    onMount(() => viewportObserver.observe(scroller));
    onCleanup(() => viewportObserver.disconnect());

    function measureRow(element: HTMLDivElement) {
        observer.observe(element);
    }

    function publishViewport() {
        batch(() => {
            setViewTop(scroller.scrollTop);
            setViewHeight(scroller.clientHeight);
        });
    }

    function snapViewportToBottom() {
        snapVirtualViewport(scroller, (top, height) => {
            batch(() => {
                setViewTop(top);
                setViewHeight(height);
            });
        });
    }

    // Search reads the whole transcript, so results follow streaming output and older pages.
    createEffect(() => syncTranscriptMatches(entries()));

    const findHighlight = createMemo(() => activeFindMessage() ?? revealTarget(selectedSession() ?? ""));

    // Highlights repaint a frame after the query, cursor or mounted rows change.
    let findRaf = 0;
    let scrolledFindOccurrence = "";
    createEffect(() => {
        const value = transcriptFindNeedle();
        const occurrence = activeFindOccurrence();
        const target = occurrence ? JSON.stringify([value, occurrence.messageId, occurrence.index]) : "";
        viewTop();
        measured();
        entries();
        cancelAnimationFrame(findRaf);
        if (!value) {
            scrolledFindOccurrence = "";
            clearFindHighlights();
            return;
        }
        findRaf = requestAnimationFrame(() => {
            const active = paintFindHighlights(scroller, value, occurrence);
            // Scroll to each occurrence once; repaints from the user's own scrolling must not pull back.
            if (active && target !== scrolledFindOccurrence) {
                scrolledFindOccurrence = target;
                scrollFindOccurrence(active);
            }
        });
    });
    onCleanup(() => {
        cancelAnimationFrame(findRaf);
        clearFindHighlights();
    });

    /**
     * Brings a message into view by index rather than by element: the row is usually unmounted, so
     * there is nothing to call `scrollIntoView` on until after the jump lands.
     *
     * Rows above the target that have never been measured contribute estimated heights, so the first
     * jump is approximate. Re-reading the offsets on the next frame, once the real heights have been
     * observed, settles it without a visible second scroll in the common case.
     */
    function scrollToMessage(messageId: string) {
        const index = timeline().findIndex((entry) => entry.info.id === messageId);
        if (index < 0) return false;
        const place = () => {
            const target = Math.max(0, headerOffset + offsets()[index] - findMargin);
            scroller.scrollTop = target;
            publishViewport();
        };
        batch(() => {
            setStick(false);
            setAwayFromBottom(true);
        });
        place();
        requestAnimationFrame(place);
        return true;
    }

    createEffect(() => {
        const target = findHighlight();
        if (!target) return;
        // Retried as heights are measured, since an unmeasured region moves the target's offset.
        measured();
        untrack(() => scrollToMessage(target));
    });

    createEffect(
        on(selectedSession, () => {
            clearReveal();
            heights.clear();
            estimates.clear();
            scroller.scrollTop = 0;
            batch(() => {
                setMeasured((value) => value + 1);
                setStick(true);
                setAwayFromBottom(false);
                setViewTop(0);
                setViewHeight(scroller.clientHeight);
            });
            // Snap once keyed content has laid out; the top reset keeps the interim frame from going blank.
            requestAnimationFrame(snapViewportToBottom);
        }),
    );

    // Untracked stick: growth follows the bottom, but entering the stick zone by hand never scrolls.
    createEffect(() => {
        const last = entries().at(-1);
        transcriptRevision(last);
        offsets();
        sessionError();
        thinking();
        if (untrack(stick)) {
            queueMicrotask(snapViewportToBottom);
            return;
        }
        queueMicrotask(() => {
            const distance = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
            setAwayFromBottom(shouldShowScrollToBottom(distance));
        });
    });

    // Only user gestures change stickiness; snaps, clamps and measurement also fire scroll events.
    let gestureAt = 0;
    let dragging = false;
    let scrollLatchReset: ReturnType<typeof setTimeout> | undefined;
    const gesture = () => (gestureAt = Date.now());
    const nativeWheel = gesture;
    const releaseDrag = () => (dragging = false);
    const forwardedWheel = (event: Event) => {
        const detail = (event as CustomEvent<ForwardedWheel>).detail;
        gesture();
        const delta = normalizedWheelDelta(detail.deltaY, detail.deltaMode, scroller.clientHeight);
        const max = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
        scroller.scrollTop = accumulatedWheelTarget(scroller.scrollTop, null, delta, max);
    };
    window.addEventListener("pointerup", releaseDrag);
    window.addEventListener(chatWheelEvent, forwardedWheel);
    onCleanup(() => {
        clearTimeout(scrollLatchReset);
        window.removeEventListener("pointerup", releaseDrag);
        window.removeEventListener(chatWheelEvent, forwardedWheel);
    });

    function onScroll() {
        const top = scroller.scrollTop;
        const previous = untrack(viewTop);
        setViewTop(top);
        setViewHeight(scroller.clientHeight);
        if (
            scroller.classList.contains("transcript-scroll-active") ||
            dragging ||
            Date.now() - gestureAt < gestureWindowMs
        ) {
            scroller.classList.add("transcript-scroll-active");
            clearTimeout(scrollLatchReset);
            scrollLatchReset = setTimeout(() => scroller.classList.remove("transcript-scroll-active"), gestureWindowMs);
        }
        if (dragging || Date.now() - gestureAt < gestureWindowMs) {
            const distance = scroller.scrollHeight - top - scroller.clientHeight;
            const nextStick = scrollGestureSticks(previous, top, distance);
            batch(() => {
                setStick(nextStick);
                setAwayFromBottom(!nextStick && shouldShowScrollToBottom(distance));
            });
        }
        maybeLoadOlder(top);
    }

    function maybeLoadOlder(top: number) {
        const id = selectedSession();
        // A stuck view's top positions are synthetic (resets, measurement), so only a user scroll pages history.
        if (!id || loadingOlder || untrack(stick) || top > loadOlderAt || !engine.state.cursors[id]) return;
        loadingOlder = true;
        const before = scroller.scrollHeight - scroller.scrollTop;
        void engine.actions.loadOlder(id).finally(() => {
            queueMicrotask(() => {
                scroller.scrollTop = scroller.scrollHeight - before;
                loadingOlder = false;
            });
        });
    }

    function scrollToBottom() {
        batch(() => {
            setStick(true);
            setAwayFromBottom(false);
        });
        scroller.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" });
    }

    return (
        <div class="relative min-h-0 flex-1">
            <div
                ref={scroller}
                class="transcript-scroll h-full overflow-x-hidden overflow-y-auto"
                onScroll={onScroll}
                onWheel={nativeWheel}
                onPointerDown={(event) => {
                    gesture();
                    dragging = event.target === scroller;
                }}
                onTouchStart={gesture}
            >
                <Show when={selectedSession()} keyed fallback={<EmptyState />}>
                    <div class="fade-in relative mx-auto box-content max-w-3xl px-4 pt-14 pb-6 select-text">
                        <Show
                            when={
                                timeline().length === 0 &&
                                (revertBackfill() ||
                                    (!engine.state.loaded[selectedSession()!] &&
                                        engine.state.connection === "online")) &&
                                !sessionError()
                            }
                        >
                            <div class="flex justify-center pt-8 text-sm select-none" role="status" aria-live="polite">
                                <TextShimmer text={t("common.loading")} />
                            </div>
                        </Show>
                        <div aria-hidden="true" style={{ height: `${offsets()[range().start]}px` }} />
                        <For each={slice()}>
                            {(entry) => (
                                <Row
                                    entry={entry}
                                    next={nextEntries().get(entry.info.id)}
                                    nextThinking={thinkingOnly(nextEntries().get(entry.info.id))}
                                    groups={assistantGroups().get(entry.info.id)}
                                    thinking={thinking()?.messageID === entry.info.id && !retry()}
                                    thinkingCompaction={thinking()?.compaction}
                                    thinkingHeading={thinking()?.heading}
                                    retry={thinking()?.messageID === entry.info.id ? retry() : undefined}
                                    terminalError={!nextEntries().get(entry.info.id) && !!sessionError()}
                                    found={findHighlight() === entry.info.id}
                                    measure={measureRow}
                                    copy={spawnedCopy()?.headerID === entry.info.id ? spawnedCopy() : undefined}
                                    copied={!!spawnedCopy()?.copiedIDs.has(entry.info.id)}
                                    instruction={spawnedCopy()?.ownID === entry.info.id}
                                    toggleCopy={toggleCopy}
                                />
                            )}
                        </For>
                        <div
                            aria-hidden="true"
                            style={{ height: `${(offsets().at(-1) ?? 0) - offsets()[range().end]}px` }}
                        />
                        <Show when={sessionError()}>
                            {(error) => (
                                <div role="alert">
                                    <div class="rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-sm break-words text-danger">
                                        {error()}
                                    </div>
                                </div>
                            )}
                        </Show>
                    </div>
                </Show>
            </div>
            <button
                type="button"
                title="Scroll to latest message"
                aria-label="Scroll to latest message"
                class="group absolute bottom-4 left-1/2 z-10 flex size-9 -translate-x-1/2 items-center justify-center rounded-full border border-edge-strong bg-overlay/95 text-ink-muted shadow-lg shadow-black/25 backdrop-blur transition-[opacity,translate,background-color,border-color,color,scale] duration-200 ease-out hover:border-accent/50 hover:bg-raised hover:text-ink active:scale-95"
                classList={{
                    "pointer-events-auto translate-y-0 opacity-100": awayFromBottom(),
                    "pointer-events-none translate-y-2 opacity-0": !awayFromBottom(),
                }}
                onClick={scrollToBottom}
            >
                <IconArrowDown class="size-4 transition-transform duration-200 group-hover:translate-y-0.5" />
            </button>
        </div>
    );
}
