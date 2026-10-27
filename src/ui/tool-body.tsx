import { toolInput, toolMetadata as toolMeta } from "../engine/parts";
import { ShellOutput, splitNotes, stripAnsi } from "./shell-output";
import { createMemo, For, Match, Show, Switch } from "solid-js";
import { citationFileGroups } from "./citation-files";
import { patchFiles, toolInfo } from "./tool-labels";
import { DiffPanel, PatchPanel } from "./diff-panel";
import { delegatedChildId } from "./tool-delegation";
import { ReadImage, readImage } from "./tool-image";
import { ShellProcessStatus } from "./process-row";
import { ReadPanel, readPage } from "./tool-read";
import { toolDisplay } from "./tool-presentation";
import { Markdown } from "./markdown";
import { useEngine } from "../engine";
import { IconInfo } from "./icons";

import type { ToolPart } from "../engine/parts";

// Tool output beyond this is clipped before rendering; long outputs otherwise stall the view.
const maxInlineOutputChars = 4000;

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
    const state = () => toolDisplay(props.part);
    const output = () => (state().status === "completed" ? (state() as { output: string }).output : "");
    const showInput = () => !!toolInfo(props.part).called;

    return (
        <div class="tool-card space-y-1.5">
            <Show when={showInput()}>
                <div class="font-mono text-xs break-all whitespace-pre-wrap text-ink-faint">
                    {JSON.stringify(state().input, null, 1)}
                </div>
            </Show>
            <Show when={output().trim()}>
                <div class="transcript-tool-output max-h-64 overflow-auto text-[length:calc(0.85rem*var(--ui-text-scale,1))] whitespace-pre-wrap text-ink-muted">
                    {clip(stripAnsi(output()))}
                </div>
            </Show>
        </div>
    );
}

/** A new file's content as a unified diff that adds every line. */
export function addedLines(content: string) {
    const lines = content.replace(/\n$/, "").split("\n");
    return [`@@ -0,0 +1,${lines.length} @@`, ...lines.map((line) => `+${line}`)].join("\n");
}

function clip(value: string) {
    return value.length > maxInlineOutputChars ? value.slice(0, maxInlineOutputChars) + "\n..." : value;
}
