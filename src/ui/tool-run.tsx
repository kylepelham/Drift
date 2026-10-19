import { createMemo, createSignal, For, Show } from "solid-js";
import { toolDisplay } from "./tool-presentation";
import { CoveredDuration } from "./tool-duration";
import { TextShimmer } from "./text-shimmer";
import { toolInput } from "../engine/parts";
import { ToolView } from "./tool-view";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";
import {
    awaitingPermission,
    commandLabel,
    patchFiles,
    patchInputPaths,
    toolFilename,
    toolGroupKind,
    toolInfo,
} from "./tool-labels";

import type { ToolGroupKind } from "../state/prefs";
import type { EngineState } from "../engine/store";
import type { ToolPart } from "../engine/parts";

const labelOrder: ToolGroupKind[] = ["commands", "edits", "fetches", "mcp", "other"];

function active(part: ToolPart) {
    return part.status === "running" || part.status === "pending";
}

/** The files an edit call changed, for counting files rather than calls. */
function editedPaths(part: ToolPart) {
    const patched = patchFiles(part).map((file) => file.filePath);
    if (patched.length) return patched;

    const named = patchInputPaths(part);
    if (named.length) return named;

    const path = toolInput(part).filePath;
    return typeof path === "string" ? [path] : [part.id];
}

/** How many of each kind a run holds; edits count distinct files. */
function runCounts(parts: ToolPart[]) {
    const counts = new Map<ToolGroupKind, number>();
    const files = new Set<string>();
    for (const part of parts) {
        const kind = toolGroupKind(part.name);
        if (!kind) continue;
        if (kind === "edits") {
            editedPaths(part).forEach((path) => files.add(path));
            continue;
        }
        counts.set(kind, (counts.get(kind) ?? 0) + 1);
    }
    if (files.size) counts.set("edits", files.size);

    return labelOrder.filter((kind) => counts.has(kind)).map((kind) => ({ kind, count: counts.get(kind)! }));
}

/** `Ran 2 commands, edited 1 file`, or the same in the present while one of them runs. */
export function runSummary(parts: ToolPart[], running: boolean) {
    const tense = running ? "running" : "done";
    const phrases = runCounts(parts).map(({ kind, count }) =>
        t(`drift.run.${kind}.${tense}.${count === 1 ? "one" : "other"}`, { count }),
    );
    const text = phrases.join(", ");

    return text.charAt(0).toLocaleUpperCase() + text.slice(1);
}

const exploredKinds = { read: "read", grep: "search", glob: "search", list: "list" } as const;
type ExploredKind = (typeof exploredKinds)[keyof typeof exploredKinds];

function exploredTarget(part: ToolPart) {
    const input = toolInput(part);
    const text = (key: string) => (typeof input[key] === "string" ? (input[key] as string) : "");
    if (part.name === "read") return toolFilename(text("filePath")) ?? "";
    if (part.name === "list") return toolFilename(text("path")) || text("path");

    return text("pattern");
}

/** What a read or list touched, by full path, so two files that share a name count apart. */
function exploredKey(part: ToolPart) {
    const key = toolInput(part)[part.name === "read" ? "filePath" : "path"];

    return typeof key === "string" && key ? key : exploredTarget(part);
}

/** `Read 2 files, searched 3 patterns`; files and folders count once however often they were read. */
/** Each kind of exploring the run did, with the files, folders or searches it touched. */
function exploredTargets(parts: ToolPart[]) {
    const seen = new Map<ExploredKind, Set<string>>();
    for (const part of parts) {
        const kind = exploredKinds[part.name as keyof typeof exploredKinds];
        if (!kind) continue;
        const targets = seen.get(kind) ?? new Set<string>();
        targets.add(kind === "search" ? part.id : exploredKey(part) || part.id);
        seen.set(kind, targets);
    }

    return seen;
}

const capitalized = (text: string) => text.charAt(0).toLocaleUpperCase() + text.slice(1);

export function exploredSummary(parts: ToolPart[], running: boolean) {
    const seen = exploredTargets(parts);
    const tense = running ? "running" : "done";
    const text = (["read", "search", "list"] as const)
        .filter((kind) => seen.has(kind))
        .map((kind) => {
            const count = seen.get(kind)!.size;
            return t(`drift.explored.${kind}.${tense}.${count === 1 ? "one" : "other"}`, { count });
        })
        .join(", ");

    return capitalized(text);
}

/** A run that touched one file, folder or search names it: "Read drift.wit", split so the target keeps its style. */
export function exploredSingle(parts: ToolPart[], running: boolean) {
    const seen = exploredTargets(parts);
    const [kind, targets] = [...seen][0] ?? [];
    const target = exploredDetail(parts);
    if (seen.size !== 1 || !kind || targets?.size !== 1 || !target) return undefined;

    const marker = String.fromCharCode(0);
    const tense = running ? "running" : "done";
    const [before = "", after = ""] = t(`drift.explored.${kind}.${tense}.single`, { target: marker }).split(marker);

    return { before: capitalized(before.trim()), target, after: after.trim() };
}

/** The file or pattern being read now, else the latest one. */
export function exploredDetail(parts: ToolPart[]) {
    const current = [...parts].reverse().find(active) ?? parts.at(-1);
    return current ? exploredTarget(current) : "";
}

/** What the row adds after the summary while a call runs: that call; a finished run shows only its summary. */
export function runDetail(parts: ToolPart[]) {
    const current = [...parts].reverse().find(active);
    if (!current) return "";

    return current.name === "bash" ? commandLabel(current).text : (toolInfo(current).subtitle ?? "");
}

/** Commands of the run that went on in the background and still run. */
function stillRunning(state: EngineState, parts: ToolPart[]) {
    return parts.filter((part) => {
        const id = part.metadata?.processId;
        if (part.name !== "bash" || typeof id !== "string") return false;

        return state.processes[part.sessionId]?.some((process) => process.id === id && process.state === "running");
    }).length;
}

export function ToolRun(props: { parts: ToolPart[] }) {
    const engine = useEngine();
    const [open, setOpen] = createSignal(false);
    const current = () => [...props.parts].reverse().find(active);
    const waiting = () => props.parts.some((part) => awaitingPermission(engine.state, part));
    const failed = () => props.parts.some((part) => part.status === "error" || part.status === "denied");
    const running = () => !!current();
    const summary = createMemo(() => runSummary(props.parts, running()));
    const detail = createMemo(() => runDetail(props.parts));
    const background = () => stillRunning(engine.state, props.parts);
    const expanded = () => open() || waiting();

    return (
        <div class="tool-row flex flex-col gap-1.5 text-sm">
            <button
                class="tool-row-header flex min-h-7 min-w-0 items-center gap-2 overflow-hidden rounded-md px-1 text-left transition-colors hover:bg-raised/35"
                aria-expanded={expanded()}
                onClick={() => setOpen(!open())}
            >
                <Show when={waiting()}>
                    <span class="size-1.5 shrink-0 rounded-full bg-warn" title={t("notification.permission.title")} />
                </Show>
                <Show when={running()} fallback={<span class="tool-row-title shrink-0">{summary()}</span>}>
                    <TextShimmer text={summary()} class="tool-row-title shrink-0" />
                </Show>
                <Show when={detail()}>
                    <span class="tool-row-subtitle min-w-0 truncate">· {detail()}</span>
                </Show>
                <Show when={background()}>
                    {(count) => (
                        <span class="shrink-0 text-xs text-accent/80">
                            ·{" "}
                            {t(count() === 1 ? "drift.run.background.one" : "drift.run.background.other", {
                                count: count(),
                            })}
                        </span>
                    )}
                </Show>
                <Show when={failed()}>
                    <span class="shrink-0 text-xs text-danger">{t("drift.run.failed")}</span>
                </Show>
                <CoveredDuration states={props.parts.map(toolDisplay)} />
                <span class="tool-row-chevron flex shrink-0">
                    <Chevron open={expanded()} />
                </span>
            </button>
            <Show when={expanded()}>
                <div class="tool-children">
                    <For each={props.parts}>{(part) => <ToolView part={part} />}</For>
                </div>
            </Show>
        </div>
    );
}
