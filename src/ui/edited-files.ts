import { toolInput, toolMetadata, type ToolPart } from "../engine/parts";
import { patchFiles, toolFilename } from "./tool-labels";
import { parseDiff } from "./diff-panel";

import type { MessageEntry } from "../engine/store";

export type EditedFile = {
    path: string;
    name: string;
    additions: number;
    deletions: number;
    /** The subagent that changed it; none for the turn's own edits. */
    by?: string;
};
/** A subagent a turn launched, with its transcript if loaded. */
export type TurnWorker = { name: string; entries: MessageEntry[] };

const editTools = new Set(["edit", "write", "apply_patch"]);

/** Parts the engine writes into a user message to carry on a turn: a background job's result or exit, a drive nudge. */
const engineNotices = new Set(["task_result", "process_exit", "nudge"]);

/** A message that goes on with the turn before it: a reply, or a user message holding only engine notices. */
export function continuesTurn(entry: MessageEntry | undefined) {
    if (!entry) return false;
    if (entry.info.role === "assistant") return true;

    return entry.parts.length > 0 && entry.parts.every((part) => engineNotices.has(part.type));
}

/** The prompt that started the turn ending at `id`, and the replies between them. */
export function turnOf(entries: MessageEntry[], id: string) {
    const end = entries.findIndex((entry) => entry.info.id === id);
    if (end < 0) return { prompt: undefined, replies: [] };

    let start = end;
    while (start > 0 && continuesTurn(entries[start - 1])) start--;

    return { prompt: entries[start - 1], replies: entries.slice(start, end + 1) };
}

/** A turn whose replies reach the first loaded message while older pages remain: its prompt has not been loaded yet. */
export function turnStartsEarlier(turn: ReturnType<typeof turnOf>, cursor?: string | null) {
    return !turn.prompt && turn.replies.length > 0 && !!cursor;
}

function counted(patch: string) {
    let additions = 0;
    let deletions = 0;
    for (const row of parseDiff(patch)) {
        if (row.kind === "add") additions++;
        if (row.kind === "del") deletions++;
    }

    return { additions, deletions };
}

/** Each change one call made: a patch per file, with its counts. */
function callChanges(part: ToolPart) {
    const patched = patchFiles(part);
    if (patched.length)
        return patched.map((file) => ({
            path: file.filePath,
            label: file.relativePath ?? file.filePath,
            additions: file.additions,
            deletions: file.deletions,
        }));

    const path = toolInput(part).filePath;
    const diff = toolMetadata(part).diff;
    if (typeof path !== "string" || typeof diff !== "string" || !diff.trim()) return [];

    return [{ path, label: path, ...counted(diff) }];
}

function editCalls(entries: MessageEntry[]) {
    return entries.flatMap((entry) =>
        entry.parts.filter(
            (part): part is ToolPart => part.type === "tool_call" && editTools.has(part.name) && part.status === "done",
        ),
    );
}

/** The files a turn and its subagents changed, one row each, in the order they were first changed. */
export function editedFiles(replies: MessageEntry[], workers: TurnWorker[]): EditedFile[] {
    const files = new Map<string, EditedFile>();
    const add = (calls: ToolPart[], by?: string) => {
        for (const change of calls.flatMap(callChanges)) {
            const key = `${by ?? ""}\u0000${change.path.replaceAll("\\", "/").toLowerCase()}`;
            const file = files.get(key) ?? {
                path: change.path,
                name: toolFilename(change.label) ?? change.label,
                additions: 0,
                deletions: 0,
                by,
            };
            file.additions += change.additions;
            file.deletions += change.deletions;
            files.set(key, file);
        }
    };

    add(editCalls(replies));
    for (const worker of workers) add(editCalls(worker.entries), worker.name);

    return [...files.values()];
}

/** The `task` calls a turn made, whose subagents' edits belong to it. */
export function turnTasks(replies: MessageEntry[]) {
    return replies.flatMap((entry) =>
        entry.parts.filter((part): part is ToolPart => part.type === "tool_call" && part.name === "task"),
    );
}
