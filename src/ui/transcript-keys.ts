import { clearMessageCursor, messageCursor, setMessageCursor, stepMessageCursor } from "../state/message-cursor";
import { appKeyFree, eventCombo, onKeybind, takesKeys, type KeybindAction } from "../state/keybinds";
import { messageText, type MessageEntry } from "../engine/store";
import { onCleanup } from "solid-js";

type TranscriptKeys = {
    rows: () => MessageEntry[];
    scroller: () => HTMLElement;
    dock: () => HTMLElement;
    headerOffset: number;
    scrollToMessage: (id: string) => void;
    scrollToBottom: () => void;
    /** Marks the next scroll as the reader's own, so it can unstick the view from the bottom. */
    gesture: () => void;
    copied: () => void;
};

/** The transcript's keys: the message shortcuts, Page Up and Down from the composer, and Escape and Ctrl+C for the ringed message. */
export function bindTranscriptKeys(options: TranscriptKeys) {
    const isPrompt = (id: string) => options.rows().find((entry) => entry.info.id === id)?.info.role === "user";

    function move(step: 1 | -1, prompts: boolean) {
        const order = options.rows().map((entry) => entry.info.id);
        const next = stepMessageCursor(
            order,
            messageCursor(),
            step,
            rowInView(step < 0),
            prompts ? isPrompt : undefined,
        );
        if (!next) return;
        setMessageCursor(next);
        options.scrollToMessage(next);
    }

    function jump(last: boolean) {
        const id = (last ? options.rows().at(-1) : options.rows()[0])?.info.id;
        if (!id) return;
        setMessageCursor(id);
        if (last) options.scrollToBottom();
        else options.scrollToMessage(id);
    }

    /** The message nearest the reader: the lowest one showing when going up, the highest going down. */
    function rowInView(up: boolean) {
        const frame = options.scroller().getBoundingClientRect();
        const top = frame.top + options.headerOffset;
        const bottom = frame.bottom - options.dock().offsetHeight;
        const showing = [...options.scroller().querySelectorAll<HTMLElement>("[data-mid]")].filter((row) => {
            const rect = row.getBoundingClientRect();
            return rect.bottom > top && rect.top < bottom;
        });
        return (up ? showing.at(-1) : showing[0])?.dataset.mid;
    }

    function page(event: KeyboardEvent) {
        const target = event.target;
        const fits =
            target instanceof HTMLTextAreaElement &&
            options.dock().contains(target) &&
            target.scrollHeight <= target.clientHeight + 1;
        if (target !== document.body && !fits) return false;
        const scroller = options.scroller();
        options.gesture();
        scroller.scrollTop +=
            (event.key === "PageUp" ? -1 : 1) * (scroller.clientHeight - options.dock().offsetHeight) * 0.9;
        return true;
    }

    function copy(event: KeyboardEvent) {
        const id = messageCursor();
        const entry = id ? options.rows().find((row) => row.info.id === id) : undefined;
        if (!entry || selectsText(event.target)) return false;
        void navigator.clipboard.writeText(messageText(entry)).then(options.copied);
        return true;
    }

    function release() {
        if (!messageCursor()) return false;
        clearMessageCursor();
        return true;
    }

    function onKeyDown(event: KeyboardEvent) {
        if (!appKeyFree(event)) return;
        const combo = eventCombo(event);
        const handled =
            (combo === "escape" && release()) ||
            ((combo === "pageup" || combo === "pagedown") && page(event)) ||
            (combo === "ctrl+c" && copy(event));
        if (handled) event.preventDefault();
    }

    const bindings: [KeybindAction, () => void][] = [
        ["previousMessage", () => move(-1, false)],
        ["nextMessage", () => move(1, false)],
        ["previousPrompt", () => move(-1, true)],
        ["nextPrompt", () => move(1, true)],
        ["firstMessage", () => jump(false)],
        ["lastMessage", () => jump(true)],
    ];
    for (const [action, handler] of bindings) onCleanup(onKeybind(action, handler));
    window.addEventListener("keydown", onKeyDown);
    onCleanup(() => window.removeEventListener("keydown", onKeyDown));
}

/** Whether Ctrl+C has its own text to copy: a selection on the page or in a field. */
function selectsText(target: EventTarget | null) {
    if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
        return target.selectionStart !== target.selectionEnd;
    if (target instanceof Element && takesKeys(target)) return true;
    return !(window.getSelection()?.isCollapsed ?? true);
}
