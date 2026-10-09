import { t } from "../state/i18n";
import { Show } from "solid-js";

import type { JSX } from "solid-js";

/** Separates each prompt group from the preceding group. */
export function ListGroup(props: { title: string; first?: boolean; children: JSX.Element }) {
    return (
        <div classList={{ "mt-4 border-t border-edge pt-4": !props.first }}>
            <div class="mb-2 px-2 text-[0.68rem] font-semibold tracking-wider text-ink-muted uppercase">
                {props.title}
            </div>
            <div class="space-y-0.5">{props.children}</div>
        </div>
    );
}

export function ListItem(props: {
    label: string;
    active: boolean;
    customized: boolean;
    unsaved: boolean;
    problem?: boolean;
    onSelect: () => void;
}) {
    return (
        <button
            type="button"
            class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[0.82rem] outline-none transition-colors focus-visible:bg-raised/60"
            classList={{
                "bg-raised text-ink": props.active,
                "text-ink-muted hover:bg-raised/60 hover:text-ink": !props.active,
            }}
            aria-current={props.active ? "true" : undefined}
            onClick={() => props.onSelect()}
        >
            <span class="min-w-0 flex-1 truncate">{props.label}</span>
            <Show when={props.problem}>
                <span class="size-1.5 shrink-0 rounded-full bg-danger" />
            </Show>
            <Show when={props.unsaved}>
                <span class="shrink-0 text-[0.65rem] text-warn" title={t("drift.settings.prompts.unsaved")}>
                    {t("drift.settings.prompts.unsavedShort")}
                </span>
            </Show>
            <Show when={props.customized && !props.unsaved}>
                <span class="size-1.5 shrink-0 rounded-full bg-accent" title={t("drift.settings.prompts.customized")} />
            </Show>
        </button>
    );
}
