import { createEffect, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import { formatBytes } from "../state/storage";
import { IconCheck, IconCopy } from "./icons";
import { openFile } from "../tool-actions";
import { t } from "../state/i18n";

// How long the copy button shows "copied"; markdown.tsx code blocks use 1600ms.
const copiedFeedbackMs = 2000;
// Trailing characters compared to confirm streamed output was appended rather than replaced.
const overlapProbeChars = 64;
// Scroll positions within this many pixels of the bottom count as "at the bottom".
const bottomSlopPx = 2;

export function stripAnsi(value: string) {
    return value.replace(/\u001b(?:\[[0-?]*[ -/]*[@-~]|[@-_])/g, "");
}

/**
 * A call's output without the notes Drift appended (`metadata.notes`), and those notes, shown under the call instead.
 */
export function splitNotes(output: string, notes: unknown): { output: string; notes: string[] } {
    const listed = Array.isArray(notes) ? notes.filter((note): note is string => typeof note === "string") : [];
    let rest = output;
    const shown: string[] = [];
    for (const note of [...listed].reverse()) {
        if (rest === note) rest = "";
        else if (rest.endsWith(`\n\n${note}`)) rest = rest.slice(0, rest.length - note.length - 2);
        else break;
        shown.unshift(note);
    }

    return { output: rest, notes: shown };
}

/** The line the engine puts where it cut a long output's middle out (`tool::spool`), and how much it cut. */
export function splitOmitted(text: string): { head: string; omitted: number; tail: string } | null {
    const found = /\n\n\.\.\. (\d+) bytes omitted; [^\n]* \.\.\.\n\n/.exec(text);
    if (!found) return null;
    return {
        head: text.slice(0, found.index),
        omitted: Number(found[1]),
        tail: text.slice(found.index + found[0].length),
    };
}

export function shellTranscript(command: string, output: string) {
    const normalized = stripAnsi(output).replace(/\r\n?/g, "\n");
    return `$ ${command}${normalized.trim() ? `\n\n${normalized}` : ""}`;
}

/**
 * Splits a replace-frame transcript into its command line and trailing output so the command can
 * be rendered with the accent `$ ` indicator. Replace frames always start with `$ ${command}`;
 * the fallback keeps unexpected frames rendering verbatim instead of dropping text.
 */
export function shellReplaceSegments(command: string, text: string) {
    const head = `$ ${command}`;
    if (!text.startsWith(head)) return { command: null, output: text };
    return { command, output: text.slice(head.length) };
}

/**
 * Renders streaming shell output incrementally.
 *
 * The engine re-sends the whole output on every update. Re-rendering all of it each time is too
 * slow for a long-running command, so this tracks how much has already been shown and emits only
 * the new tail when it can prove the new output is an append of the old one. When it cannot, it
 * emits a full replacement.
 *
 * Callers get `{ replace, text }`: `replace: true` means "this is the whole transcript",
 * `replace: false` means "append this".
 */
export function createShellTranscriptStream() {
    let command = "";
    /** How many characters of the raw output have already been turned into visible text. */
    let outputLength = 0;
    let previousOutput = "";
    let initialized = false;
    /** Set once non-whitespace output has been shown; until then output is buffered in `pending`. */
    let visible = false;
    let finished = false;
    /** ANSI escape parser state. CSI sequences run until a byte in the 0x40-0x7e final range. */
    let escape: "none" | "start" | "csi" = "none";
    /** A trailing CR is held over: it only becomes a newline once we know what follows it. */
    let carriageReturn = false;
    /** Whitespace-only chunks seen before the first visible output, kept so none are lost. */
    let pending: string[] = [];

    /** Strips ANSI escapes and folds CR / CRLF into newlines. `flush` resolves a trailing CR. */
    const consume = (value: string, flush: boolean) => {
        let normalized = "";
        for (const character of value) {
            if (escape === "start") {
                escape = character === "[" ? "csi" : "none";
                continue;
            }
            if (escape === "csi") {
                const code = character.charCodeAt(0);
                if (code >= 0x40 && code <= 0x7e) escape = "none";
                continue;
            }
            if (character === "\u001b") {
                escape = "start";
                continue;
            }
            if (carriageReturn) {
                normalized += "\n";
                carriageReturn = false;
                if (character === "\n") continue;
            }
            if (character === "\r") carriageReturn = true;
            else normalized += character;
        }
        if (flush && carriageReturn) {
            normalized += "\n";
            carriageReturn = false;
        }

        return normalized;
    };

    /** Re-renders the whole transcript from scratch and re-primes the incremental state. */
    const reset = (nextCommand: string, output: string, done: boolean) => {
        command = nextCommand;
        outputLength = output.length;
        previousOutput = output;
        initialized = true;
        finished = done;
        visible = false;
        escape = "none";
        carriageReturn = false;
        pending = [];

        const normalized = consume(output, done);
        visible = !!normalized.trim();
        if (!visible) pending = [normalized];

        return { replace: true, text: `$ ${command}${visible ? `\n\n${normalized}` : ""}` };
    };

    return {
        update(nextCommand: string, output: string, done: boolean) {
            // Nothing to append to: first update, new command, shrunk output, or a final frame to flush.
            if (!initialized || command !== nextCommand || output.length < outputLength || finished || done) {
                return reset(nextCommand, output, done);
            }
            // Same length: either a genuine no-op, or the content changed underneath us.
            if (output.length === outputLength) {
                if (output === previousOutput) return { replace: false, text: "" };
                return reset(nextCommand, output, done);
            }
            // The engine truncated the head and prefixed an ellipsis, so earlier offsets no longer line up.
            if (output.startsWith("...\n\n") && !previousOutput.startsWith("...\n\n"))
                return reset(nextCommand, output, done);
            // An append keeps the previous tail at the same offset; anything else was a rewrite.
            const overlap = previousOutput.slice(-overlapProbeChars);
            if (output.slice(outputLength - overlap.length, outputLength) !== overlap)
                return reset(nextCommand, output, done);

            const normalized = consume(output.slice(outputLength), false);
            outputLength = output.length;
            previousOutput = output;
            if (visible) return { replace: false, text: normalized };
            // Leading whitespace waits for real output, then emits as one block.
            pending.push(normalized);
            const combined = pending.join("");
            if (!combined.trim()) return { replace: false, text: "" };
            visible = true;
            pending = [];
            return { replace: true, text: `$ ${command}\n\n${combined}` };
        },
    };
}

export function createFrameCoalescer<T>(
    schedule: (callback: () => void) => number,
    cancel: (handle: number) => void,
    apply: (value: T) => void,
) {
    let frame: number | undefined;
    let latest: T;
    return {
        push(value: T, defer: boolean) {
            latest = value;
            if (!defer) {
                if (frame !== undefined) cancel(frame);
                frame = undefined;
                apply(latest);
                return;
            }

            if (frame !== undefined) return;

            frame = schedule(() => {
                frame = undefined;
                apply(latest);
            });
        },
        dispose() {
            if (frame !== undefined) cancel(frame);
            frame = undefined;
        },
    };
}

/** Where the engine cut a long output, a divider; its saved whole opens from the link. */
function omittedDivider(omitted: number, file?: string) {
    const divider = document.createElement("span");
    divider.className = "my-2 flex items-center gap-2 text-xs text-ink-faint select-none";

    const rule = () => {
        const line = document.createElement("span");
        line.className = "h-px flex-1 bg-edge";
        return line;
    };

    const label = document.createElement("span");
    label.textContent = t("drift.shell.omitted", { size: formatBytes(omitted) });
    divider.append(rule(), label);
    if (file) {
        const open = document.createElement("button");
        open.type = "button";
        open.className = "text-accent hover:underline";
        open.textContent = t("drift.shell.openFull");
        open.title = file;
        open.onclick = () => void openFile(file).catch(() => undefined);
        divider.append(open);
    }
    divider.append(rule());

    return divider;
}

export function ShellOutput(props: { command: string; output: string; running: boolean; file?: string }) {
    const [copied, setCopied] = createSignal(false);
    const [renderRevision, setRenderRevision] = createSignal(0);
    let viewport!: HTMLPreElement;
    let mounted = false;
    let savedTop = 0;
    let following = true;
    /** The text node holding streamed output, so appends never disturb the styled command line. */
    let outputNode: Text | undefined;
    const stream = createShellTranscriptStream();
    const normalizer = createFrameCoalescer(
        requestAnimationFrame,
        cancelAnimationFrame,
        ({ command, output, running }: { command: string; output: string; running: boolean }) => {
            const update = stream.update(command, output, !running);

            if (!mounted) return;

            if (update.replace) {
                const segments = shellReplaceSegments(command, update.text);
                const cut = splitOmitted(segments.output);
                outputNode = document.createTextNode(cut ? cut.tail : segments.output);
                const shown: Node[] = cut
                    ? [document.createTextNode(cut.head), omittedDivider(cut.omitted, props.file), outputNode]
                    : [outputNode];
                if (segments.command === null) {
                    viewport.replaceChildren(...shown);
                } else {
                    const prompt = document.createElement("span");
                    prompt.className = "text-accent select-none";
                    prompt.textContent = "$ ";
                    const name = document.createElement("span");
                    name.className = "font-medium text-ink";
                    name.textContent = segments.command;
                    const trailing = document.createElement("span");
                    trailing.className = "text-ink-muted";
                    trailing.append(...shown);
                    viewport.replaceChildren(prompt, name, trailing);
                }
            } else if (update.text) {
                if (outputNode) outputNode.appendData(update.text);
                else viewport.append(update.text);
            }
            if (update.replace || update.text) setRenderRevision((value) => value + 1);
        },
    );
    onMount(() => {
        mounted = true;
        normalizer.push({ command: props.command, output: props.output, running: props.running }, false);
    });
    createEffect(
        on(
            () => [props.command, props.output, props.running] as const,
            ([command, output, running]) => {
                normalizer.push({ command, output, running }, running);
            },
            { defer: true },
        ),
    );
    onCleanup(() => normalizer.dispose());

    const copy = async () => {
        await navigator.clipboard.writeText(shellTranscript(props.command, props.output));
        setCopied(true);
        setTimeout(() => setCopied(false), copiedFeedbackMs);
    };

    createEffect(
        on(renderRevision, () => {
            const top = savedTop;
            const follow = following;
            queueMicrotask(() => {
                viewport.scrollTop = shellScrollTarget(top, follow, viewport.scrollHeight);
            });
        }),
    );

    return (
        <div class="group/shell relative overflow-hidden rounded-[6px] border-[0.5px] border-edge">
            <button
                title={t("drift.shell.copyOutput")}
                class="absolute top-1 right-1 z-10 flex size-6 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-focus-within/shell:opacity-100 group-hover/shell:opacity-100 hover:bg-raised hover:text-ink"
                onClick={() => void copy()}
            >
                <Show when={copied()} fallback={<IconCopy class="size-3.5" />}>
                    <IconCheck class="size-3.5" />
                </Show>
            </button>
            <pre
                ref={viewport}
                class="shell-output transcript-tool-output code-display max-h-60 overflow-x-hidden overflow-y-auto p-3 pr-10 font-mono leading-[1.5] text-ink"
                role="region"
                aria-label={t("drift.shell.output")}
                tabIndex={0}
                onScroll={(event) => {
                    savedTop = event.currentTarget.scrollTop;
                    following = shellAtBottom(
                        savedTop,
                        event.currentTarget.clientHeight,
                        event.currentTarget.scrollHeight,
                    );
                }}
            />
        </div>
    );
}

export function shellAtBottom(scrollTop: number, clientHeight: number, scrollHeight: number) {
    return scrollHeight - clientHeight - scrollTop <= bottomSlopPx;
}

export function shellScrollTarget(savedTop: number, following: boolean, scrollHeight: number) {
    return following ? scrollHeight : savedTop;
}
