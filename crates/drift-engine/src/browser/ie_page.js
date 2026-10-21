// Installed into an IE mode page by the browser tool. Written for IE's oldest document modes: no let/const,
// arrow functions, JSON, forEach, trim or getComputedStyle. Frames are walked from the top window, so refs
// reach elements in any same-site frame.
(function (w) {
    if (w.__drift) return;

    var refs = [];
    var ROLES = {
        A: "link",
        BUTTON: "button",
        SELECT: "combobox",
        TEXTAREA: "textbox",
        IMG: "img",
        FORM: "form",
        TABLE: "table",
        UL: "list",
        OL: "list",
        LI: "listitem",
        H1: "heading",
        H2: "heading",
        H3: "heading",
        H4: "heading",
        H5: "heading",
        H6: "heading",
        IFRAME: "frame",
        FRAME: "frame",
        LABEL: "label"
    };
    var INPUTS = {
        button: "button",
        submit: "button",
        reset: "button",
        image: "button",
        checkbox: "checkbox",
        radio: "radio",
        file: "button",
        hidden: "",
        password: "textbox",
        text: "textbox"
    };
    var SKIP = { SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, HEAD: 1, META: 1, LINK: 1, TITLE: 1, OBJECT: 1 };

    function trim(text) {
        return String(text == null ? "" : text)
            .replace(/\s+/g, " ")
            .replace(/^ | $/g, "");
    }

    function clip(text) {
        var flat = trim(text);
        return flat.length > 100 ? flat.substring(0, 100) + "..." : flat;
    }

    function quote(text) {
        return '"' + String(text).replace(/"/g, "'") + '"';
    }

    function refOf(el) {
        for (var i = 0; i < refs.length; i++) if (refs[i] === el) return i + 1;
        refs.push(el);
        return refs.length;
    }

    function element(ref) {
        var el = refs[Number(ref) - 1];
        if (!el || !el.ownerDocument || !el.parentNode)
            throw new Error("ref " + ref + " is gone (the page changed); take a new snapshot");
        return el;
    }

    function style(el) {
        if (el.currentStyle) return el.currentStyle;
        var view = el.ownerDocument.defaultView;
        return view && view.getComputedStyle ? view.getComputedStyle(el, null) : {};
    }

    function hidden(el) {
        if (el.type === "hidden") return true;
        var s = style(el);
        return s.display === "none" || s.visibility === "hidden";
    }

    function role(el) {
        var explicit = el.getAttribute && el.getAttribute("role");
        if (explicit) return String(explicit).split(" ")[0];
        var tag = el.tagName;
        if (tag === "INPUT") {
            var type = String(el.type || "text").toLowerCase();
            return INPUTS.hasOwnProperty(type) ? INPUTS[type] : "textbox";
        }
        if (tag === "A" && !el.getAttribute("href") && !el.onclick) return "generic";
        return ROLES[tag] || "generic";
    }

    function acts(el, r) {
        if (r === "button" || r === "link" || r === "textbox" || r === "checkbox" || r === "radio") return true;
        if (r === "combobox" || r === "menuitem" || r === "tab" || r === "option") return true;
        return !!(el.onclick || (el.getAttribute && el.getAttribute("onclick")));
    }

    function labelFor(el) {
        if (!el.id) return "";
        var labels = el.ownerDocument.getElementsByTagName("LABEL");
        for (var i = 0; i < labels.length; i++) if (labels[i].htmlFor === el.id) return labels[i].innerText;
        return "";
    }

    function secret(el) {
        var type = String(el.type || "").toLowerCase();
        var auto = String((el.getAttribute && el.getAttribute("autocomplete")) || "").toLowerCase();
        return type === "password" || auto.indexOf("password") >= 0 || auto.indexOf("cc-") === 0;
    }

    function name(el, r) {
        var direct = (el.getAttribute && (el.getAttribute("aria-label") || el.getAttribute("title"))) || el.alt;
        if (direct) return clip(direct);
        var label = labelFor(el);
        if (label) return clip(label);
        if (el.tagName === "INPUT" && (el.type === "submit" || el.type === "button" || el.type === "reset"))
            return clip(el.value);
        if (r === "button" || r === "link" || r === "heading" || r === "listitem" || r === "option" || r === "menuitem")
            return clip(el.innerText);
        return clip(el.name || el.placeholder || "");
    }

    function value(el, r) {
        if (r === "combobox" && el.options) {
            var option = el.options[el.selectedIndex];
            return option ? " value=" + quote(clip(option.text)) : "";
        }
        if (r !== "textbox") return "";
        if (secret(el)) return el.value ? ' value="[hidden]"' : "";
        return el.value ? " value=" + quote(clip(el.value)) : "";
    }

    function ownText(el) {
        var text = "";
        for (var i = 0; i < el.childNodes.length; i++)
            if (el.childNodes[i].nodeType === 3) text += el.childNodes[i].nodeValue;
        return clip(text);
    }

    function pad(depth) {
        var text = "";
        for (var i = 0; i < depth; i++) text += "  ";
        return text;
    }

    function line(el, r, depth, interactive) {
        var active = acts(el, r);
        var n = name(el, r);
        if (!active && (r === "generic" || r === "listitem" || r === "label") && !n) return null;
        if (!active && r === "generic") return null;
        if (interactive && !active) return null;

        var extra = [];
        if (el.disabled) extra.push("disabled");
        if (el.checked) extra.push("checked");
        if (active) extra.push("ref=" + refOf(el));
        var text = pad(depth) + "- " + r + (n ? " " + quote(n) : "");
        if (extra.length) text += " [" + extra.join(", ") + "]";
        return text + value(el, r);
    }

    function frameDocument(el) {
        try {
            var doc = el.contentWindow.document;
            return doc && doc.body ? doc : null;
        } catch (e) {
            return null;
        }
    }

    function walk(el, depth, out, interactive) {
        if (out.length >= 4000 || el.nodeType !== 1 || SKIP[el.tagName] || hidden(el)) return;

        var r = role(el);
        var written = line(el, r, depth, interactive);
        if (written) out.push(written);
        var inner = written ? depth + 1 : depth;
        if (!written && !interactive) {
            var text = ownText(el);
            if (text.length > 1) out.push(pad(depth) + "- text: " + text);
        }
        if (written && (r === "button" || r === "link" || r === "heading" || r === "option")) return;

        if (el.tagName === "FRAME" || el.tagName === "IFRAME") {
            var doc = frameDocument(el);
            var label = el.name || el.id || "";
            out.push(pad(inner) + "- frame " + quote(label));
            if (doc) walk(doc.body, inner + 1, out, interactive);
            else out.push(pad(inner + 1) + "- text: (another site's frame; not readable)");
            return;
        }
        if (el.tagName === "FRAMESET") {
            var frames = el.childNodes;
            for (var f = 0; f < frames.length; f++) walk(frames[f], depth, out, interactive);
            return;
        }
        for (var i = 0; i < el.childNodes.length; i++) walk(el.childNodes[i], inner, out, interactive);
    }

    function root(doc) {
        var sets = doc.getElementsByTagName("FRAMESET");
        return sets.length ? sets[0] : doc.body || doc.documentElement;
    }

    function fire(el, type, init) {
        var doc = el.ownerDocument;
        if (doc.createEventObject && el.fireEvent) {
            var legacy = doc.createEventObject();
            for (var key in init || {}) legacy[key] = init[key];
            return el.fireEvent("on" + type, legacy);
        }
        var event = doc.createEvent(
            /key/.test(type) ? "Events" : /mouse|click/.test(type) ? "MouseEvents" : "HTMLEvents"
        );
        event.initEvent(type, true, true);
        for (var name in init || {}) event[name] = init[name];
        return el.dispatchEvent(event);
    }

    function activeElement(doc) {
        var el = doc.activeElement;
        if (el && (el.tagName === "FRAME" || el.tagName === "IFRAME")) {
            var inner = frameDocument(el);
            if (inner) return activeElement(inner);
        }
        return el || doc.body;
    }

    function clickElement(el, twice) {
        if (el.scrollIntoView) el.scrollIntoView(false);
        fire(el, "mouseover");
        fire(el, "mousedown");
        if (el.focus)
            try {
                el.focus();
            } catch (e) {}
        fire(el, "mouseup");
        if (el.click) el.click();
        else fire(el, "click");
        if (twice) fire(el, "dblclick");
    }

    // The element under a point of the top window, looking into frames as far as they go.
    function at(doc, x, y) {
        var el = doc.elementFromPoint(x, y);
        if (el && (el.tagName === "FRAME" || el.tagName === "IFRAME")) {
            var inner = frameDocument(el);
            var box = el.getBoundingClientRect();
            if (inner) return at(inner, x - box.left, y - box.top);
        }
        return el;
    }

    function describe(el) {
        var text = clip(el.innerText || el.value || "");
        return el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (text ? " " + quote(text) : "");
    }

    function choose(el, wanted) {
        var needle = String(wanted).toLowerCase();
        for (var pass = 0; pass < 2; pass++) {
            for (var i = 0; i < el.options.length; i++) {
                var option = el.options[i];
                var text = trim(option.text).toLowerCase();
                var hit = pass === 0 ? option.value === wanted || text === needle : text.indexOf(needle) >= 0;
                if (hit) {
                    el.selectedIndex = i;
                    fire(el, "change");
                    return "selected " + quote(trim(option.text));
                }
            }
        }
        throw new Error("no option matches " + quote(wanted));
    }

    function allText(doc) {
        var text = doc.body ? doc.body.innerText || "" : "";
        var frames = doc.getElementsByTagName("FRAME");
        var iframes = doc.getElementsByTagName("IFRAME");
        for (var i = 0; i < frames.length + iframes.length; i++) {
            var inner = frameDocument(i < frames.length ? frames[i] : iframes[i - frames.length]);
            if (inner) text += "\n" + allText(inner);
        }
        return text;
    }

    w.__drift = {
        snapshot: function (interactive) {
            var out = [];
            walk(root(w.document), 0, out, interactive);
            return out.join("\n");
        },
        text: function () {
            return allText(w.document);
        },
        click: function (ref, twice) {
            clickElement(element(ref), twice);
            return "Clicked ref " + ref + ".";
        },
        clickAt: function (x, y, twice) {
            var el = at(w.document, x, y);
            if (!el) throw new Error("nothing is at " + x + "," + y);
            clickElement(el, twice);
            return "Clicked " + describe(el) + ".";
        },
        hover: function (ref, x, y) {
            var el = ref ? element(ref) : at(w.document, x, y);
            if (!el) throw new Error("nothing is there to hover");
            fire(el, "mouseover");
            fire(el, "mouseenter");
            return "Hovered " + (ref ? "ref " + ref : describe(el)) + ".";
        },
        fill: function (ref, text, checked) {
            var el = element(ref);
            if (el.tagName === "SELECT") return "Ref " + ref + ": " + choose(el, text) + ".";
            if (el.type === "checkbox" || el.type === "radio") {
                var want = checked === null ? true : checked;
                if (el.checked === want) return "Ref " + ref + ": already so.";
                clickElement(el, false);
                return "Clicked ref " + ref + " to change it.";
            }
            if (el.focus)
                try {
                    el.focus();
                } catch (e) {}
            el.value = text;
            fire(el, "keyup");
            fire(el, "change");
            return text ? "Filled ref " + ref + "." : "Cleared ref " + ref + ".";
        },
        scroll: function (ref, dy) {
            if (ref) {
                element(ref).scrollIntoView(true);
                return "Scrolled ref " + ref + " into view.";
            }
            w.scrollBy(0, dy);
            return "Scrolled " + dy + " pixels.";
        },
        type: function (text) {
            var el = activeElement(w.document);
            if (el.value === undefined || el === el.ownerDocument.body)
                throw new Error("nothing that takes text has focus");
            for (var i = 0; i < text.length; i++) {
                var code = text.charCodeAt(i);
                fire(el, "keydown", { keyCode: code });
                fire(el, "keypress", { keyCode: code });
                el.value += text.charAt(i);
                fire(el, "keyup", { keyCode: code });
            }
            fire(el, "change");
        },
        press: function (key, code, ctrl, shift, alt) {
            var el = activeElement(w.document);
            var init = { keyCode: code, ctrlKey: ctrl, shiftKey: shift, altKey: alt };
            var go = fire(el, "keydown", init);
            if (key.length === 1 && !ctrl && el.value !== undefined && go !== false) el.value += key;
            if (key === "Backspace" && el.value) el.value = el.value.substring(0, el.value.length - 1);
            fire(el, "keyup", init);
            if (key === "Enter" && go !== false && el.form && el.tagName === "INPUT") {
                var buttons = el.form.getElementsByTagName("INPUT");
                for (var i = 0; i < buttons.length; i++)
                    if (buttons[i].type === "submit" || buttons[i].type === "image")
                        return clickElement(buttons[i], false);
                el.form.submit();
            }
        },
        size: function () {
            var doc = w.document;
            var box = doc.documentElement && doc.documentElement.clientWidth ? doc.documentElement : doc.body;
            return box.clientWidth + "x" + box.clientHeight;
        }
    };
})(window);
