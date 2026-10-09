import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { resolveFileLanguage } from "../syntax-language";
import { highlightedCode } from "./markdown-syntax";
import { syntaxTheme } from "../state/code";

import type { BundledTheme } from "shiki";

const chunkLines = 160;
// Highlight chunks before they reach the viewport so scrolling does not land on plain code.
const chunkPrefetchMargin = "320px 0px";

function CodeView(props: { code: string; lang: string }) {
    const [html, setHtml] = createSignal("");
    let request = 0;

    createEffect(() => {
        const { code, lang } = props;
        const shikiTheme = syntaxTheme() as BundledTheme;
        const current = ++request;

        setHtml("");
        void highlightedCode(code, lang, shikiTheme)
            .then((output) => current === request && setHtml(output))
            .catch(() => current === request && setHtml(""));
    });

    return (
        <Show when={html()} fallback={<pre>{props.code}</pre>}>
            {/* eslint-disable-next-line solid/no-innerhtml -- highlightedCode sanitises Shiki output with DOMPurify. */}
            <div innerHTML={html()} />
        </Show>
    );
}

export function codeChunks(code: string) {
    const lines = code.replace(/\r\n?/g, "\n").split("\n");
    const result: string[] = [];

    for (let index = 0; index < lines.length; index += chunkLines)
        result.push(lines.slice(index, index + chunkLines).join("\n"));

    return result;
}

export function ProgressiveCodeView(props: { code: string; filename: string; fill?: boolean; line?: number }) {
    let root!: HTMLDivElement;
    let observer: IntersectionObserver | undefined;
    const chunks = createMemo(() => codeChunks(props.code));
    const [active, setActive] = createSignal(new Set([0]));
    const [language, setLanguage] = createSignal<string>();
    let languageRequest = 0;

    createEffect(() => {
        const filename = props.filename;
        const current = ++languageRequest;

        setLanguage(undefined);
        void resolveFileLanguage(filename)
            .then((result) => {
                if (current === languageRequest) setLanguage(result);
            })
            .catch(() => {
                if (current === languageRequest) setLanguage("text");
            });
    });

    createEffect(() => {
        const count = chunks().length;

        queueMicrotask(() => {
            observer?.disconnect();
            if (!("IntersectionObserver" in window))
                return setActive(new Set(Array.from({ length: count }, (_, index) => index)));

            observer = new IntersectionObserver(
                (entries) => {
                    const visible = entries
                        .filter((entry) => entry.isIntersecting)
                        .map((entry) => Number((entry.target as HTMLElement).dataset.chunk));
                    if (!visible.length) return;

                    setActive((current) => new Set([...current, ...visible]));
                },
                { root, rootMargin: chunkPrefetchMargin },
            );
            for (const element of root.querySelectorAll("[data-chunk]")) observer.observe(element);
        });
    });
    onCleanup(() => observer?.disconnect());

    onMount(() => {
        if (!props.line) return;

        const frame = requestAnimationFrame(() => {
            const chunk = Math.min(chunks().length - 1, Math.floor((props.line! - 1) / chunkLines));
            root.querySelector<HTMLElement>(`[data-chunk="${chunk}"]`)?.scrollIntoView({ block: "start" });
        });
        onCleanup(() => cancelAnimationFrame(frame));
    });

    return (
        <div
            ref={root}
            class="transcript-tool-output code-view code-stream overflow-auto rounded-lg border border-edge"
            classList={{ "max-h-80": !props.fill, "min-h-0 flex-1": props.fill }}
        >
            <For each={chunks()}>
                {(code, index) => (
                    <div class="code-stream-chunk" data-chunk={index()}>
                        <Show when={active().has(index()) ? language() : undefined} fallback={<pre>{code}</pre>}>
                            {(lang) => <CodeView code={code} lang={lang()} />}
                        </Show>
                    </div>
                )}
            </For>
        </div>
    );
}
