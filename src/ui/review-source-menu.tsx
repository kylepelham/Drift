import { defaultBase, matching, sameSource, sourceTitle } from "./review-sources";
import { IconCheck, IconChevronLeft, IconChevronRight } from "./icons";
import { createSignal, For, Show, type JSX } from "solid-js";
import { createDismissOnOutside } from "./dismiss";
import { t } from "../state/i18n";

import type { ReviewRefs } from "../engine/native/client";
import type { ReviewSource } from "../state/panels";

type View = "main" | "branches" | "commits";

/** The review's source: this turn, all changes since the main line, uncommitted ones, another branch, or one commit. */
export function ReviewSourceMenu(props: {
    source: ReviewSource;
    refs?: ReviewRefs;
    turn?: Extract<ReviewSource, { kind: "turn" }>;
    onPick: (source: ReviewSource) => void;
}) {
    const [open, setOpen] = createSignal(false);
    const [view, setView] = createSignal<View>("main");
    let root!: HTMLDivElement;
    const base = () => defaultBase(props.refs);
    const title = () => sourceTitle(props.source, props.refs);
    const close = () => {
        setOpen(false);
        setView("main");
    };
    const pick = (source: ReviewSource) => {
        props.onPick(source);
        close();
    };
    createDismissOnOutside({ inside: () => [root], onDismiss: close, escape: true, enabled: open });

    return (
        <div ref={root} class="relative min-w-0">
            <button
                class="flex h-8 min-w-0 max-w-full items-center gap-1.5 rounded-lg px-2.5 text-[length:calc(0.82rem*var(--ui-text-scale,1))] text-ink transition-colors hover:bg-raised"
                classList={{ "bg-raised": open() }}
                aria-label={t("drift.review.source")}
                aria-expanded={open()}
                onClick={() => (open() ? close() : setOpen(true))}
            >
                <span class="min-w-0 truncate">{title().text}</span>
                <Show when={title().to}>
                    {(to) => (
                        <>
                            <span class="shrink-0 text-ink-faint">→</span>
                            <span class="min-w-0 truncate">{to()}</span>
                        </>
                    )}
                </Show>
                <span class="flex shrink-0 rotate-90 text-ink-faint">
                    <IconChevronRight class="size-3" />
                </span>
            </button>
            <Show when={open()}>
                <div class="absolute top-full left-0 z-30 mt-1 w-72 overflow-hidden rounded-xl border border-edge bg-overlay p-1 text-[length:calc(0.82rem*var(--ui-text-scale,1))] shadow-2xl shadow-black/40">
                    <Show when={view() === "main"}>
                        <MainView {...props} base={base()} onPick={pick} onView={setView} />
                    </Show>
                    <Show when={view() === "branches"}>
                        <ListView
                            title={t("drift.review.compareAgainst")}
                            placeholder={t("drift.review.searchBranches")}
                            items={(props.refs?.branches ?? []).map((name) => ({ id: name, label: name }))}
                            selected={props.source.kind === "branch" ? props.source.target : undefined}
                            onBack={() => setView("main")}
                            onPick={(target) => pick({ kind: "branch", target })}
                        />
                    </Show>
                    <Show when={view() === "commits"}>
                        <ListView
                            title={t("drift.review.commits")}
                            placeholder={t("drift.review.searchCommits")}
                            items={(props.refs?.commits ?? []).map((commit) => ({
                                id: commit.id,
                                label: commit.subject,
                                detail: commit.short,
                            }))}
                            selected={props.source.kind === "commit" ? props.source.target : undefined}
                            onBack={() => setView("main")}
                            onPick={(target) => pick({ kind: "commit", target })}
                        />
                    </Show>
                </div>
            </Show>
        </div>
    );
}

function MainView(props: {
    source: ReviewSource;
    refs?: ReviewRefs;
    turn?: Extract<ReviewSource, { kind: "turn" }>;
    base?: string;
    onPick: (source: ReviewSource) => void;
    onView: (view: View) => void;
}) {
    const repo = () => !!props.refs?.repo;
    const is = (source: ReviewSource) => sameSource(props.source, source);
    const compared = () => (props.source.kind === "branch" ? props.source.target : props.base);

    return (
        <>
            <Show when={props.turn}>
                {(turn) => (
                    <Row
                        label={t("drift.review.source.turn")}
                        checked={is(turn())}
                        onClick={() => props.onPick(turn())}
                    />
                )}
            </Show>
            <Show when={repo() && props.base}>
                {(base) => (
                    <Row
                        label={t("drift.review.allChanges")}
                        detail={t("drift.review.versus", { base: base() })}
                        checked={is({ kind: "branch", target: base() })}
                        onClick={() => props.onPick({ kind: "branch", target: base() })}
                    />
                )}
            </Show>
            <Show when={repo()}>
                <Row
                    label={t("drift.review.source.uncommitted")}
                    checked={is({ kind: "uncommitted" })}
                    onClick={() => props.onPick({ kind: "uncommitted" })}
                />
            </Show>
            <Show when={repo() && props.refs?.branches.length}>
                <Divider />
                <Row
                    label={t("drift.review.compareAgainst")}
                    detail={compared()}
                    next
                    onClick={() => props.onView("branches")}
                />
            </Show>
            <Show when={repo() && props.refs?.commits.length}>
                <Divider />
                <Row
                    label={t("drift.review.commits")}
                    detail={String(props.refs?.commits.length ?? 0)}
                    next
                    onClick={() => props.onView("commits")}
                />
            </Show>
            <Show when={!repo() && !props.turn}>
                <p class="px-2.5 py-2 text-xs text-ink-faint">{t("drift.review.notRepo")}</p>
            </Show>
        </>
    );
}

function ListView(props: {
    title: string;
    placeholder: string;
    items: { id: string; label: string; detail?: string }[];
    selected?: string;
    onBack: () => void;
    onPick: (id: string) => void;
}) {
    const [query, setQuery] = createSignal("");
    const shown = () => matching(props.items, query(), (item) => `${item.label} ${item.detail ?? ""}`);

    return (
        <>
            <button
                class="flex h-8 w-full items-center gap-1.5 rounded-md px-1.5 text-left text-ink-muted transition-colors hover:bg-raised hover:text-ink"
                onClick={() => props.onBack()}
            >
                <IconChevronLeft class="size-3.5" />
                {props.title}
            </button>
            <input
                ref={(input) => queueMicrotask(() => input.focus())}
                class="my-1 h-8 w-full rounded-md border border-edge bg-bg/60 px-2.5 text-[length:calc(0.82rem*var(--ui-text-scale,1))] text-ink outline-none placeholder:text-ink-faint focus:border-accent"
                placeholder={props.placeholder}
                value={query()}
                onInput={(event) => setQuery(event.currentTarget.value)}
                onKeyDown={(event) => {
                    if (event.key === "Enter" && shown()[0]) props.onPick(shown()[0]!.id);
                }}
            />
            <div class="max-h-72 overflow-y-auto overscroll-contain">
                <For
                    each={shown()}
                    fallback={<p class="px-2.5 py-2 text-xs text-ink-faint">{t("drift.review.noMatches")}</p>}
                >
                    {(item) => (
                        <Row
                            label={item.label}
                            detail={item.detail}
                            checked={item.id === props.selected}
                            onClick={() => props.onPick(item.id)}
                        />
                    )}
                </For>
            </div>
        </>
    );
}

function Row(props: { label: string; detail?: string; checked?: boolean; next?: boolean; onClick: () => void }) {
    return (
        <button
            class="flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2.5 text-left text-ink transition-colors hover:bg-raised"
            onClick={() => props.onClick()}
        >
            <span class="min-w-0 flex-1 truncate">{props.label}</span>
            <Show when={props.checked}>
                <IconCheck class="size-3.5 shrink-0 text-accent" />
            </Show>
            <Show when={props.detail}>
                {(detail) => <span class="max-w-32 shrink-0 truncate text-xs text-ink-faint">{detail()}</span>}
            </Show>
            <Show when={props.next}>
                <IconChevronRight class="size-3.5 shrink-0 text-ink-faint" />
            </Show>
        </button>
    );
}

function Divider(): JSX.Element {
    return <div class="mx-2 my-1 h-px bg-edge/70" />;
}
