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

    const result = workerTokens(code, lang, theme) ?? mainThreadTokens(code, lang, theme);

    return tokenCache.set(key, shikiSourceSize(key, code), result).catch(() => []);
}

function mainThreadTokens(code: string, lang: string, theme: BundledTheme) {
    return (shikiModule ??= import("shiki"))
        .then((shiki) => shiki.codeToTokens(code, { lang: lang as BundledLanguage | SpecialLanguage, theme }))
        .then((value) =>
            value.tokens.map((line) =>
                line.map((token) => ({ content: token.content, color: token.color, fontStyle: token.fontStyle })),
            ),
        );
}

type WorkerReply = { id: number; tokens?: SyntaxToken[][]; error?: string };
let worker: Worker | undefined;
let workerBroken = false;
let nextRequest = 0;
const waiting = new Map<number, { resolve: (tokens: SyntaxToken[][]) => void; reject: (error: Error) => void }>();

/** Tokens from the syntax worker; none where workers are unavailable (the test runner), so the main thread does it. */
function workerTokens(code: string, lang: string, theme: BundledTheme): Promise<SyntaxToken[][]> | undefined {
    if (workerBroken || typeof Worker === "undefined") return undefined;
    worker ??= startWorker();
    if (!worker) return undefined;

    const id = ++nextRequest;
    return new Promise((resolve, reject) => {
        waiting.set(id, { resolve, reject });
        worker!.postMessage({ id, code, lang, theme });
    });
}

function startWorker() {
    try {
        const started = new Worker(new URL("./syntax-worker.ts", import.meta.url), { type: "module" });
        started.onmessage = (event: MessageEvent<WorkerReply>) => {
            const request = waiting.get(event.data.id);
            waiting.delete(event.data.id);
            if (event.data.tokens) request?.resolve(event.data.tokens);
            else request?.reject(new Error(event.data.error ?? "highlighting failed"));
        };
        // A worker that cannot start leaves highlighting to the main thread from then on.
        started.onerror = () => {
            workerBroken = true;
            worker = undefined;
            for (const request of waiting.values()) request.reject(new Error("syntax worker failed"));
            waiting.clear();
        };
        return started;
    } catch {
        workerBroken = true;
        return undefined;
    }
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
