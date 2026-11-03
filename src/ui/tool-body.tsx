import { ReadImage, readImage, returnedImages, ToolImage } from "./tool-image";
import { toolInput, toolMetadata as toolMeta } from "../engine/parts";
import { ShellOutput, splitNotes, stripAnsi } from "./shell-output";
import { createMemo, For, Match, Show, Switch } from "solid-js";
import { citationFileGroups } from "./citation-files";
import { DiffPanel, PatchPanel } from "./diff-panel";
import { delegatedChildId } from "./tool-delegation";
import { inputFields, outputView } from "./tool-io";
import { ShellProcessStatus } from "./process-row";
import { ReadPanel, readPage } from "./tool-read";
import { toolDisplay } from "./tool-presentation";
import { patchFiles } from "./tool-labels";
import { Markdown } from "./markdown";
import { useEngine } from "../engine";
import { IconInfo } from "./icons";
import { t } from "../state/i18n";

import type { ToolPart } from "../engine/parts";

// Tool output beyond this is clipped before rendering; long outputs otherwise stall the view.
const maxInlineOutputChars = 4000;
// A tool's own result is what the card is opened for, so it keeps far more; the engine already caps it near 32 KB.
const maxReturnedChars = 20_000;

export function ToolBody(props: { part: ToolPart; diff: string | null; error: string | null }) {
    const engine = useEngine();
    const state = () => toolDisplay(props.part);
    const shellCommand = () => (state().input as { command?: string }).command ?? "";

    // While running, the output so far rides on the part's metadata; once ended, the saved result is the output.
    const shellOutput = () => {
        const current = state();
        if (current.status === "completed") return current.output;
        if (current.status === "error") return current.error;

        return (toolMeta(props.part)?.output as string | undefined) ?? "";
    };

    const shell = createMemo(() => splitNotes(shellOutput() ?? "", toolMeta(props.part)?.notes));
    const backgrounded = () => toolMeta(props.part)?.processId as string | undefined;

    // Until the engine reports its diff, a write being streamed shows as the diff it will be: every line added.
    const written = () => {
        if (props.part.name !== "write" || props.diff) return null;
        const input = state().input as { content?: string };
        return typeof input.content === "string" ? addedLines(input.content) : null;
    };

    const patched = () => (props.part.name === "apply_patch" ? patchFiles(props.part) : []);

    const diffFilename = () => {
        const input = state().input as { filePath?: string };
        const file = patched()[0];
        return file?.relativePath ?? file?.filePath ?? input.filePath ?? "";
    };

    const tasked = () => taskBody(props.part);

    const citationFiles = () => {
        const child = delegatedChildId(engine.state, props.part);
        const endedAt = props.part.finishedAt ?? props.part.startedAt ?? undefined;
        const until = props.part.status === "done" ? endedAt : undefined;

        return child
            ? citationFileGroups(engine.state, child, undefined, undefined, until)
            : citationFileGroups(engine.state, props.part.sessionId, props.part.messageId, props.part.id);
    };

    return (
        <>
            <Switch fallback={<GenericBody part={props.part} />}>
                <Match when={tasked()}>
                    {(task) => (
                        <div class="tool-card space-y-2">
                            <Show when={task().prompt}>
                                <div class="transcript-tool-output max-h-40 overflow-auto text-[length:calc(0.85rem*var(--ui-text-scale,1))] whitespace-pre-wrap text-ink-faint">
                                    {clip(task().prompt)}
                                </div>
                            </Show>
                            <Show when={task().result}>
                                <div class="transcript-tool-output max-h-80 overflow-auto text-ink-muted">
                                    <Markdown
                                        text={task().result}
                                        directory={
                                            engine.state.sessions[
                                                delegatedChildId(engine.state, props.part) ?? props.part.sessionId
                                            ]?.directory
                                        }
                                        fileGroups={citationFiles}
                                        done
                                    />
                                </div>
                            </Show>
                        </div>
                    )}
                </Match>
                <Match when={props.part.name === "bash"}>
                    <ShellOutput
                        command={shellCommand()}
                        output={shell().output}
                        file={toolMeta(props.part)?.outputFile as string | undefined}
                        running={state().status === "pending" || state().status === "running"}
                    />
                    {/* A call that went on in the background shows how that process is, not the notes written for the model. */}
                    <Show
                        when={backgrounded()}
                        fallback={
                            <For each={shell().notes}>
                                {(note) => (
                                    <div class="mt-1 flex items-start gap-1.5 px-1 text-xs text-ink-faint">
                                        <IconInfo class="mt-0.5 size-3 shrink-0" />
                                        <span class="min-w-0 whitespace-pre-wrap">{note}</span>
                                    </div>
                                )}
                            </For>
                        }
                    >
                        {(id) => <ShellProcessStatus sessionId={props.part.sessionId} processId={id()} />}
                    </Show>
                </Match>
                <Match when={patched().length > 1 && patched()}>{(files) => <PatchPanel files={files()} />}</Match>
                <Match when={readImage(props.part)}>
                    {(image) => <ReadImage hash={image().hash} path={image().path} />}
                </Match>
                <Match when={readPage(props.part)}>
                    {(page) => <ReadPanel path={page().path} lines={page().lines} rest={page().rest} />}
                </Match>
                <Match when={props.diff ?? written()}>
                    {(patch) => <DiffPanel diff={patch()} filename={diffFilename()} />}
                </Match>
            </Switch>
            <Show when={props.error}>
                {(message) => (
                    <div class="tool-card tool-card-error mt-1.5 text-[length:calc(0.85rem*var(--ui-text-scale,1))] break-words whitespace-pre-wrap text-ink-muted">
                        {clip(stripAnsi(message()))}
                    </div>
                )}
            </Show>
        </>
    );
}

export function taskBody(part: ToolPart) {
    if (part.name !== "task" && part.name !== "spawn_thread") return null;

    const input = toolInput(part) as { prompt?: string; task?: string };
    const output = part.status === "done" ? (part.output ?? "") : "";
    const result = output.match(/<task_result>\n?([\s\S]*?)\n?<\/task_result>/)?.[1] ?? output;

    const prompt = input.prompt ?? input.task ?? "";
    // The engine tells the model how to continue the subagent; the card shows the subagent's own words.
    const words = result.replace(/\n\n\(task_id: [^)]*\)$/, "");

    return { prompt, result: words };
}

function GenericBody(props: { part: ToolPart }) {
    const engine = useEngine();
    const state = () => toolDisplay(props.part);
    const fields = createMemo(() => inputFields(state().input));
    const output = createMemo(() => {
        const current = state();
        return current.status === "completed" ? outputView(stripAnsi(current.output)) : null;
    });
    const images = createMemo(() => returnedImages(props.part));
    const returned = () => !!output()?.text || images().length > 0;
    const citationFiles = () =>
        citationFileGroups(engine.state, props.part.sessionId, props.part.messageId, props.part.id);

    return (
        <Show when={fields().length > 0 || returned()}>
            <div class="tool-card tool-io space-y-2.5 text-[length:calc(0.85rem*var(--ui-text-scale,1))]">
                <Show when={fields().length > 0}>
                    <section>
                        <div class="mb-1 text-xs font-medium text-ink-faint">{t("drift.tool.input")}</div>
                        <dl class="grid grid-cols-[minmax(5rem,max-content)_minmax(0,1fr)] gap-x-6 gap-y-1">
                            <For each={fields()}>
                                {(field) => (
                                    <>
                                        <dt class="text-ink-faint">{field.label}</dt>
                                        <dd
                                            class="max-h-40 min-w-0 overflow-auto break-words whitespace-pre-wrap text-ink-muted"
                                            classList={{ "font-mono text-xs": field.structured }}
                                        >
                                            {clip(field.text)}
                                        </dd>
                                    </>
                                )}
                            </For>
                        </dl>
                    </section>
                </Show>
                <Show when={returned()}>
                    <section classList={{ "border-t-[0.5px] border-edge pt-2.5": fields().length > 0 }}>
                        <div class="mb-1 text-xs font-medium text-ink-faint">{t("drift.tool.output")}</div>
                        <Show when={output()?.text ? output() : null}>
                            {(view) => (
                                <div class="transcript-tool-output max-h-80 overflow-auto text-ink-muted">
                                    <Show
                                        when={view().kind === "markdown"}
                                        fallback={
                                            <div class="font-mono text-xs break-words whitespace-pre-wrap">
                                                {clip(view().text, maxReturnedChars)}
                                            </div>
                                        }
                                    >
                                        <Markdown
                                            text={clip(view().text, maxReturnedChars)}
                                            directory={engine.state.sessions[props.part.sessionId]?.directory}
                                            fileGroups={citationFiles}
                                            done
                                        />
                                    </Show>
                                </div>
                            )}
                        </Show>
                        <Show when={images().length > 0}>
                            <div class="mt-2 flex flex-wrap gap-2">
                                <For each={images()}>{(hash) => <ToolImage hash={hash} />}</For>
                            </div>
                        </Show>
                    </section>
                </Show>
            </div>
        </Show>
    );
}

/** A new file's content as a unified diff that adds every line. */
export function addedLines(content: string) {
    const lines = content.replace(/\n$/, "").split("\n");
    return [`@@ -0,0 +1,${lines.length} @@`, ...lines.map((line) => `+${line}`)].join("\n");
}

function clip(value: string, max = maxInlineOutputChars) {
    return value.length > max ? value.slice(0, max) + "\n..." : value;
}
