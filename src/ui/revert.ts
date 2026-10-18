import { compareMessages, messageText, nextUserMessage, previousUserMessage, type MessageEntry } from "../engine/store";
import { clearComposerDraft, composerScope, draftFromMessage, setComposerDraft } from "../state/composer";
import { t } from "../state/i18n";

export type RevertHost = {
    state: { transcripts: Record<string, MessageEntry[]> };
    actions: {
        revert: (id: string, messageID: string) => Promise<boolean>;
        unrevert: (id: string) => Promise<boolean>;
    };
};

export function revertDockEntries(entries: MessageEntry[], marker?: string) {
    if (!marker) return [];
    const boundary = entries.find((entry) => entry.info.id === marker);
    return entries
        .filter((entry) => {
            if (entry.info.role !== "user") return false;
            if (boundary) return compareMessages(entry, boundary) >= 0;
            return entry.info.id >= marker;
        })
        .sort((a, b) => compareMessages(b, a));
}

export function revertPreview(entry?: MessageEntry) {
    if (!entry) return "";
    if (isCompaction(entry)) return t("drift.context.compacted");
    return messageText(entry).replace(/\s+/g, " ").trim();
}

/** Restores an undone user message: the marker moves to the next one, or the session unreverts. */
export async function restoreReverted(engine: RevertHost, sessionID: string, messageID: string) {
    const next = nextUserMessage(engine.state.transcripts[sessionID] ?? [], messageID);
    if (next) {
        const restored = draftFromMessage(next);
        const success = await engine.actions.revert(sessionID, next.info.id);
        if (success) setComposerDraft(composerScope(sessionID), restored);
        return success;
    }

    const success = await engine.actions.unrevert(sessionID);
    if (success) clearComposerDraft(composerScope(sessionID));

    return success;
}

function isCompaction(entry: MessageEntry) {
    return entry.parts.some((part) => part.type === "compaction");
}

/** Undoes a failed compaction and summarises again; compacting commits the undo, so the failed attempt goes. */
export async function retryCompaction(
    engine: RevertHost & { actions: { summarize: (id: string) => Promise<void> } },
    sessionID: string,
    summaryID: string,
) {
    const boundary = previousUserMessage(engine.state.transcripts[sessionID] ?? [], summaryID);
    if (!boundary || !isCompaction(boundary)) return false;
    if (!(await engine.actions.revert(sessionID, boundary.info.id))) return false;

    await engine.actions.summarize(sessionID);
    return true;
}
