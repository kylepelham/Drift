import { commentOn, removeReviewComment, reviewComments, saveReviewComment } from "../state/review-comments";
import { closeReview, openReview, reviewPanel, type ReviewPanel, type ReviewSource } from "../state/panels";
import { EngineError, type Review, type ReviewFile } from "../engine/native/client";
import { CommentButton, CommentEditor, SavedComment } from "./review-comment";
import { diffIndicator, diffLineNumbers, diffWordWrap } from "../state/code";
import { createDiffHighlight, SyntaxTokenView } from "./diff-panel";
import { reviewRows, type ReviewRow } from "./review-rows";
import { ReviewSourceMenu } from "./review-source-menu";
import { IconExternalLink, IconRestore } from "./icons";
import { activeWorkspace } from "../state/workspaces";
import { selectedSession } from "../state/selection";
import { samePath } from "./review-sources";
import { openFile } from "../tool-actions";
import { backendInvoke } from "../backend";
import { SidePanel } from "./side-panel";
import { FileIcon } from "./file-icon";
import { useEngine } from "../engine";
import { Chevron } from "./controls";
import { t } from "../state/i18n";
import {
    createEffect,
    createMemo,
    createResource,
    createSignal,
    For,
    Index,
    Match,
    on,
    onCleanup,
    onMount,
    Show,
    Switch,
    untrack,
} from "solid-js";
import {
    anchorFindings,
    createFindingsWalk,
    FindingNote,
    FindingsStepper,
    type AnchoredFinding,
} from "./review-findings";

/** Files with more merged lines than this start folded, and are never syntax highlighted. */
const largeFileLines = 4000;
/** A diff row's height (text-xs at leading-relaxed), so a file not yet built keeps its place in the scroll. */
const rowPx = 19.5;
/** How far outside the view a file's lines are built, so scrolling never meets an empty file. */
const buildAhead = "1200px 0px";

/** The changes of a turn or of the workspace against git, right of the chat; full screen on a phone. */
export function ReviewPanelHost() {
    return <Show when={reviewPanel()}>{(panel) => <ReviewPanelView panel={panel()} />}</Show>;
}

function ReviewPanelView(props: { panel: ReviewPanel }) {
    const engine = useEngine();
    const [refresh, setRefresh] = createSignal(0);
    const turn = () => (props.panel.turn?.sessionId === selectedSession() ? props.panel.turn : undefined);

    const [refs] = createResource(
        () => ({ workspace: activeWorkspace()?.path, refresh: refresh() }),
        ({ workspace }) => engine.actions.reviewRefs(workspace),
    );
    const [review] = createResource(
        () => ({ source: props.panel.source, workspace: activeWorkspace()?.path, refresh: refresh() }),
        ({ source, workspace }) => loadReview(engine.actions, source, workspace),
    );

    const [fold, setFold] = createSignal<Fold>();
    const pick = (source: ReviewSource) => openReview({ source, turn: turn(), findings: props.panel.findings });

    const walk = createFindingsWalk(() => props.panel.findings);
    const anchored = createMemo(() => anchorFindings(walk.findings(), review.error ? [] : (review()?.files ?? [])));
    const layer: FindingsLayer = {
        forFile: (path) => [...anchored().values()].filter((item) => item.path === path),
        current: () => walk.current()?.id,
        select: walk.select,
        claimSeek: walk.claimSeek,
    };

    return (
        <SidePanel
            name="review"
            defaultWidth={640}
            onClose={closeReview}
            title={<ReviewSourceMenu source={props.panel.source} refs={refs()} turn={turn()} onPick={pick} />}
            actions={
                <button
                    class="flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                    title={t("drift.review.refresh")}
                    onClick={() => setRefresh(refresh() + 1)}
                >
                    <IconRestore class="size-3.5" />
                </button>
            }
        >
            <Show when={!review.error && review()?.files.length ? review() : undefined}>
                {(loaded) => <ReviewSummary review={loaded()} onFold={setFold} />}
            </Show>
            <Show when={walk.findings().length}>
                <FindingsStepper
                    walk={walk}
                    anchored={anchored()}
                    onClose={() => openReview({ ...props.panel, findings: undefined })}
                />
            </Show>
            <div class="review-scroll min-h-0 flex-1 overflow-y-auto overscroll-contain select-text">
                <ReviewBody
                    review={review.error ? undefined : review()}
                    loading={review.loading}
                    error={review.error ? errorText(review.error) : undefined}
                    focus={props.panel.focus}
                    fold={fold()}
                    findings={layer}
                />
            </div>
        </SidePanel>
    );
}

type ReviewActions = ReturnType<typeof useEngine>["actions"];
/** Review findings on the panel's files: those on one file, the one being walked to, and how to pick another. */
type FindingsLayer = {
    forFile: (path: string) => AnchoredFinding[];
    current: () => string | undefined;
    select: (id: string) => void;
    claimSeek: () => boolean;
};
/** Collapse all or Expand all, numbered so pressing the same one twice still applies. */
type Fold = { open: boolean; at: number };

/** How many files changed and by how much, with one control to fold or open every file. */
function ReviewSummary(props: { review: Review; onFold: (fold: Fold) => void }) {
    const [open, setOpen] = createSignal(true);
    const total = createMemo(() =>
        props.review.files.reduce(
            (sum, file) => ({ additions: sum.additions + file.additions, deletions: sum.deletions + file.deletions }),
            { additions: 0, deletions: 0 },
        ),
    );
    const count = () => props.review.files.length + (props.review.omitted ?? 0);
    const toggle = () => {
        const next = !open();
        setOpen(next);
        props.onFold({ open: next, at: Date.now() });
    };

    return (
        <div class="flex h-9 shrink-0 items-center gap-3 border-b border-edge/60 px-4 text-xs">
            <span class="text-ink-muted">
                {t(count() === 1 ? "drift.review.files.one" : "drift.review.files.other", { count: count() })}
            </span>
            <span class="font-mono">
                <span class="text-ok">+{total().additions}</span> <span class="text-danger">-{total().deletions}</span>
            </span>
            <span class="flex-1" />
            <button
                class="rounded px-1.5 py-0.5 text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                onClick={toggle}
            >
                {t(open() ? "drift.review.collapseAll" : "drift.review.expandAll")}
            </button>
        </div>
    );
}

async function loadReview(
    actions: ReviewActions,
    source: ReviewSource,
    workspace?: string,
): Promise<Review | undefined> {
    if (source.kind === "turn") return actions.reviewTurn(source.sessionId, source.messageId);
    if (source.kind === "uncommitted") return actions.reviewWorkspace("uncommitted", undefined, workspace);

    return actions.reviewWorkspace(source.kind, source.target, workspace);
}

function errorText(error: unknown) {
    if (error instanceof EngineError && error.code === "not_a_repo") return t("drift.review.notRepo");

    return error instanceof Error ? error.message : String(error);
}

function ReviewBody(props: {
    review?: Review;
    loading: boolean;
    error?: string;
    focus?: string;
    fold?: Fold;
    findings: FindingsLayer;
}) {
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
        <div ref={list} class="flex flex-col">
            <Switch>
                <Match when={props.error}>{(error) => <p class="px-4 py-3 text-sm text-ink-faint">{error()}</p>}</Match>
                <Match when={!props.review && props.loading}>
                    <p class="px-4 py-3 text-sm text-ink-faint">{t("drift.review.loading")}</p>
                </Match>
                <Match when={props.review && !props.review.files.length}>
                    <p class="px-4 py-3 text-sm text-ink-faint">{t("drift.review.empty")}</p>
                </Match>
                <Match when={props.review}>
                    {(review) => (
                        <>
                            <For each={review().files}>
                                {(file) => <FileDiff file={file} fold={props.fold} findings={props.findings} />}
                            </For>
                            <Show when={review().omitted}>
                                {(count) => (
                                    <p class="px-4 py-3 text-xs text-ink-faint">
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

/** One file: a header that stays at the top while its lines scroll past, and its lines, built once near the view. */
function FileDiff(props: { file: ReviewFile; fold?: Fold; findings: FindingsLayer }) {
    const [open, setOpen] = createSignal(untrack(() => props.file.lines.length <= largeFileLines));
    const [near, setNear] = createSignal(false);
    const parts = () => splitPath(props.file.path);
    // Opening needs the desktop host, and a deleted file has nothing left to open.
    const openable = () => (backendInvoke() && props.file.status !== "deleted" ? props.file.location : undefined);
    const findings = createMemo(() => props.findings.forFile(props.file.path));
    // The finding being walked to opens and builds its file wherever it is.
    createEffect(() => {
        const current = props.findings.current();
        if (!findings().some((item) => item.finding.id === current)) return;
        setOpen(true);
        setNear(true);
    });
    createEffect(
        on(
            () => props.fold,
            (fold) => fold && setOpen(fold.open),
            { defer: true },
        ),
    );
    // Rows the file shows before anything is unfolded, for its height until it is built.
    const estimate = createMemo(() => reviewRows(props.file.lines, new Set()).length * rowPx);
    let root!: HTMLDivElement;
    onMount(() => {
        const observer = new IntersectionObserver(
            (entries) => {
                if (!entries.some((entry) => entry.isIntersecting)) return;
                setNear(true);
                observer.disconnect();
            },
            { root: root.closest(".review-scroll"), rootMargin: buildAhead },
        );
        observer.observe(root);
        onCleanup(() => observer.disconnect());
    });

    return (
        <div ref={root} class="review-file border-b border-edge/60" data-review-path={props.file.path}>
            <div
                class="review-file-header sticky top-0 z-[1] flex w-full min-w-0 items-center border-b border-transparent bg-bg text-[length:calc(0.82rem*var(--ui-text-scale,1))] transition-colors select-none hover:bg-raised"
                classList={{ "border-edge/60": open() }}
            >
                <button
                    class="flex min-w-0 flex-1 items-center gap-2 py-2 pl-4 text-left"
                    classList={{ "pr-4": !openable(), "pr-2": !!openable() }}
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
                <Show when={openable()}>
                    {(location) => (
                        <button
                            class="mr-2 flex size-7 shrink-0 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-overlay hover:text-ink"
                            title={t("drift.review.openFile")}
                            aria-label={t("drift.review.openFile")}
                            onClick={() => void openFile(location()).catch(() => undefined)}
                        >
                            <IconExternalLink class="size-3.5" />
                        </button>
                    )}
                </Show>
            </div>
            <Show when={open()}>
                <Show
                    when={props.file.preview === "lines"}
                    fallback={<p class="px-4 py-2 text-xs text-ink-faint">{previewText(props.file.preview)}</p>}
                >
                    <Show when={near()} fallback={<div style={{ height: `${estimate()}px` }} />}>
                        <FileLines file={props.file} findings={findings()} layer={props.findings} />
                    </Show>
                </Show>
            </Show>
        </div>
    );
}

function previewText(preview: ReviewFile["preview"]) {
    if (preview === "binary") return t("drift.review.binary");
    if (preview === "large") return t("drift.review.large");

    return t("drift.review.unavailable");
}

/** Rows built together; a chunk far from the view is torn down to its measured height, so the panel never holds a whole repository's lines. */
const chunkRows = 60;
/** How far outside the view a chunk stays built, so scrolling never meets an empty stretch. */
const keepBuilt = "1500px 0px";

type LineRowValue = Exclude<ReviewRow, { kind: "fold" }>;
type Writing = { index: number; id?: string };

function FileLines(props: { file: ReviewFile; findings: AnchoredFinding[]; layer: FindingsLayer }) {
    const [opened, setOpened] = createSignal<ReadonlySet<number>>(new Set());
    const findings = createMemo(() => {
        const found = new Map<number, AnchoredFinding[]>();
        for (const item of props.findings)
            found.set(item.anchor.index, [...(found.get(item.anchor.index) ?? []), item]);
        return found;
    });
    const rows = createMemo(() => reviewRows(props.file.lines, opened(), new Set(findings().keys())));
    const chunks = createMemo(() => {
        const all = rows();
        return Array.from({ length: Math.ceil(all.length / chunkRows) }, (_, at) =>
            all.slice(at * chunkRows, (at + 1) * chunkRows),
        );
    });
    const code = createMemo(() =>
        props.file.lines.length > largeFileLines ? "" : props.file.lines.map((line) => line.slice(1)).join("\n"),
    );
    const tokens = createDiffHighlight(() => props.file.path, code);
    const unfold = (start: number) => setOpened(new Set([...opened(), start]));
    // The row a note is being written on, and the note when it is an edit of one already saved.
    const [writing, setWriting] = createSignal<Writing>();
    // Each line's notes by its index, worked out once per change rather than by every row.
    const notes = createMemo(() => {
        const found = new Map<number, ReturnType<typeof reviewComments>>();
        const mine = reviewComments().filter((note) => note.path === props.file.path);
        if (!mine.length) return found;
        for (const row of rows()) {
            if (row.kind === "fold") continue;
            const on = mine.filter((note) => commentOn(note, props.file.path, row));
            if (on.length) found.set(row.index, on);
        }
        return found;
    });
    const save = (line: LineRowValue, text: string, id: string = crypto.randomUUID()) => {
        saveReviewComment({ id, path: props.file.path, line: line.line, side: line.kind, code: line.text, text });
        setWriting();
    };
    const lines: LinesContext = {
        tokens: () => tokens(),
        notes: () => notes(),
        findings,
        layer: () => props.layer,
        writing,
        setWriting,
        save,
        unfold,
    };

    return (
        <div class="review-lines @container overflow-x-auto font-mono text-xs leading-relaxed">
            <div classList={{ "w-full": diffWordWrap(), "w-max min-w-full": !diffWordWrap() }}>
                <Index each={chunks()}>{(chunk) => <LineChunk rows={chunk()} lines={lines} />}</Index>
            </div>
        </div>
    );
}

type LinesContext = {
    tokens: () => ReturnType<ReturnType<typeof createDiffHighlight>>;
    notes: () => Map<number, ReturnType<typeof reviewComments>>;
    findings: () => Map<number, AnchoredFinding[]>;
    layer: () => FindingsLayer;
    writing: () => Writing | undefined;
    setWriting: (writing?: Writing) => void;
    save: (line: LineRowValue, text: string, id?: string) => void;
    unfold: (start: number) => void;
};

/** A run of rows built only while it is near the view; away from it, an empty block of the height it had. */
function LineChunk(props: { rows: ReviewRow[]; lines: LinesContext }) {
    const [near, setNear] = createSignal(false);
    const [height, setHeight] = createSignal<number>();
    // A chunk holding the finding being walked to is built even far from the view, so its note can scroll to itself.
    const holdsCurrent = () => {
        const current = props.lines.layer().current();
        return (
            !!current &&
            props.rows.some(
                (row) =>
                    row.kind !== "fold" &&
                    (props.lines.findings().get(row.index) ?? []).some((item) => item.finding.id === current),
            )
        );
    };
    const built = () => near() || holdsCurrent();
    let root!: HTMLDivElement;
    onMount(() => {
        const observer = new IntersectionObserver(
            (entries) => {
                const entry = entries.at(-1);
                if (!entry) return;
                if (!entry.isIntersecting && near()) setHeight(root.offsetHeight);
                setNear(entry.isIntersecting);
            },
            { root: root.closest(".review-scroll"), rootMargin: keepBuilt },
        );
        observer.observe(root);
        onCleanup(() => observer.disconnect());
    });

    return (
        <div ref={root} style={built() ? undefined : { height: `${height() ?? props.rows.length * rowPx}px` }}>
            <Show when={built()}>
                <For each={props.rows}>
                    {(row) => (
                        <Show
                            when={row.kind !== "fold" && row}
                            fallback={<FoldBar row={row} onOpen={props.lines.unfold} />}
                        >
                            {(line) => <LineWithNotes line={line()} lines={props.lines} />}
                        </Show>
                    )}
                </For>
            </Show>
        </div>
    );
}

function LineWithNotes(props: { line: LineRowValue; lines: LinesContext }) {
    const notes = () => props.lines.notes().get(props.line.index) ?? [];
    const writing = () => props.lines.writing();

    return (
        <>
            <LineRow
                row={props.line}
                tokens={props.lines.tokens()[props.line.index]}
                onComment={() => props.lines.setWriting({ index: props.line.index })}
            />
            <For each={props.lines.findings().get(props.line.index) ?? []}>
                {(item) => (
                    <FindingNote
                        item={item}
                        current={props.lines.layer().current() === item.finding.id}
                        claimSeek={() => props.lines.layer().claimSeek()}
                        onSelect={() => props.lines.layer().select(item.finding.id)}
                    />
                )}
            </For>
            <For each={notes()}>
                {(note) => (
                    <Show
                        when={writing()?.id === note.id}
                        fallback={
                            <SavedComment
                                comment={note}
                                onEdit={() => props.lines.setWriting({ index: props.line.index, id: note.id })}
                                onRemove={() => removeReviewComment(note.id)}
                            />
                        }
                    >
                        <CommentEditor
                            side={note.side}
                            line={note.line}
                            initial={note.text}
                            onSave={(text) => props.lines.save(props.line, text, note.id)}
                            onCancel={() => props.lines.setWriting()}
                        />
                    </Show>
                )}
            </For>
            <Show when={writing()?.index === props.line.index && !writing()?.id}>
                <CommentEditor
                    side={props.line.kind}
                    line={props.line.line}
                    onSave={(text) => props.lines.save(props.line, text)}
                    onCancel={() => props.lines.setWriting()}
                />
            </Show>
        </>
    );
}

function FoldBar(props: { row: ReviewRow; onOpen: (start: number) => void }) {
    const fold = () => props.row as Extract<ReviewRow, { kind: "fold" }>;

    return (
        <button
            class="review-fold flex h-[19.5px] w-full items-center gap-2 bg-raised/30 pl-14 text-left font-sans text-[length:calc(0.7rem*var(--ui-text-scale,1))] text-ink-faint transition-colors hover:bg-raised hover:text-ink"
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

function LineRow(props: {
    row: LineRowValue;
    tokens?: ReturnType<ReturnType<typeof createDiffHighlight>>[number];
    onComment: () => void;
}) {
    return (
        <div
            class="group/line relative flex border-l-2"
            classList={{
                "bg-ok/10": diffIndicator() === "background" && props.row.kind === "add",
                "bg-danger/10": diffIndicator() === "background" && props.row.kind === "del",
                "border-l-ok": diffIndicator() === "bars" && props.row.kind === "add",
                "border-l-danger": diffIndicator() === "bars" && props.row.kind === "del",
                "border-l-transparent": diffIndicator() !== "bars" || props.row.kind === "ctx",
            }}
        >
            <CommentButton onClick={props.onComment} />
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
