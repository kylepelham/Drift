// Runs in an isolated world of the page, so the page's own scripts never see it. Gives each element the agent
// can act on a stable number for as long as the document lives.
(() => {
    if (globalThis.__drift) return;

    const MAX_TEXT = 100;
    const SECRET_TYPES = ["password", "hidden"];
    const SECRET_AUTOCOMPLETE = ["current-password", "new-password", "one-time-code", "cc-number", "cc-csc", "cc-exp"];
    const ROLES = {
        a: "link",
        button: "button",
        select: "combobox",
        textarea: "textbox",
        img: "img",
        nav: "navigation",
        main: "main",
        header: "banner",
        footer: "contentinfo",
        aside: "complementary",
        form: "form",
        dialog: "dialog",
        table: "table",
        ul: "list",
        ol: "list",
        li: "listitem",
        summary: "button",
        h1: "heading",
        h2: "heading",
        h3: "heading",
        h4: "heading",
        h5: "heading",
        h6: "heading",
        iframe: "frame",
    };
    const INPUT_ROLES = {
        button: "button",
        submit: "button",
        reset: "button",
        image: "button",
        checkbox: "checkbox",
        radio: "radio",
        range: "slider",
        search: "searchbox",
        file: "button",
    };
    const INTERACTIVE_ROLES = new Set([
        "button",
        "link",
        "textbox",
        "searchbox",
        "checkbox",
        "radio",
        "combobox",
        "slider",
        "switch",
        "tab",
        "menuitem",
        "option",
        "listbox",
        "spinbutton",
        "treeitem",
    ]);
    const SKIPPED = new Set(["script", "style", "noscript", "template", "svg", "head", "meta", "link"]);
    const TEXT_INPUTS = new Set(["text", "search", "email", "url", "tel", "password", "number"]);

    const refs = new Map();
    const numbers = new WeakMap();
    let next = 1;

    function refOf(element) {
        let number = numbers.get(element);
        if (!number) {
            number = next++;
            numbers.set(element, number);
            refs.set(number, new WeakRef(element));
        }
        return number;
    }

    function element(ref) {
        const found = refs.get(Number(ref));
        const live = found && found.deref();
        if (!live || !live.isConnected) throw new Error(`ref ${ref} is gone (the page changed); take a new snapshot`);
        return live;
    }

    function roleOf(node) {
        const explicit = node.getAttribute("role");
        if (explicit) return explicit.split(" ")[0];
        const tag = node.tagName.toLowerCase();
        if (tag === "input") return INPUT_ROLES[(node.getAttribute("type") || "text").toLowerCase()] || "textbox";
        if (tag === "a" && !node.hasAttribute("href")) return "generic";
        if (node.isContentEditable && node.getAttribute("contenteditable") !== null) return "textbox";
        return ROLES[tag] || "generic";
    }

    function secret(node) {
        const type = (node.getAttribute("type") || "").toLowerCase();
        const autocomplete = (node.getAttribute("autocomplete") || "").toLowerCase();
        return SECRET_TYPES.includes(type) || SECRET_AUTOCOMPLETE.some((value) => autocomplete.includes(value));
    }

    function clip(text) {
        const flat = (text || "").replace(/\s+/g, " ").trim();
        return flat.length > MAX_TEXT ? `${flat.slice(0, MAX_TEXT)}...` : flat;
    }

    function ownText(node) {
        let text = "";
        for (const child of node.childNodes) if (child.nodeType === Node.TEXT_NODE) text += child.textContent;
        return clip(text);
    }

    function labelled(node) {
        const by = node.getAttribute("aria-labelledby");
        if (by) {
            const text = by
                .split(/\s+/)
                .map((id) => node.ownerDocument.getElementById(id))
                .filter(Boolean)
                .map((label) => label.textContent)
                .join(" ");
            if (text.trim()) return clip(text);
        }
        if (node.labels && node.labels.length) return clip(node.labels[0].textContent);
        return "";
    }

    function nameOf(node, role) {
        const direct =
            node.getAttribute("aria-label") ||
            labelled(node) ||
            node.getAttribute("alt") ||
            node.getAttribute("title") ||
            node.getAttribute("placeholder");
        if (direct && direct.trim()) return clip(direct);
        if (["button", "link", "heading", "tab", "menuitem", "option", "listitem", "checkbox", "radio"].includes(role))
            return clip(node.innerText || node.textContent);
        if (node.tagName === "INPUT" && ["submit", "button", "reset"].includes(node.type)) return clip(node.value);
        return "";
    }

    function hidden(node) {
        if (node.getAttribute("aria-hidden") === "true" || node.hidden) return true;
        const style = getComputedStyle(node);
        if (style.display === "none" || style.visibility === "hidden") return true;
        const box = node.getBoundingClientRect();
        return box.width === 0 && box.height === 0 && style.overflow !== "visible";
    }

    function interactive(node, role) {
        if (INTERACTIVE_ROLES.has(role)) return true;
        if (node.hasAttribute("onclick") || node.getAttribute("contenteditable") === "true") return true;
        const tabIndex = node.getAttribute("tabindex");
        return tabIndex !== null && Number(tabIndex) >= 0;
    }

    function state(node, role) {
        const parts = [];
        if (role === "heading") parts.push(`level=${node.tagName.slice(1) || node.getAttribute("aria-level")}`);
        if (node.disabled || node.getAttribute("aria-disabled") === "true") parts.push("disabled");
        if (node.checked || node.getAttribute("aria-checked") === "true") parts.push("checked");
        if (node.getAttribute("aria-expanded")) parts.push(`expanded=${node.getAttribute("aria-expanded")}`);
        if (node.getAttribute("aria-selected") === "true") parts.push("selected");
        if (node.required) parts.push("required");
        return parts;
    }

    function value(node, role) {
        if (!["textbox", "searchbox", "combobox", "spinbutton", "slider"].includes(role)) return "";
        if (secret(node)) return node.value ? ' value="[hidden]"' : "";
        if (node.tagName === "SELECT") {
            const option = node.options[node.selectedIndex];
            return option ? ` value="${clip(option.textContent)}"` : "";
        }
        const current = node.value !== undefined ? node.value : node.textContent;
        return current ? ` value="${clip(current)}"` : "";
    }

    function line(node, role, depth, options) {
        const acts = interactive(node, role);
        const name = nameOf(node, role);
        const meaningful = acts || (role !== "generic" && role !== "listitem") || (name && role !== "generic");
        if (!meaningful) return null;
        if (options.interactive && !acts) return null;

        let text = `${"  ".repeat(depth)}- ${role}`;
        if (name) text += ` "${name.replace(/"/g, "'")}"`;
        const extra = state(node, role);
        if (acts) extra.push(`ref=${refOf(node)}`);
        if (extra.length) text += ` [${extra.join(", ")}]`;
        text += value(node, role);
        if (role === "link" && node.getAttribute("href")) text += ` -> ${clip(node.getAttribute("href"))}`;
        return text;
    }

    function walk(node, depth, out, options) {
        if (out.length >= options.limit) return;
        const tag = node.tagName.toLowerCase();
        if (SKIPPED.has(tag)) return;
        if (!options.all && hidden(node)) return;

        const role = roleOf(node);
        const written = line(node, role, depth, options);
        if (written) out.push(written);
        const inner = written ? depth + 1 : depth;

        // Text that sits directly in a container, as a paragraph does.
        if (!written && !options.interactive) {
            const text = ownText(node);
            if (text.length > 1) out.push(`${"  ".repeat(depth)}- text: ${text}`);
        }
        if (written && ["button", "link", "heading", "option", "tab", "menuitem"].includes(role)) return;

        const children = node.shadowRoot ? [...node.shadowRoot.children, ...node.children] : [...node.children];
        for (const child of children) walk(child, inner, out, options);

        if (tag === "iframe") {
            try {
                const body = node.contentDocument && node.contentDocument.body;
                if (body) walk(body, inner, out, options);
            } catch (_) {
                out.push(`${"  ".repeat(inner)}- text: (another site's frame; not readable)`);
            }
        }
    }

    // Viewport coordinates of a point inside an element, through any same-site frames it sits in.
    function centre(node) {
        node.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
        const box = node.getBoundingClientRect();
        if (box.width === 0 && box.height === 0) throw new Error("that element has no size, so it cannot be clicked");
        let x = box.left + box.width / 2;
        let y = box.top + box.height / 2;
        let frame = node.ownerDocument.defaultView.frameElement;
        while (frame) {
            const outer = frame.getBoundingClientRect();
            x += outer.left + frame.clientLeft;
            y += outer.top + frame.clientTop;
            frame = frame.ownerDocument.defaultView.frameElement;
        }
        return { x, y };
    }

    // The element under a viewport point, through same-site frames and open shadow roots, and the point in its document.
    function under(x, y) {
        let node = document.elementFromPoint(x, y);
        while (node) {
            const inner = node.shadowRoot && node.shadowRoot.elementFromPoint(x, y);
            if (inner && inner !== node) {
                node = inner;
                continue;
            }
            if (node.tagName !== "IFRAME") break;

            let frameDocument = null;
            try {
                frameDocument = node.contentDocument;
            } catch (_) {
                break;
            }
            if (!frameDocument) break;

            const box = node.getBoundingClientRect();
            x -= box.left + node.clientLeft;
            y -= box.top + node.clientTop;
            node = frameDocument.elementFromPoint(x, y);
        }
        return { node, x, y };
    }

    function editable(node) {
        if (node.isContentEditable || node.tagName === "TEXTAREA") return true;
        return node.tagName === "INPUT" && TEXT_INPUTS.has((node.getAttribute("type") || "text").toLowerCase());
    }

    // Whether the point is on selectable text, where the browser shows an I-beam for `cursor: auto`.
    function onText(node, x, y) {
        if (getComputedStyle(node).userSelect === "none") return false;
        const doc = node.ownerDocument;
        // Chrome has caretRangeFromPoint and Firefox caretPositionFromPoint.
        const caret = doc.caretRangeFromPoint
            ? doc.caretRangeFromPoint(x, y)
            : doc.caretPositionFromPoint && doc.caretPositionFromPoint(x, y);
        const text = caret && (caret.startContainer || caret.offsetNode);
        if (!text || text.nodeType !== Node.TEXT_NODE || !text.textContent.trim()) return false;

        const whole = doc.createRange();
        whole.selectNodeContents(text);
        return [...whole.getClientRects()].some(
            (box) => x >= box.left && x <= box.right && y >= box.top && y <= box.bottom,
        );
    }

    function choose(node, wanted) {
        const options = [...node.options];
        const match =
            options.find((option) => option.value === wanted) ||
            options.find((option) => option.textContent.trim() === wanted) ||
            options.find((option) => option.textContent.trim().toLowerCase().includes(String(wanted).toLowerCase()));
        if (!match) {
            const names = options.map((option) => option.textContent.trim()).join(", ");
            throw new Error(`no option matches "${wanted}"; the options are: ${names}`);
        }
        node.value = match.value;
        node.dispatchEvent(new Event("input", { bubbles: true }));
        node.dispatchEvent(new Event("change", { bubbles: true }));
        return `selected "${match.textContent.trim()}"`;
    }

    globalThis.__drift = {
        snapshot(options) {
            const out = [];
            const root = document.body || document.documentElement;
            walk(root, 0, out, {
                interactive: !!options.interactive,
                all: !!options.all,
                limit: options.limit || 4000,
            });
            return out.join("\n");
        },
        point(ref) {
            return centre(element(ref));
        },
        // Gets a field ready for typing: a select is set here, a checkbox says whether it needs a click.
        prepare(ref, text, checked) {
            const node = element(ref);
            const tag = node.tagName;
            if (tag === "SELECT") return { done: choose(node, text) };
            if (tag === "INPUT" && ["checkbox", "radio"].includes(node.type)) {
                const want = checked === undefined ? true : !!checked;
                return node.checked === want ? { done: "already so" } : { click: centre(node) };
            }
            node.scrollIntoView({ block: "center", behavior: "instant" });
            node.focus();
            if (typeof node.select === "function") node.select();
            else {
                const range = document.createRange();
                range.selectNodeContents(node);
                const selection = getSelection();
                selection.removeAllRanges();
                selection.addRange(range);
            }
            return { type: true };
        },
        scroll(ref) {
            element(ref).scrollIntoView({ block: "center", behavior: "instant" });
        },
        text() {
            return document.body ? document.body.innerText : "";
        },
        // The cursor keyword a mouse at this viewport point shows; an image cursor gives its keyword fallback.
        cursor(x, y) {
            const hit = under(x, y);
            if (!hit.node) return "default";

            const declared = getComputedStyle(hit.node).cursor.split(",").pop().trim();
            if (declared !== "auto") return /^[a-z-]+$/.test(declared) ? declared : "default";
            if (editable(hit.node) || onText(hit.node, hit.x, hit.y)) return "text";
            return "default";
        },
    };
})();
