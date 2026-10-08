import { AmbiguousCitationError, citationHref, classifyMarkdownLink, resolveMarkdownCitation } from "./markdown-links";
import { createEffect, createMemo, createSignal, onCleanup, onMount, Show, untrack } from "solid-js";
import { markdownNodeSignature, replaceMarkdownSuffix } from "./markdown-render";
import { previewParentDirectory, readFilePreview } from "../file-preview";
import { animateResponses, responseAnimationSpeed } from "../state/prefs";
import { filePreviewMime, filePreviewType } from "../file-preview-types";
import { endsInsideFence, prepareMarkdown } from "./markdown-prose";
import { shouldPreviewFile } from "../state/file-preview-prefs";
import { sanitizeMarkdownHtml } from "./markdown-sanitize";
import { observeMarkdownImages } from "./markdown-images";
import { openFilePreview } from "../state/file-preview";
import { highlightBlocks } from "./markdown-syntax";
import { syntaxTheme } from "../state/code";
import { backendInvoke } from "../backend";
import { openFile } from "../tool-actions";
import { openLightbox } from "./lightbox";
import { shellInvoke } from "../shell";
import { t } from "../state/i18n";
import { marked } from "marked";
import {
    responseAnimationInterruptEvent,
    responseBurstSize,
    responseRevealDuration,
    revealResponseNodes,
    shouldPreserveResponseReveal,
    shouldQueueResponseRedraw,
} from "./response-animation";

import type { BundledTheme } from "shiki";

export { sanitizeMarkdownDocumentHtml, sanitizeMarkdownHtml } from "./markdown-sanitize";
export { endsInsideFence, fixEscapedEmphasis, prepareMarkdown } from "./markdown-prose";
export { AsyncSizeCache, codeTokens, type SyntaxToken } from "./markdown-syntax";
export { codeChunks, ProgressiveCodeView } from "./progressive-code";
export { markdownImageAttribute } from "./markdown-images";

marked.use({ gfm: true, breaks: true });

const codeBlocks = new WeakMap<HTMLElement, { button: HTMLButtonElement; code: string }>();
const copiedFeedbackMs = 1600;
const codeBlockAttribute = "[data-code-block]";
const copyButtonAttribute = "[data-copy-code]";
const copyIcon =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>';
const copiedIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>';

export async function openMarkdownLink(
    event: MouseEvent,
    directory?: string,
    workspaceDirectory = directory,
    fileGroups?: () => readonly (readonly string[])[],
) {
    if (!markdownNavigationClick(event)) return;

    const anchor = (event.target as Element | null)?.closest<HTMLAnchorElement>("a[href]");
    if (!anchor) return;

    const href = anchor.getAttribute("href") ?? "";
    // Preserve browser handling for contact links and companion navigation.
    if (/^(?:mailto:|tel:|#\/)/i.test(href)) return;

    let link = classifyMarkdownLink(fileGroups ? citationHref(href) : href, directory);
    if (link.kind === "external") return openExternalMarkdownLink(event, link.url);

    // Cancel navigation before contextual resolution can reject an ambiguous citation.
    event.preventDefault();
    event.stopPropagation();
    if (link.kind === "fragment") {
        scrollMarkdownFragment(event.currentTarget as HTMLElement, link.hash);
        return;
    }
    if (link.kind === "unsupported") throw new Error("The link is invalid or its workspace directory is unavailable");
    if (fileGroups) link = resolveMarkdownCitation(href, directory, fileGroups());
    if (link.kind !== "file") return;

    const hash = href.includes("#") ? decodeURIComponent(href.slice(href.indexOf("#") + 1)) : undefined;

    await openWorkspaceFile(link, workspaceDirectory, hash);
}

async function openExternalMarkdownLink(event: MouseEvent, url: string) {
    const invoke = shellInvoke();
    if (!invoke) return;

    event.preventDefault();
    event.stopPropagation();
    await invoke("plugin:opener|open_url", { url });
}

function markdownNavigationClick(event: MouseEvent) {
    return !event.defaultPrevented && (event.button === 0 || event.button === 1);
}

function scrollMarkdownFragment(root: HTMLElement, hash: string) {
    const id = decodeURIComponent(hash.slice(1));
    const target = [...root.querySelectorAll<HTMLElement>("[id]")].find((item) => item.id === id);

    target?.scrollIntoView({ block: "nearest" });
}

/** Opens images in the lightbox, previewable files in the viewer, and other files in the editor. */
export async function openWorkspaceFile(
    link: { path: string; line?: number; column?: number },
    workspaceDirectory?: string,
    hash?: string,
) {
    if (filePreviewType(link.path) === "image" && shouldPreviewFile(link.path) && backendInvoke())
        return openImageLink(link.path);
    if (workspaceDirectory && shouldPreviewFile(link.path) && backendInvoke()) {
        openFilePreview({ ...link, directory: workspaceDirectory, hash });
        return;
    }

    await openFile(link.path, { line: link.line, column: link.column, editorOnly: true });
}

// Explicit image links use their own folder so screenshots outside the workspace can open.
async function openImageLink(path: string) {
    const { bytes } = await readFilePreview({ path, directory: previewParentDirectory(path) });
    const mime = filePreviewMime(path);

    openLightbox({
        url: "",
        blob: new Blob([bytes], { type: mime }),
        filename: path.slice(path.lastIndexOf("/") + 1),
        mime,
    });
}

export function decorateCodeBlocks(root: HTMLElement) {
    for (const pre of root.querySelectorAll<HTMLElement>("pre")) {
        const code = pre.querySelector("code")?.textContent ?? pre.textContent ?? "";
        const existing = pre.closest<HTMLElement>(codeBlockAttribute);
        const block = existing ? codeBlocks.get(existing) : undefined;
        if (block) {
            block.code = code;
            continue;
        }

        const wrapper = document.createElement("div");
        wrapper.className = "code-block";
        wrapper.dataset.codeBlock = "";
        const button = document.createElement("button");
        button.type = "button";
        button.className = "code-copy";
        button.dataset.copyCode = "";
        button.innerHTML = copyIcon;
        button.setAttribute("aria-label", t("drift.markdown.copyCode"));
        button.title = t("drift.markdown.copyCode");

        pre.before(wrapper);
        wrapper.append(pre, button);
        codeBlocks.set(wrapper, { button, code });
    }
}

export function markdownClick(
    event: MouseEvent,
    directory?: string,
    workspaceDirectory = directory,
    fileGroups?: () => readonly (readonly string[])[],
) {
    if (event.defaultPrevented || event.button !== 0) return;

    const button = (event.target as Element).closest<HTMLButtonElement>(copyButtonAttribute);
    if (!button) return openMarkdownLink(event, directory, workspaceDirectory, fileGroups);

    const wrapper = button.closest<HTMLElement>(codeBlockAttribute);
    const block = wrapper ? codeBlocks.get(wrapper) : undefined;
    // Authored data attributes cannot identify a trusted copy control.
    if (!block || block.button !== button) return openMarkdownLink(event, directory, workspaceDirectory, fileGroups);

    event.preventDefault();
    event.stopPropagation();
    void writeClipboard(block.code)
        .then(() => {
            button.innerHTML = copiedIcon;
            button.setAttribute("aria-label", t("drift.markdown.codeCopied"));
            button.title = t("drift.markdown.copied");

            setTimeout(() => {
                button.innerHTML = copyIcon;
                button.setAttribute("aria-label", t("drift.markdown.copyCode"));
                button.title = t("drift.markdown.copyCode");
            }, copiedFeedbackMs);
        })
        .catch((error) => console.warn("[Drift] Could not copy code", error));
}

async function writeClipboard(text: string) {
    try {
        await navigator.clipboard.writeText(text);
        return;
    } catch {
        const input = document.createElement("textarea");
        input.value = text;
        input.style.position = "fixed";
        input.style.opacity = "0";

        document.body.append(input);
        input.select();
        const copied = document.execCommand("copy");
        input.remove();
        if (!copied) throw new Error("clipboard write was rejected");
    }
}

export function Markdown(props: {
    text: string;
    directory?: string;
    workspaceDirectory?: string;
    fileGroups?: () => readonly (readonly string[])[];
    documentPreview?: boolean;
    done?: boolean;
    humanAuthored?: boolean;
    responseID?: string;
    live?: boolean;
    revision?: number;
}) {
    let root!: HTMLDivElement;
    let request = 0;
    let identity = untrack(() => props.responseID);
    const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const animationAllowed = () => animateResponses() && !!props.responseID && !reducedMotion();

    let sourceSignatures: string[] = [];
    let sourceNodes: ChildNode[] = [];
    let renderedTheme: BundledTheme | undefined;
    let renderedRevision = untrack(() => props.revision);
    let previousLength = 0;
    let mounted = false;

    let revealActive = false;
    let revealQueued = false;
    let revealDone = false;
    let flushReveal = false;
    let finishReveal = () => {};
    const [revealRevision, setRevealRevision] = createSignal(0);
    const [linkError, setLinkError] = createSignal<string>();

    async function handleClick(event: MouseEvent) {
        setLinkError(undefined);

        try {
            if (event.type === "auxclick")
                await openMarkdownLink(
                    event,
                    props.directory,
                    props.workspaceDirectory ?? props.directory,
                    props.fileGroups,
                );
            else
                await markdownClick(
                    event,
                    props.directory,
                    props.workspaceDirectory ?? props.directory,
                    props.fileGroups,
                );
        } catch (cause) {
            setLinkError(
                cause instanceof AmbiguousCitationError
                    ? t("drift.markdown.ambiguousCitation", { href: cause.href, files: cause.files.join(", ") })
                    : `${t("drift.markdown.linkFailed")} ${cause instanceof Error ? cause.message : String(cause)}`,
            );
        }
    }

    // Reconciliation can replace text without notifying consumers, so revision forces reparsing.
    const html = createMemo(() => {
        void props.revision;

        return sanitizeMarkdownHtml(
            marked.parse(props.documentPreview ? props.text : prepareMarkdown(props.text, props.humanAuthored), {
                async: false,
            }),
            props.documentPreview,
        );
    });
    createEffect(() => {
        if (props.documentPreview) return;

        void props.responseID;
        onCleanup(
            observeMarkdownImages(root, {
                parent: props.directory,
                directory: props.workspaceDirectory ?? props.directory,
                enabled: shouldPreviewFile("image.png"),
            }),
        );
    });
    onMount(() => window.addEventListener(responseAnimationInterruptEvent, finishActiveReveal));
    onCleanup(() => {
        request++;
        revealActive = false;
        revealQueued = false;
        revealDone = false;
        finishReveal();
        window.removeEventListener(responseAnimationInterruptEvent, finishActiveReveal);
    });

    function finishActiveReveal() {
        if (!revealActive) return;

        const queued = revealQueued;
        revealActive = false;
        revealQueued = false;
        revealDone = false;
        const finish = finishReveal;
        finishReveal = () => {};
        finish();
        if (!queued) return;

        flushReveal = true;
        setRevealRevision((value) => value + 1);
    }

    function pendingRevealBurst(
        textLength: number,
        live: boolean,
        done: boolean,
        canAnimate: boolean,
        identityChanged: boolean,
    ) {
        if (!mounted || identityChanged || !canAnimate) return 0;

        return revealDone
            ? Math.max(0, textLength - previousLength)
            : responseBurstSize(previousLength, textLength, live, done);
    }

    function preserveReveal(
        change: { themeChanged: boolean; identityChanged: boolean; canAnimate: boolean },
        textLength: number,
        live: boolean,
        done: boolean,
    ) {
        return (
            !flushReveal &&
            !change.themeChanged &&
            !change.identityChanged &&
            change.canAnimate &&
            shouldPreserveResponseReveal(revealActive, previousLength, textLength, live, done)
        );
    }

    createEffect(() => {
        revealRevision();
        const revision = props.revision;
        const revisionChanged = revision !== renderedRevision;
        const theme = syntaxTheme() as BundledTheme;
        const responseID = props.responseID;
        const identityChanged = responseID !== identity;
        const textLength = props.text.length;
        const live = !!props.live;
        const done = !!props.done;
        const canAnimate = animationAllowed();
        const themeChanged = renderedTheme !== theme;
        const burst = pendingRevealBurst(textLength, live, done, canAnimate, identityChanged);

        if (preserveReveal({ themeChanged, identityChanged, canAnimate }, textLength, live, done)) {
            revealQueued ||= shouldQueueResponseRedraw(previousLength, textLength, revisionChanged);
            revealDone ||= done && textLength > previousLength;
            return;
        }

        const source = html();
        const current = ++request;
        renderedTheme = theme;
        const finish = finishReveal;
        finishReveal = () => {};
        revealActive = false;
        revealQueued = false;
        revealDone = false;
        finish();
        const revealBurst = flushReveal ? 0 : burst;
        flushReveal = false;

        if (responseID && !themeChanged && !identityChanged) {
            const update = replaceMarkdownSuffix(root, source, sourceSignatures, sourceNodes, revealBurst > 0);
            sourceSignatures = update.signatures;
            sourceNodes = update.nodes;
            if (update.revealNodes.length) {
                const complete = revealResponseNodes(
                    update.revealNodes,
                    responseRevealDuration(update.revealedCharacters, responseAnimationSpeed()),
                    () => {
                        if (finishReveal !== complete) return;

                        finishReveal = () => {};
                        revealActive = false;
                        if (!revealQueued) return;

                        revealQueued = false;
                        setRevealRevision((value) => value + 1);
                    },
                );
                finishReveal = complete;
                revealActive = true;
            }
        } else {
            root.innerHTML = source;
            sourceNodes = responseID ? [...root.childNodes].map((node) => node.cloneNode(true) as ChildNode) : [];
            sourceSignatures = sourceNodes.map(markdownNodeSignature);
        }

        identity = responseID;
        previousLength = textLength;
        renderedRevision = revision;
        mounted = true;
        decorateCodeBlocks(root);
        void highlightBlocks(root, theme, () => current === request, !props.done && endsInsideFence(props.text)).then(
            () => {
                if (current === request) decorateCodeBlocks(root);
            },
        );
    });

    return (
        <>
            <div
                ref={root}
                class="md"
                classList={{ "md-user": props.humanAuthored }}
                onClick={handleClick}
                onAuxClick={(event) => {
                    if (event.button === 1) void handleClick(event);
                }}
            />
            <Show when={linkError()}>
                {(error) => (
                    <div role="alert" class="mt-1 text-xs text-danger">
                        {error()}
                    </div>
                )}
            </Show>
        </>
    );
}
