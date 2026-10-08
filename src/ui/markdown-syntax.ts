import { syntaxTheme } from "../state/code";
import DOMPurify from "dompurify";

import type { BundledLanguage, BundledTheme, SpecialLanguage } from "shiki";
import type * as Shiki from "shiki";

export type SyntaxToken = { content: string; color?: string; fontStyle?: number };
type AsyncCacheEntry<T> = { value: Promise<T>; size: number };

export class AsyncSizeCache<T> {
    private entries = new Map<string, AsyncCacheEntry<T>>();
    private total = 0;

    constructor(
        private limit: number,
        private outputSize: (value: T) => number,
    ) {}

    get size() {
        return this.total;
    }
    get count() {
        return this.entries.size;
    }

    get(key: string) {
        const entry = this.entries.get(key);
        if (!entry) return undefined;

        this.entries.delete(key);
        this.entries.set(key, entry);

        return entry.value;
    }

    /** Shares in-flight promises, charges source size until resolution, and evicts failures for retry. */
    set(key: string, sourceSize: number, value: Promise<T>) {
        const existing = this.entries.get(key);
        if (existing) this.remove(key, existing);
        if (sourceSize > this.limit) return value;

        const entry: AsyncCacheEntry<T> = { value, size: sourceSize };
        const tracked = value.then(
            (result) => {
                // Evicted or replaced entries no longer contribute to the running total.
                if (this.entries.get(key) !== entry) return result;

                this.total -= entry.size;
                entry.size = sourceSize + this.outputSize(result);
                this.total += entry.size;
                this.trim();

                return result;
            },
            (error) => {
                if (this.entries.get(key) === entry) this.remove(key, entry);

                throw error;
            },
        );

        entry.value = tracked;
        this.entries.set(key, entry);
        this.total += entry.size;
        this.trim();

        return tracked;
    }

    private trim() {
        while (this.total > this.limit && this.entries.size) {
            const key = this.entries.keys().next().value;
            if (key === undefined) break;

            this.remove(key, this.entries.get(key)!);
        }
    }

    private remove(key: string, entry: AsyncCacheEntry<T>) {
        if (!this.entries.delete(key)) return;

        this.total -= entry.size;
    }
}

// Approximate object overhead prevents many small entries from escaping the byte budget.
const perLineOverhead = 16;
const perTokenOverhead = 32;
const perSourceEntryOverhead = 128;
const perHtmlEntryOverhead = 64;
const shikiCacheBudget = 2 * 1024 * 1024;
const highlightCache = new AsyncSizeCache<string>(shikiCacheBudget, (html) => html.length + perHtmlEntryOverhead);
const tokenCache = new AsyncSizeCache<SyntaxToken[][]>(shikiCacheBudget, tokenOutputSize);
let shikiModule: Promise<typeof Shiki> | undefined;

function tokenOutputSize(lines: SyntaxToken[][]) {
    let size = lines.length * perLineOverhead;

    for (const line of lines) {
        for (const token of line) size += token.content.length + perTokenOverhead;
    }

    return size;
}

function shikiSourceSize(key: string, code: string) {
    return key.length + code.length + perSourceEntryOverhead;
}

export async function codeTokens(code: string, lang: string): Promise<SyntaxToken[][]> {
    const theme = syntaxTheme() as BundledTheme;
    const key = `${theme}\0${lang}\0${code}`;
    const cached = tokenCache.get(key);
    if (cached) return cached.catch(() => []);

    const result = (shikiModule ??= import("shiki"))
        .then((shiki) => shiki.codeToTokens(code, { lang: lang as BundledLanguage | SpecialLanguage, theme }))
        .then((value) =>
            value.tokens.map((line) =>
                line.map((token) => ({ content: token.content, color: token.color, fontStyle: token.fontStyle })),
            ),
        );

    return tokenCache.set(key, shikiSourceSize(key, code), result).catch(() => []);
}

export function highlightedCode(code: string, lang: string, theme: BundledTheme) {
    const key = `${theme}\0${lang}\0${code}`;
    const cached = highlightCache.get(key);
    if (cached) return cached;

    const result = (shikiModule ??= import("shiki"))
        .then((shiki) => shiki.codeToHtml(code, { lang, theme }))
        .then((html) => DOMPurify.sanitize(html));

    return highlightCache.set(key, shikiSourceSize(key, code), result);
}

/** Highlights fenced blocks, leaving the last block alone while its fence remains open. */
export async function highlightBlocks(
    root: HTMLElement,
    theme: BundledTheme,
    current: () => boolean,
    skipLast = false,
) {
    const blocks = [...root.querySelectorAll<HTMLElement>("pre > code[class*='language-']")];
    if (skipLast) blocks.pop();

    await Promise.all(
        blocks.map(async (code) => {
            const lang = code.className.match(/language-([\w-]+)/)?.[1] ?? "text";
            const pre = code.parentElement;
            if (!pre) return;

            const html = await highlightedCode(code.textContent ?? "", lang, theme).catch(() => "");
            if (html && current() && pre.isConnected) pre.outerHTML = html;
        }),
    );
}
