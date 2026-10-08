import { citationHref, classifyMarkdownLink } from "./markdown-links";
import { markdownImageAttribute } from "./markdown-images";
import DOMPurify from "dompurify";

let markdownPurifier: ReturnType<typeof DOMPurify> | undefined;

export function sanitizeMarkdownDocumentHtml(html: string) {
    // Document sanitization must not inherit the transcript's hooks or insert raw HTML.
    const purifier = DOMPurify(window);
    const images = new WeakMap<Node, string>();
    purifier.addHook("beforeSanitizeAttributes", (node) => {
        if (!(node instanceof window.Element)) return;

        node.removeAttribute(markdownImageAttribute);
        if (node.nodeName !== "IMG" || node.namespaceURI !== "http://www.w3.org/1999/xhtml") return;

        const raw = node.getAttribute("src") ?? "";
        const link = classifyMarkdownLink(raw, "/");
        if (link.kind === "file" && !link.path.startsWith("//")) images.set(node, raw);
    });
    purifier.addHook("uponSanitizeAttribute", (node, attribute) => {
        if (
            attribute.attrName === "class" &&
            (node.nodeName !== "CODE" || !/^language-[\w-]+$/.test(attribute.attrValue))
        )
            attribute.keepAttr = false;
        if (
            attribute.attrName === "href" &&
            node.nodeName === "A" &&
            node.namespaceURI === "http://www.w3.org/1999/xhtml" &&
            classifyMarkdownLink(attribute.attrValue).kind === "file"
        )
            attribute.forceKeepAttr = true;
    });
    purifier.addHook("afterSanitizeAttributes", (node) => {
        const raw = images.get(node);
        if (raw !== undefined) node.setAttribute(markdownImageAttribute, raw);
        else if (node.nodeName === "IMG") node.setAttribute("title", "Only local workspace images can be previewed");
    });

    const clean = purifier.sanitize(html, {
        ALLOWED_TAGS: [
            "a",
            "p",
            "br",
            "hr",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "blockquote",
            "pre",
            "code",
            "em",
            "strong",
            "b",
            "i",
            "del",
            "s",
            "u",
            "sub",
            "sup",
            "kbd",
            "samp",
            "var",
            "ul",
            "ol",
            "li",
            "dl",
            "dt",
            "dd",
            "table",
            "caption",
            "thead",
            "tbody",
            "tfoot",
            "tr",
            "th",
            "td",
            "div",
            "span",
            "details",
            "summary",
            "figure",
            "figcaption",
            "img",
        ],
        ALLOWED_ATTR: ["href", "alt", "title", "class", "colspan", "rowspan", "scope", "start", "reversed"],
        ALLOW_DATA_ATTR: false,
        ALLOW_ARIA_ATTR: false,
        FORBID_CONTENTS: [
            "script",
            "style",
            "svg",
            "math",
            "iframe",
            "object",
            "embed",
            "audio",
            "video",
            "picture",
            "template",
        ],
        RETURN_DOM_FRAGMENT: true,
    });

    const ids = new Set<string>();
    const suffixes = new Map<string, number>();
    for (const heading of clean.querySelectorAll<HTMLElement>("h1,h2,h3,h4,h5,h6")) {
        const base =
            (heading.textContent ?? "")
                .trim()
                .toLowerCase()
                .replace(/[^\p{L}\p{N}\s_-]/gu, "")
                .replace(/\s+/g, "-") || "section";
        let suffix = suffixes.get(base) ?? 0;
        let id = suffix ? `${base}-${suffix}` : base;
        while (ids.has(id)) id = `${base}-${++suffix}`;

        suffixes.set(base, suffix + 1);
        ids.add(id);
        heading.id = id;
    }

    const template = document.createElement("template");
    template.content.append(clean);

    return template.innerHTML;
}

export function sanitizeMarkdownHtml(html: string, documentPreview = false) {
    if (documentPreview) return sanitizeMarkdownDocumentHtml(html);

    if (!markdownPurifier) {
        markdownPurifier = DOMPurify(window);
        const images = new WeakMap<Node, string>();
        markdownPurifier.addHook("beforeSanitizeAttributes", (node) => {
            if (!(node instanceof window.Element)) return;

            node.removeAttribute(markdownImageAttribute);
            // Responsive image sources must not bypass the workspace reader with browser-relative URLs.
            if (node.nodeName === "IMG" || node.nodeName === "SOURCE") node.removeAttribute("srcset");
            if (node.nodeName !== "IMG" || node.namespaceURI !== "http://www.w3.org/1999/xhtml") return;

            const raw = node.getAttribute("src") ?? "";
            const link = classifyMarkdownLink(raw, "/");
            if (link.kind === "external" || /^data:image\//i.test(raw)) return;

            node.removeAttribute("src");
            if (link.kind === "file" && !link.path.startsWith("//")) images.set(node, raw);
        });
        markdownPurifier.addHook("uponSanitizeAttribute", (node, attribute) => {
            // Only local anchor destinations bypass the URI filter, never image sources or other schemes.
            if (
                node.nodeName === "A" &&
                node.namespaceURI === "http://www.w3.org/1999/xhtml" &&
                attribute.attrName === "href" &&
                classifyMarkdownLink(citationHref(attribute.attrValue), "/").kind === "file"
            )
                attribute.forceKeepAttr = true;
        });
        markdownPurifier.addHook("afterSanitizeAttributes", (node) => {
            const raw = images.get(node);
            if (raw !== undefined) node.setAttribute(markdownImageAttribute, raw);
        });
    }

    return markdownPurifier.sanitize(html);
}
