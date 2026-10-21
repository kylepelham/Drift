import { NoteBox } from "./note-box";
import { For, Show } from "solid-js";
import { IconInfo } from "./icons";
import { t } from "../state/i18n";

import type { Part } from "../engine/parts";

export type InstructionUpdatePart = Extract<Part, { type: "instruction_update" }>;

const named = ["agent", "skills", "subagents", "deferred", "references", "environment", "model", "date"];

/** A source by its key, in the user's language: an MCP server or instruction file by its own name. */
export function instructionSource(key: string) {
    const [kind, name] = key.split(/:(.*)/s, 2);
    if (kind === "base") return t("drift.instructions.source.base");
    if ((kind === "mcp" || kind === "file") && name) return name;
    return named.includes(key) ? t(`drift.instructions.source.${key}`) : key;
}

/** The engine telling the model that some of its instructions changed: the sources folded, their new text inside. */
export function InstructionUpdateRow(props: { part: InstructionUpdatePart }) {
    const preview = () => props.part.changes.map((change) => instructionSource(change.key)).join(", ");

    return (
        <div class="flex min-w-0 justify-end">
            <NoteBox icon={<IconInfo class="size-3.5" />} label={t("drift.instructions.changed")} preview={preview()}>
                <For each={props.part.changes}>
                    {(change) => (
                        <div class="space-y-1">
                            <div class="text-xs text-ink-faint">{instructionSource(change.key)}</div>
                            <Show
                                when={change.text}
                                fallback={<div class="text-xs text-ink-muted">{t("drift.instructions.removed")}</div>}
                            >
                                {(text) => (
                                    <pre class="max-h-60 overflow-auto font-mono text-xs whitespace-pre-wrap break-words text-ink-muted">
                                        {text()}
                                    </pre>
                                )}
                            </Show>
                        </div>
                    )}
                </For>
            </NoteBox>
        </div>
    );
}

export type ContinuationPart = Extract<Part, { type: "continuation" }>;

/** The engine's note that Drift restarted mid-turn, folded on the prompt that resumed it. */
export function ResumedRow(props: { part: ContinuationPart }) {
    return (
        <div class="flex min-w-0 justify-end">
            <NoteBox icon={<IconInfo class="size-3.5" />} label={t("drift.turn.resumed")}>
                <div class="text-xs whitespace-pre-wrap break-words text-ink-muted">{props.part.text}</div>
            </NoteBox>
        </div>
    );
}
