// Tokenizes code off the main thread, so highlighting a large file never stalls scrolling or typing.
import { shikiTokens } from "../syntax-grammars";

import type { BundledTheme, LanguageRegistration } from "shiki";
import type * as Shiki from "shiki";

/** A plugin's grammar comes with the first request for it, under its id as `lang`. */
type Request = { id: number; code: string; lang: string; theme: BundledTheme; grammar?: LanguageRegistration };

let shiki: Promise<typeof Shiki> | undefined;
const grammars = new Map<string, LanguageRegistration>();

self.onmessage = async (event: MessageEvent<Request>) => {
    const { id, code, lang, theme, grammar } = event.data;
    if (grammar) grammars.set(lang, grammar);
    try {
        const module = await (shiki ??= import("shiki"));
        const result = await shikiTokens(module, code, lang, theme, grammars.get(lang));
        const tokens = result.tokens.map((line) =>
            line.map((token) => ({ content: token.content, color: token.color, fontStyle: token.fontStyle })),
        );
        self.postMessage({ id, tokens });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
