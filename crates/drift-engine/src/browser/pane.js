// What the pane does in a page that input alone cannot: paste and drop with their data, and a drag Firefox's
// WebDriver input cannot carry. Called as `(this script)(action, args)`; runs in the page's own world, so the page's
// listeners see the events and the data they carry.
(action, args) => {
    const state = (window.__driftPane ??= {});
    const deepActive = () => {
        let element = document.activeElement;
        while (element?.shadowRoot?.activeElement) element = element.shadowRoot.activeElement;
        return element;
    };
    const files = (list) =>
        (list ?? []).map((file) => {
            const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
            return new File([bytes], file.name, { type: file.mime || "application/octet-stream" });
        });
    const transfer = (text, list) => {
        const data = new DataTransfer();
        if (text) data.setData("text/plain", text);
        for (const file of files(list)) data.items.add(file);
        return data;
    };
    const editable = (element) =>
        !!element &&
        (element.isContentEditable ||
            (element instanceof HTMLTextAreaElement && !element.readOnly) ||
            (element instanceof HTMLInputElement &&
                !element.readOnly &&
                !["checkbox", "radio", "button", "submit", "file", "range", "color"].includes(element.type)));
    const drag = (type, target, data) => {
        const event = new DragEvent(type, {
            bubbles: true,
            cancelable: true,
            composed: true,
            clientX: args.x,
            clientY: args.y,
            dataTransfer: data,
        });
        target.dispatchEvent(event);
        return event;
    };
    const at = () => document.elementFromPoint(args.x, args.y) ?? document.body;

    if (action === "paste") {
        const target = deepActive();
        // A frame from another origin has focus; the engine types into it instead.
        if (target instanceof HTMLIFrameElement && !target.contentDocument) return "frame";
        const clipboardData = transfer(args.text, args.files);
        const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true, composed: true, clipboardData });
        // Firefox drops the data a script's paste event is made with, so the event carries it as its own.
        if (!event.clipboardData?.types.length) Object.defineProperty(event, "clipboardData", { value: clipboardData });
        (target ?? document.body).dispatchEvent(event);
        if (event.defaultPrevented || !args.text || !editable(target)) return "pasted";
        (target.ownerDocument ?? document).execCommand("insertText", false, args.text);
        return "pasted";
    }

    if (action === "drop") {
        const target = at();
        const data = transfer(args.text, args.files);
        drag("dragenter", target, data);
        const over = drag("dragover", target, data);
        if (target instanceof HTMLInputElement && target.type === "file" && data.files.length) {
            target.files = data.files;
            target.dispatchEvent(new Event("input", { bubbles: true }));
            target.dispatchEvent(new Event("change", { bubbles: true }));
            return "chosen";
        }
        if (over.defaultPrevented) drag("drop", target, data);
        else if (args.text && editable(target)) {
            target.focus();
            (target.ownerDocument ?? document).execCommand("insertText", false, args.text);
        }
        return "dropped";
    }

    if (action === "dragStart") {
        const source = document.elementFromPoint(args.fromX, args.fromY)?.closest('[draggable="true"]');
        if (!source) return false;
        const data = new DataTransfer();
        const start = drag("dragstart", source, data);
        if (start.defaultPrevented) return false;
        // Firefox would start its own drag on the next move, which WebDriver input never ends.
        source.draggable = false;
        Object.assign(state, { source, data, over: null, accepted: false });
        return true;
    }

    if (action === "dragMove" || action === "dragEnd") {
        const { source, data } = state;
        if (!source) return false;
        const target = at();
        if (target !== state.over) {
            if (state.over) drag("dragleave", state.over, data);
            drag("dragenter", target, data);
            state.over = target;
        }
        state.accepted = drag("dragover", target, data).defaultPrevented;
        if (action === "dragMove") return true;

        if (state.accepted) drag("drop", target, data);
        drag("dragend", source, data);
        // The press that started the drag is released where it began, which must not click the source.
        const swallow = (event) => {
            event.stopPropagation();
            event.preventDefault();
        };
        window.addEventListener("click", swallow, true);
        setTimeout(() => {
            window.removeEventListener("click", swallow, true);
            source.draggable = true;
        }, 300);
        window.__driftPane = {};
        return true;
    }

    return false;
};
