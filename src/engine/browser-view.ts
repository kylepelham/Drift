import type { ViewInput } from "./native/client";

/** One picture of a browser's active page, and the page's size in CSS pixels that input maps onto. */
export type BrowserFrame = { page: string; width: number; height: number; jpeg: Blob };

export type BrowserView = { send: (input: ViewInput) => void; close: () => void };

export type BrowserViewHandlers = {
    onFrame: (frame: BrowserFrame) => void;
    /** The CSS cursor the page shows under the pane's pointer. */
    onCursor: (cursor: string) => void;
    /** Text this pane's click or shortcut copied in the browser, whose clipboard is its own. */
    onCopied: (text: string) => void;
    /** The socket opened, first or after a drop: input sent before it was lost. */
    onOpen: () => void;
};

const retryMs = 1000;

type FrameHeader = { page: string; width: number; height: number };

/** Watches a session's browser over its view socket, reconnecting after a drop until closed. */
export function connectBrowserView(url: string, handlers: BrowserViewHandlers): BrowserView {
    let socket: WebSocket | undefined;
    let closed = false;
    let retry: ReturnType<typeof setTimeout> | undefined;

    const open = () => {
        socket = new WebSocket(url);
        socket.binaryType = "arraybuffer";
        socket.onopen = () => handlers.onOpen();
        socket.onmessage = (event) => {
            if (typeof event.data === "string") {
                const notice = decodeNotice(event.data);
                if (notice?.type === "cursor") handlers.onCursor(notice.cursor);
                if (notice?.type === "copied") handlers.onCopied(notice.text);
                return;
            }
            if (!(event.data instanceof ArrayBuffer)) return;

            const frame = decodeFrame(event.data);
            if (frame) handlers.onFrame(frame);
        };
        socket.onclose = () => {
            if (!closed) retry = setTimeout(open, retryMs);
        };
    };
    open();

    return {
        send: (input) => {
            if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(input));
        },
        close: () => {
            closed = true;
            clearTimeout(retry);
            socket?.close();
        },
    };
}

/** A frame message: a 4-byte little-endian header length, the JSON header, then the JPEG. */
export function decodeFrame(data: ArrayBuffer): BrowserFrame | undefined {
    if (data.byteLength < 4) return undefined;

    const length = new DataView(data).getUint32(0, true);
    if (4 + length > data.byteLength) return undefined;

    try {
        const header = JSON.parse(new TextDecoder().decode(new Uint8Array(data, 4, length))) as FrameHeader;
        const jpeg = new Blob([new Uint8Array(data, 4 + length)], { type: "image/jpeg" });

        return { page: header.page, width: header.width, height: header.height, jpeg };
    } catch {
        return undefined;
    }
}

type Notice = { type: "cursor"; cursor: string } | { type: "copied"; text: string };

/** A text message: a cursor's CSS keyword or copied text; anything else the socket says is ignored. */
export function decodeNotice(text: string): Notice | undefined {
    try {
        const message = JSON.parse(text) as { type?: string; cursor?: unknown; text?: unknown };
        if (message.type === "copied" && typeof message.text === "string")
            return { type: "copied", text: message.text };
        if (message.type !== "cursor" || typeof message.cursor !== "string") return undefined;

        return /^[a-z-]+$/.test(message.cursor) ? { type: "cursor", cursor: message.cursor } : undefined;
    } catch {
        return undefined;
    }
}
