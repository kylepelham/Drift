import { clarificationAnswer } from "./clarification-answer";
import { messageText } from "../engine/store";
import { largeUserText } from "./message";

import type { MessageEntry } from "../engine/store";
import type { Part } from "../engine/parts";

export const estimatedRow = 96;
const overscan = 800;
// The "at the bottom" distance; scrollGestureSticks and shouldShowScrollToBottom must share it.
const stickyThresholdPx = 80;

export function estimatedTimelineRow(
    entry: MessageEntry,
    fontSize = 13,
    parts: Part[] = entry.parts,
    thinkingOnly = false,
    collapsedSummary = false,
) {
    if (thinkingOnly) return 32;
    if (collapsedSummary) return 44;
    if (clarificationAnswer(parts === entry.parts ? entry : { ...entry, parts })) return 40;
    const text = messageText(parts === entry.parts ? entry : { ...entry, parts });
    const generated = parts.some((part) => part.type === "nudge");
    if (entry.info.role === "user" && !generated && largeUserText(text))
        return Math.max(estimatedRow, Math.ceil(text.split("\n").length * fontSize * 1.6 + 62));
    const width = entry.info.role === "user" ? 72 : 88;
    const textHeight = estimateTextLines(text, width) * 14 * 1.6;
    const toolHeight = parts.filter((part) => part.type === "tool_call").length * 56;
    return Math.max(estimatedRow, Math.ceil(textHeight + toolHeight + (text ? 48 : 0)));
}

export function estimateTextLines(text: string, width: number) {
    let fenced = false;
    return text.split("\n").reduce((total, line) => {
        if (/^\s*```/.test(line)) {
            fenced = !fenced;
            return total + 1;
        }
        if (fenced) return total + 1;
        if (!line) return total;
        return total + Math.max(1, Math.ceil(line.length / width));
    }, 0);
}

export function resizeCompensation(previous: number, next: number, rowBottom: number, viewportTop: number) {
    return rowBottom < viewportTop ? next - previous : 0;
}

export function virtualRange(offsets: number[], viewTop: number, viewHeight: number) {
    const currentTop = Math.min(viewTop, Math.max(0, (offsets.at(-1) ?? 0) - viewHeight));
    const top = currentTop - overscan;
    const bottom = currentTop + viewHeight + overscan;
    let start = 0;
    while (start < offsets.length - 1 && offsets[start + 1] < top) start++;
    let end = start;
    while (end < offsets.length - 1 && offsets[end] < bottom) end++;
    return { start, end };
}

export function snapVirtualViewport(
    scroller: { scrollTop: number; readonly scrollHeight: number; readonly clientHeight: number },
    publish: (top: number, height: number) => void,
) {
    scroller.scrollTop = scroller.scrollHeight;
    // Publish the clamped value now; a no-op assignment need not fire a scroll event.
    publish(scroller.scrollTop, scroller.clientHeight);
}

export function scrollGestureSticks(previousTop: number, nextTop: number, distanceFromBottom: number) {
    if (nextTop < previousTop) return false;
    return distanceFromBottom < stickyThresholdPx;
}

export function shouldShowScrollToBottom(distanceFromBottom: number) {
    return distanceFromBottom >= stickyThresholdPx;
}

export function transcriptRevision(entry?: { parts: Part[] }) {
    if (!entry) return "0";
    let revision = `${entry.parts.length}`;
    for (const part of entry.parts) revision += partRevision(part);

    return revision;
}

function partRevision(part: Part) {
    if (part.type === "text" || part.type === "reasoning") return `|${part.type}:${part.text?.length ?? 0}`;
    if (part.type !== "tool_call") return "";

    return toolRevision(part);
}

function toolRevision(part: Extract<Part, { type: "tool_call" }>) {
    let revision = `|tool:${part.status}`;
    if (typeof part.output === "string") revision += `:o${part.output.length}`;

    const metadata = part.metadata;
    if (typeof metadata?.output === "string") revision += `:m${metadata.output.length}`;
    if (typeof metadata?.diff === "string") revision += `:d${metadata.diff.length}`;

    return revision;
}
