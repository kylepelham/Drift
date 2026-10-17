import { closeReview, openReview, reviewPanel, type ReviewPanel, type ReviewSource } from "../state/panels";
import { EngineError, type Review, type ReviewFile } from "../engine/native/client";
import { samePath, sourceFromId, sourceId, sourceItems } from "./review-sources";
import { diffIndicator, diffLineNumbers, diffWordWrap } from "../state/code";
import { createMediaQuery, smallScreenQuery } from "../state/screen";
import { createDiffHighlight, SyntaxTokenView } from "./diff-panel";
import { IconChevronLeft, IconRestore, IconX } from "./icons";
import { reviewRows, type ReviewRow } from "./review-rows";
import { nativeFrame, WindowControls } from "./titlebar";
import { activeWorkspace } from "../state/workspaces";
import { selectedSession } from "../state/selection";
import { persisted } from "../state/persist";
import { FileIcon } from "./file-icon";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { Picker } from "./picker";
import { t } from "../state/i18n";
import {
    createEffect,
    createMemo,
    createResource,
    createSignal,
    For,
    Match,
    on,
    Show,
    Switch,
    untrack,
} from "solid-js";

const minWidth = 360;
const defaultWidth = 640;
/** Files with more merged lines than this start folded, and are never syntax highlighted. */
const largeFileLines = 4000;

const [panelWidth, setPanelWidth] = persisted<number>("drift.review.width", defaultWidth);

/** The changes of a turn or of the workspace against git, right of the chat; full screen on a phone. */
export function ReviewPanelHost() {
    return <Show when={reviewPanel()}>{(panel) => <ReviewPanelView panel={panel()} />}</Show>;
}

function ReviewPanelView(props: { panel: ReviewPanel }) {
    const engine = useEngine();
    const narrow = createMediaQuery(smallScreenQuery);
    const [refresh, setRefresh] = createSignal(0);
    const turn = () => (props.panel.turn?.sessionId === selectedSession() ? props.panel.turn : undefined);

    const [refs] = createResource(
        () => ({ workspace: activeWorkspace()?.path, refresh: refresh() }),
        () => engine.actions.reviewRefs(),
    );
    const [review] = createResource(
        () => ({ source: props.panel.source, refresh: refresh() }),
        ({ source }) => loadReview(engine.actions, source),
    );

    const pick = (id: string) => {
        const source = sourceFromId(id, turn());
        if (source) openReview({ source, turn: turn() });
    };

    return (
        <div
            class="review-panel relative flex min-h-0 min-w-0 shrink-0 flex-col overflow-hidden border-l border-edge bg-bg"
            style={narrow() ? undefined : { width: `${panelWidth()}px` }}
        >
            <Show when={!narrow()}>
                <ResizeHandle />
            </Show>
            <div
                class="review-header flex h-11 shrink-0 items-center gap-1 border-b border-edge/60 pl-2"
                classList={{ "pr-2": !nativeFrame() || narrow() }}
                data-tauri-drag-region={nativeFrame() ? "" : undefined}
            >
                <Show when={narrow()}>
                    <button
                        class="flex size-11 shrink-0 items-center justify-center rounded-md text-ink-muted hover:bg-raised hover:text-ink"
                        title={t("common.goBack")}
                        onClick={closeReview}
                    >
                        <IconChevronLeft />
                    </button>
                </Show>
                <Picker
                    items={sourceItems(refs(), !!turn())}
                    selected={sourceId(props.panel.source)}
                    fallbackLabel={sourceLabel(props.panel.source)}
                    label={t("drift.review.source")}
                    floating
                    placement="below"
                    onPick={pick}
                />
                <div class="min-w-2 flex-1 self-stretch" data-tauri-drag-region={nativeFrame() ? "" : undefined} />
                <button
                    class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                    title={t("drift.review.refresh")}
                    onClick={() => setRefresh(refresh() + 1)}
                >
                    <IconRestore class="size-3.5" />
                </button>
                <Show when={!narrow()}>
                    <button
                        class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                        title={t("common.close")}
                        onClick={closeReview}
                    >
                        <IconX class="size-3.5" />
                    </button>
                    <WindowControls />
                </Show>
            </div>
            <div class="review-scroll min-h-0 flex-1 overflow-y-auto overscroll-contain">
                <Show when={props.panel.source.kind === "turn"}>
                    <p class="px-4 pt-3 text-xs text-ink-faint">{t("drift.review.turnNote")}</p>
                </Show>
                <ReviewBody
                    review={review.error ? undefined : review()}
                    loading={review.loading}
                    error={review.error ? errorText(review.error) : undefined}
                    focus={props.panel.focus}
                />
            </div>
        </div>
    );
}

type ReviewActions = ReturnType<typeof useEngine>["actions"];

async function loadReview(actions: ReviewActions, source: ReviewSource): Promise<Review | undefined> {
    if (source.kind === "turn") return actions.reviewTurn(source.sessionId, source.messageId);
    if (source.kind === "uncommitted") return actions.reviewWorkspace("uncommitted");

    return actions.reviewWorkspace(source.kind, source.target);
}

function sourceLabel(source: ReviewSource) {
    if (source.kind === "turn") return t("drift.review.source.turn");
    if (source.kind === "uncommitted") return t("drift.review.source.uncommitted");

    return source.kind === "commit" ? source.target.slice(0, 7) : source.target;
}

function errorText(error: unknown) {
    if (error instanceof EngineError && error.code === "not_a_repo") return t("drift.review.notRepo");

    return error instanceof Error ? error.message : String(error);
}

function ResizeHandle() {
    const start = (event: PointerEvent) => {
        event.preventDefault();
        const handle = event.currentTarget as HTMLElement;
        handle.setPointerCapture(event.pointerId);
        const origin = event.clientX;
        const initial = panelWidth();
        const move = (moved: PointerEvent) => {
            const max = Math.max(minWidth, window.innerWidth * 0.75);
            setPanelWidth(Math.round(Math.min(max, Math.max(minWidth, initial + origin - moved.clientX))));
        };
        const stop = () => {
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", stop);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", stop);
    };

    return (
        <div
            class="review-resize absolute inset-y-0 left-0 z-10 w-1.5 cursor-col-resize transition-colors hover:bg-accent/30"
            role="separator"
            aria-orientation="vertical"
            onPointerDown={start}
        />
    );
}

function ReviewBody(props: { review?: Review; loading: boolean; error?: string; focus?: string }) {
    let list!: HTMLDivElement;

    createEffect(
        on(
            () => [props.review, props.focus] as const,
            ([review, focus]) => {
                if (!review || !focus) return;
                const target = [...list.querySelectorAll<HTMLElement>("[data-review-path]")].find((element) =>
                    samePath(element.dataset.reviewPath ?? "", focus),
                );
                queueMicrotask(() => target?.scrollIntoView({ block: "start" }));
            },
        ),
    );

    return (
        <div ref={list} class="flex flex-col gap-2 p-3">
            <Switch>
                <Match when={props.error}>{(error) => <p class="px-1 text-sm text-ink-faint">{error()}</p>}</Match>
                <Match when={!props.review && props.loading}>
                    <p class="px-1 text-sm text-ink-faint">{t("drift.review.loading")}</p>
                </Match>
                <Match when={props.review && !props.review.files.length}>
                    <p class="px-1 text-sm text-ink-faint">{t("drift.review.empty")}</p>
                </Match>
                <Match when={props.review}>
                    {(review) => (
                        <>
                            <For each={review().files}>{(file) => <FileDiff file={file} />}</For>
                            <Show when={review().omitted}>
                                {(count) => (
                                    <p class="px-1 text-xs text-ink-faint">
                                        {t("drift.review.omitted", { count: count() })}
                                    </p>
                                )}
                            </Show>
                        </>
                    )}
                </Match>
            </Switch>
        </div>
    );
}

function splitPath(path: string) {
    const normalized = path.replaceAll("\\", "/");
    const slash = normalized.lastIndexOf("/");

    return { folder: normalized.slice(0, slash + 1), name: normalized.slice(slash + 1) };
}

function FileDiff(props: { file: ReviewFile }) {
    const [open, setOpen] = createSignal(untrack(() => props.file.lines.length <= largeFileLines));
    const parts = () => splitPath(props.file.path);

    return (
        <div class="review-file rounded-lg border border-edge/70" data-review-path={props.file.path}>
            <button
                class="review-file-header sticky top-0 z-[1] flex w-full min-w-0 items-center gap-2 rounded-t-lg bg-bg px-3 py-2 text-left text-sm transition-colors hover:bg-raised/35"
                classList={{ "rounded-b-lg": !open() }}
                aria-expanded={open()}
                title={props.file.path}
                onClick={() => setOpen(!open())}
            >
                <span class="flex shrink-0 text-ink-faint">
                    <Chevron open={open()} />
                </span>
                <FileIcon path={props.file.path} class="size-4" />
                <span class="min-w-0 truncate">
                    <span class="text-ink-faint">{parts().folder}</span>
                    <span class="text-ink">{parts().name}</span>
                </span>
                <Show when={props.file.status !== "modified"}>
                    <span class="shrink-0 text-xs text-ink-faint">
                        {t(props.file.status === "added" ? "drift.file.created" : "drift.file.deleted")}
                    </span>
                </Show>
                <span class="flex-1" />
                <span class="shrink-0 font-mono text-xs">
                    <span class="text-ok">+{props.file.additions}</span>{" "}
                    <span class="text-danger">-{props.file.deletions}</span>
                </span>
            </button>
            <Show when={open()}>
                <div class="border-t border-edge/70">
                    <Show
                        when={props.file.preview === "lines"}
                        fallback={<p class="px-3 py-2 text-xs text-ink-faint">{previewText(props.file.preview)}</p>}
                    >
                        <FileLines file={props.file} />
                    </Show>
                </div>
            </Show>
        </div>
    );
}

function previewText(preview: ReviewFile["preview"]) {
    if (preview === "binary") return t("drift.review.binary");
    if (preview === "large") return t("drift.review.large");

    return t("drift.review.unavailable");
}

function FileLines(props: { file: ReviewFile }) {
    const [opened, setOpened] = createSignal<ReadonlySet<number>>(new Set());
    const rows = createMemo(() => reviewRows(props.file.lines, opened()));
    const code = createMemo(() =>
        props.file.lines.length > largeFileLines ? "" : props.file.lines.map((line) => line.slice(1)).join("\n"),
    );
    const tokens = createDiffHighlight(() => props.file.path, code);
    const unfold = (start: number) => setOpened(new Set([...opened(), start]));

    return (
        <div class="overflow-x-auto py-1 font-mono text-xs leading-relaxed">
            <div classList={{ "w-full": diffWordWrap(), "w-max min-w-full": !diffWordWrap() }}>
                <For each={rows()}>
                    {(row) => (
                        <Show when={row.kind !== "fold" && row} fallback={<FoldBar row={row} onOpen={unfold} />}>
                            {(line) => <LineRow row={line()} tokens={tokens()[line().index]} />}
                        </Show>
                    )}
                </For>
            </div>
        </div>
    );
}

function FoldBar(props: { row: ReviewRow; onOpen: (start: number) => void }) {
    const fold = () => props.row as Extract<ReviewRow, { kind: "fold" }>;

    return (
        <button
            class="review-fold my-0.5 flex w-full items-center gap-2 bg-raised/40 px-3 py-1 text-left font-sans text-xs text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            onClick={() => props.onOpen(fold().start)}
        >
            {t(fold().count === 1 ? "drift.review.unmodified.one" : "drift.review.unmodified.other", {
                count: fold().count,
            })}
        </button>
    );
}

function lineSymbol(kind: LineRowValue["kind"]) {
    if (kind === "add") return "+";
    if (kind === "del") return "-";

    return " ";
}

type LineRowValue = Exclude<ReviewRow, { kind: "fold" }>;

function LineRow(props: { row: LineRowValue; tokens?: ReturnType<ReturnType<typeof createDiffHighlight>>[number] }) {
    return (
        <div
            class="flex border-l-2"
            classList={{
                "bg-ok/10": diffIndicator() === "background" && props.row.kind === "add",
                "bg-danger/10": diffIndicator() === "background" && props.row.kind === "del",
                "border-l-ok": diffIndicator() === "bars" && props.row.kind === "add",
                "border-l-danger": diffIndicator() === "bars" && props.row.kind === "del",
                "border-l-transparent": diffIndicator() !== "bars" || props.row.kind === "ctx",
            }}
        >
            <Show when={diffLineNumbers()}>
                <span class="w-12 shrink-0 pr-2 text-right text-ink-faint select-none">{props.row.line}</span>
            </Show>
            <Show when={diffIndicator() === "symbols"}>
                <span
                    class="w-4 shrink-0 text-center text-ink-faint select-none"
                    classList={{ "text-ok": props.row.kind === "add", "text-danger": props.row.kind === "del" }}
                >
                    {lineSymbol(props.row.kind)}
                </span>
            </Show>
            <span
                class="min-w-0 flex-1 pr-3 text-ink-muted"
                classList={{
                    "whitespace-pre-wrap [overflow-wrap:anywhere]": diffWordWrap(),
                    "whitespace-pre": !diffWordWrap(),
                }}
            >
                <Show
                    when={props.tokens?.length ? props.tokens : undefined}
                    fallback={
                        <span
                            classList={{ "text-ok": props.row.kind === "add", "text-danger": props.row.kind === "del" }}
                        >
                            {props.row.text}
                        </span>
                    }
                >
                    {(line) => <For each={line()}>{(token) => <SyntaxTokenView token={token} />}</For>}
                </Show>
            </span>
        </div>
    );
}
