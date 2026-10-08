import { clearComposerDraft, composerScope, draftFromMessage, setComposerDraft } from "../state/composer";
import { compareMessages, messageText, nextUserMessage, type MessageEntry } from "../engine/store";

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
