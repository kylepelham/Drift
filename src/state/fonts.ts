import { createSignal } from "solid-js";
import { shellInvoke } from "../shell";

/** One installed font family, and whether it is monospaced. */
export type SystemFont = { family: string; monospace: boolean };

const [fonts, setFonts] = createSignal<SystemFont[]>();
let asked = false;

/** The fonts on this computer, asked of the shell once; empty where there is no shell to ask (a phone, a browser). */
export function systemFonts() {
    if (!asked) {
        asked = true;
        const invoke = shellInvoke();
        if (!invoke) setFonts([]);
        else
            void invoke<SystemFont[]>("system_fonts")
                .then(setFonts)
                .catch(() => setFonts([]));
    }

    return fonts();
}

/** The family a CSS font list names first, without its quotes: `"Inter", sans-serif` is `Inter`. */
export function firstFamily(list: string) {
    return (list.split(",")[0] ?? "").trim().replace(/^["']|["']$/g, "");
}

/** A family as a CSS font list, with the generic fallback a picker of its kind needs. */
export function fontList(family: string, mono: boolean) {
    return `"${family}", ${mono ? "monospace" : "sans-serif"}`;
}
