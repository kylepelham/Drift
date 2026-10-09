import { toolInput, toolMetadata } from "../engine/parts";
import { classifyMarkdownLink } from "./markdown-links";

import type { EngineState, MessageEntry } from "../engine/store";
import type { Part, ToolPart } from "../engine/parts";

/** Build context only on a click, and stop at the cited message/part so old links stay stable. */
export function citationFileGroups(
    state: Pick<EngineState, "sessions" | "transcripts">,
    sessionID: string,
    messageID?: string,
    partID?: string,
    beforeTime?: number,
): string[][] {
    const directory = state.sessions[sessionID]?.directory;
    const entries = (state.transcripts[sessionID] ?? []).filter(
        (entry) =>
            entry.info.sessionId === sessionID && (beforeTime === undefined || entry.info.createdAt <= beforeTime),
    );
    const end = messageID ? entries.findIndex((entry) => entry.info.id === messageID) : entries.length - 1;
    if (!directory || end < 0) return [];
    if (partID && !entries[end].parts.some((part) => part.id === partID)) return [];
    let start = end;
    while (start > 0 && entries[start].info.role !== "user") start--;
    return [collect(entries.slice(start, end + 1)), collect(entries.slice(0, start))];

    function collect(messages: MessageEntry[]) {
        const files = new Set<string>();

        function add(value: unknown) {
            if (typeof value !== "string") return;

            // Tool paths are native strings, not percent-encoded hrefs.
            const href = value
                .split(/([/\\])/)
                .map((segment) => (segment === "/" || segment === "\\" ? segment : encodeURIComponent(segment)))
                .join("")
                .replace(/^([a-z])%3A([/\\])/i, "$1:$2");
            const link = classifyMarkdownLink(href, directory);
            if (link.kind === "file") files.add(link.path);
        }

        for (const entry of messages) {
            for (const part of entry.parts) {
                if (part.sessionId !== sessionID) continue;
                collectPart(part, files, add);
                if (entry.info.id === messageID && part.id === partID) break;
            }
        }

        return [...files];
    }

    function collectPart(part: Part, files: Set<string>, add: (value: unknown) => void) {
        if (part.type === "file" && /^file:\/\//i.test(part.url)) {
            const link = classifyMarkdownLink(part.url);
            if (link.kind === "file") files.add(link.path);
        }
        if (part.type !== "tool_call" || part.status !== "done") return;
        if (!completedBefore(part, beforeTime)) return;

        const input = toolInput(part);
        const metadata = toolMetadata(part);
        if (["read", "write", "edit", "multiedit"].includes(part.name)) add(input.filePath);
        if (part.name === "edit") add((metadata?.filediff as { file?: string } | undefined)?.file);
        if (part.name === "apply_patch") collectPatchFiles(metadata?.files, input.patchText, add);
    }
}

function completedBefore(part: ToolPart, beforeTime: number | undefined) {
    const finished = part.finishedAt ?? part.startedAt;

    return beforeTime === undefined || (finished !== undefined && finished !== null && finished <= beforeTime);
}

function collectPatchFiles(files: unknown, patch: unknown, add: (value: unknown) => void) {
    if (Array.isArray(files)) {
        for (const file of files) {
            if (!file || typeof file !== "object" || file.type === "delete") continue;
            add(file.movePath ?? file.filePath);
        }
    } else if (typeof patch === "string") {
        for (const match of patch.matchAll(/^\*\*\* (?:Add File|Update File|Move to): (.+)$/gm)) add(match[1].trim());
    }
}
