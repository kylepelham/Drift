import { toolInput, toolMetadata as toolMeta } from "../engine/parts";
import { ShellOutput, splitNotes, stripAnsi } from "./shell-output";
import { patchFiles, toolFilename, toolInfo } from "./tool-labels";
import { createMemo, For, Match, Show, Switch } from "solid-js";
import { Markdown, ProgressiveCodeView } from "./markdown";
import { citationFileGroups } from "./citation-files";
import { DiffPanel, PatchPanel } from "./diff-panel";
import { delegatedChildId } from "./tool-delegation";
import { toolDisplay } from "./tool-presentation";
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

    const written = () => {
        if (props.part.name !== "write") return null;
        const input = state().input as { content?: string; filePath?: string };
        return typeof input.content === "string"
            ? { content: input.content, name: toolFilename(input.filePath) }
            : null;
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
        return child
            ? citationFileGroups(
                  engine.state,
                  child,
                  undefined,
                  undefined,
                  props.part.status === "done"
                      ? (props.part.finishedAt ?? props.part.startedAt ?? undefined)
                      : undefined,
              )
            : citationFileGroups(engine.state, props.part.sessionId, props.part.messageId, props.part.id);
    };

    return (
        <>
            <Switch fallback={<GenericBody part={props.part} />}>
                <Match when={tasked()}>
                    {(task) => (
                        <div class="space-y-2 border-l-2 border-edge pl-3">
                            <Show when={task().prompt}>
                                <div class="transcript-tool-output max-h-40 overflow-auto text-[0.85rem] whitespace-pre-wrap text-ink-faint">
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
                    <For each={shell().notes}>
                        {(note) => (
                            <div class="mt-1 flex items-start gap-1.5 px-1 text-xs text-ink-faint">
                                <IconInfo class="mt-0.5 size-3 shrink-0" />
                                <span class="min-w-0 whitespace-pre-wrap">{note}</span>
                            </div>
                        )}
                    </For>
                </Match>
                <Match when={written()}>
                    {(file) => <ProgressiveCodeView code={file().content} filename={file().name ?? ""} />}
                </Match>
                <Match when={patched().length > 1 && patched()}>{(files) => <PatchPanel files={files()} />}</Match>
                <Match when={props.diff}>{(patch) => <DiffPanel diff={patch()} filename={diffFilename()} />}</Match>
            </Switch>
            <Show when={props.error}>
                {(message) => (
                    <div class="mt-1.5 border-l-2 border-danger py-0.5 pl-3 text-[0.85rem] break-words whitespace-pre-wrap text-ink-muted">
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

    // The engine tells the model how to continue the subagent; the card shows the subagent's own words.
    return { prompt: input.prompt ?? input.task ?? "", result: result.replace(/\n\n\(task_id: [^)]*\)$/, "") };
}

function GenericBody(props: { part: ToolPart }) {
    const state = () => toolDisplay(props.part);
    const output = () => (state().status === "completed" ? (state() as { output: string }).output : "");
    const showInput = () => !!toolInfo(props.part).called;

    return (
        <div class="space-y-1.5 border-l-2 border-edge pl-3">
            <Show when={showInput()}>
                <div class="font-mono text-xs break-all whitespace-pre-wrap text-ink-faint">
                    {JSON.stringify(state().input, null, 1)}
                </div>
            </Show>
            <Show when={output().trim()}>
                <div class="transcript-tool-output max-h-64 overflow-auto text-[0.85rem] whitespace-pre-wrap text-ink-muted">
                    {clip(stripAnsi(output()))}
                </div>
            </Show>
        </div>
    );
}

function clip(value: string) {
    return value.length > maxInlineOutputChars ? value.slice(0, maxInlineOutputChars) + "\n..." : value;
}
