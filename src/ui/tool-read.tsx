import { createDiffHighlight, SyntaxTokenView } from "./diff-panel";
import { toolInput, type ToolPart } from "../engine/parts";
import { createMemo, For, Show } from "solid-js";
import { diffWordWrap } from "../state/code";

type ReadLine = { line: number; text: string };

/** A finished text read as its numbered lines and whatever the engine wrote after them (paging, reminders). */
export function readPage(part: ToolPart) {
    if (part.name !== "read" || part.status !== "done" || !part.output) return null;
    const input = toolInput(part) as { path?: unknown; filePath?: unknown };
    const path = [input.path, input.filePath].find((value): value is string => typeof value === "string" && !!value);
    if (!path) return null;

    const page = parseReadOutput(part.output);
    return page.lines.length ? { path, ...page } : null;
}

export function parseReadOutput(output: string) {
    const rows = output.split("\n");
    const lines: ReadLine[] = [];
    for (const row of rows) {
        const numbered = /^(\d+): ?(.*)$/.exec(row);
        if (!numbered) break;
        lines.push({ line: Number(numbered[1]), text: numbered[2]! });
    }

    return { lines, rest: rows.slice(lines.length).join("\n").trim() };
}

export function ReadPanel(props: { path: string; lines: ReadLine[]; rest: string }) {
    const code = createMemo(() => props.lines.map((row) => row.text).join("\n"));
    const tokens = createDiffHighlight(() => props.path, code);

    return (
        <div class="space-y-1.5">
            <div class="overflow-hidden rounded-lg border border-edge">
                <div class="transcript-tool-output max-h-80 overflow-auto py-1 font-mono leading-relaxed">
                    <div classList={{ "w-full": diffWordWrap(), "w-max min-w-full": !diffWordWrap() }}>
                        <For each={props.lines}>
                            {(row, index) => (
                                <div class="flex">
                                    <span class="min-w-10 shrink-0 pr-3 pl-2 text-right text-ink-faint select-none">
                                        {row.line}
                                    </span>
                                    <span
                                        class="min-w-0 flex-1 pr-3 text-ink-muted"
                                        classList={{
                                            "whitespace-pre-wrap [overflow-wrap:anywhere]": diffWordWrap(),
                                            "whitespace-pre": !diffWordWrap(),
                                        }}
                                    >
                                        <Show
                                            when={tokens()[index()]?.length ? tokens()[index()] : undefined}
                                            fallback={row.text}
                                        >
                                            {(line) => (
                                                <For each={line()}>{(token) => <SyntaxTokenView token={token} />}</For>
                                            )}
                                        </Show>
                                    </span>
                                </div>
                            )}
                        </For>
                    </div>
                </div>
            </div>
            <Show when={props.rest}>
                <div class="px-1 text-xs whitespace-pre-wrap text-ink-faint">{props.rest}</div>
            </Show>
        </div>
    );
}
