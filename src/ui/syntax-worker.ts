// Tokenizes code off the main thread, so highlighting a large file never stalls scrolling or typing.
import type { BundledLanguage, BundledTheme, SpecialLanguage } from "shiki";
import type * as Shiki from "shiki";

type Request = { id: number; code: string; lang: string; theme: BundledTheme };

let shiki: Promise<typeof Shiki> | undefined;

self.onmessage = async (event: MessageEvent<Request>) => {
    const { id, code, lang, theme } = event.data;
    try {
        const module = await (shiki ??= import("shiki"));
        const result = await module.codeToTokens(code, { lang: lang as BundledLanguage | SpecialLanguage, theme });
        const tokens = result.tokens.map((line) =>
            line.map((token) => ({ content: token.content, color: token.color, fontStyle: token.fontStyle })),
        );
        self.postMessage({ id, tokens });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
