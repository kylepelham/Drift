// What the pane does in a page that input alone cannot: paste and drop with their data, a drag Firefox's
// WebDriver input cannot carry, and a select's list, which the browser draws outside the page's picture. Called as `(this script)(action, args)`; runs in the page's own world, so the page's
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

    // The deepest element at the point, through open shadow roots and same-origin frames, with the frame's offset.
    const deepAt = (x, y) => {
        let node = document.elementFromPoint(x, y);
        let [dx, dy] = [0, 0];
        for (let depth = 0; node && depth < 32; depth++) {
            const inner = node.shadowRoot?.elementFromPoint(x - dx, y - dy);
            if (inner && inner !== node) {
                node = inner;
                continue;
            }
            const inside = node.tagName === "IFRAME" ? node.contentDocument : null;
            if (!inside) break;
            const box = node.getBoundingClientRect();
            dx += box.left + node.clientLeft;
            dy += box.top + node.clientTop;
            node = inside.elementFromPoint(x - dx, y - dy);
        }
        return { node, dx, dy };
    };

    // The press that opens or closes the list must not open the browser's own, which the pane cannot show.
    const holdBack = (select) => {
        const view = select.ownerDocument.defaultView;
        const block = (event) => {
            view.removeEventListener("mousedown", block, true);
            if (!event.composedPath().includes(select)) return;
            event.preventDefault();
            select.focus();
        };
        view.addEventListener("mousedown", block, true);
        setTimeout(() => view.removeEventListener("mousedown", block, true), 1000);
    };

    const rows = (select) => {
        const found = [];
        for (const child of select.children) {
            if (child.tagName === "OPTGROUP") {
                found.push({ label: child.label, group: true });
                for (const option of child.children)
                    if (option.tagName === "OPTION" && !option.hidden)
                        found.push({ option, nested: true, disabled: child.disabled || option.disabled });
            } else if (child.tagName === "OPTION" && !child.hidden) {
                found.push({ option: child, disabled: child.disabled });
            }
        }
        return found;
    };

    const ROW = 24;
    const PICKER_CSS = `:host{all:initial}
        .list{position:fixed;box-sizing:border-box;overflow-y:auto;background:Canvas;color:CanvasText;
            border:1px solid #8f8f9d;border-radius:3px;box-shadow:0 2px 8px rgba(0,0,0,.25);padding:2px 0}
        .row{box-sizing:border-box;height:${ROW}px;line-height:${ROW - 4}px;padding:2px 8px;white-space:pre;cursor:default}
        .nested{padding-left:20px}.group{font-weight:bold}.disabled{opacity:.5}
        .active{background:Highlight;color:HighlightText}`;

    const pick = (select, dx, dy) => {
        const doc = document;
        const view = select.ownerDocument.defaultView;
        const host = doc.createElement("div");
        const root = host.attachShadow({ mode: "closed" });
        const style = doc.createElement("style");
        style.textContent = PICKER_CSS;
        const list = doc.createElement("div");
        list.className = "list";
        list.setAttribute("role", "listbox");
        const look = getComputedStyle(select);
        Object.assign(list.style, {
            fontFamily: look.fontFamily,
            fontSize: look.fontSize,
            colorScheme: look.colorScheme,
        });
        const items = rows(select).map((row) => {
            const item = doc.createElement("div");
            item.className = ["row", row.group && "group", row.nested && "nested", row.disabled && "disabled"]
                .filter(Boolean)
                .join(" ");
            item.textContent = row.group ? row.label : row.option.label || row.option.text;
            list.append(item);
            return { ...row, item };
        });
        const choosable = items.filter((row) => row.option && !row.disabled);
        let active = choosable.find((row) => row.option.selected);
        const activate = (row) => {
            active?.item.classList.remove("active");
            active = row;
            row?.item.classList.add("active");
            row?.item.scrollIntoView({ block: "nearest" });
        };

        const box = select.getBoundingClientRect();
        const [top, bottom] = [box.top + dy, box.bottom + dy];
        const tall = Math.min(items.length, 20) * ROW + 6;
        const below = innerHeight - bottom;
        const down = below >= tall || below >= top;
        const height = Math.min(tall, Math.max(below, top) - 4);
        Object.assign(list.style, {
            left: `${Math.max(0, box.left + dx)}px`,
            top: `${down ? bottom : top - height}px`,
            minWidth: `${box.width}px`,
            maxHeight: `${height}px`,
        });
        root.append(style, list);
        Object.assign(host.style, { position: "fixed", left: 0, top: 0, zIndex: 2147483647 });
        doc.documentElement.append(host);
        activate(active);

        const choose = (row) => {
            close();
            select.focus();
            if (row.option.selected) return;
            row.option.selected = true;
            select.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
            select.dispatchEvent(new Event("change", { bubbles: true }));
        };
        // The press that opened the list reaches the select after it; a later press there is `pick`'s to close it.
        const outside = (event) => {
            const path = event.composedPath();
            if (!path.includes(host) && !path.includes(select)) close();
        };
        const keys = (event) => {
            const at = choosable.indexOf(active);
            const step = { ArrowDown: 1, ArrowUp: -1, PageDown: 10, PageUp: -10 }[event.key];
            if (step) activate(choosable[Math.min(Math.max(at + step, 0), choosable.length - 1)]);
            else if (event.key === "Home" || event.key === "End")
                activate(event.key === "Home" ? choosable[0] : choosable.at(-1));
            else if ((event.key === "Enter" || event.key === " ") && active) choose(active);
            else if (event.key === "Escape" || event.key === "Tab") close();
            else return;
            if (event.key === "Tab") return;
            event.preventDefault();
            event.stopImmediatePropagation();
        };
        const moved = (event) => {
            if (!event.composedPath().includes(list)) close();
        };
        const listeners = [
            [window, "mousedown", outside],
            [view, "mousedown", outside],
            [view, "keydown", keys],
            [window, "scroll", moved],
            [window, "resize", close],
        ];
        function close() {
            for (const [target, type, listener] of listeners) target.removeEventListener(type, listener, true);
            host.remove();
            if (window.__driftPicker?.select === select) window.__driftPicker = null;
        }
        for (const [target, type, listener] of listeners) target.addEventListener(type, listener, true);
        list.addEventListener("mousedown", (event) => event.preventDefault());
        list.addEventListener("mousemove", (event) => {
            const row = choosable.find((row) => row.item === event.target);
            if (row && row !== active) activate(row);
        });
        list.addEventListener("click", (event) => {
            const row = choosable.find((row) => row.item === event.target);
            if (row) choose(row);
        });
        window.__driftPicker = { select, host, close };
    };

    if (action === "pick") {
        const { node, dx, dy } = deepAt(args.x, args.y);
        const select = node?.closest?.("select");
        const open = window.__driftPicker;
        if (open && node === open.host) return "inside";
        open?.close();
        if (!select || select.disabled || select.multiple || select.size > 1) return false;
        holdBack(select);
        if (open?.select === select) return "closed";
        pick(select, dx, dy);
        return "opened";
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
