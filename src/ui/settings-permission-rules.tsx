import { IconArrowDown, IconArrowUp, IconPlus, IconTrash } from "./icons";
import { For, Show } from "solid-js";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { PermissionRule } from "../engine/native/client";
import type { JSX } from "solid-js";

/** Permission kinds requested by tools; `*` matches every kind. */
const permissionKinds = [
    "*",
    "bash",
    "edit",
    "read",
    "glob",
    "grep",
    "webfetch",
    "mcp",
    "skill",
    "task",
    "project-commands",
] as const;
const decisions = ["allow", "ask", "deny"] as const;

/** Returns a list with the selected rule moved by the requested offset, clamped to the list. */
export function moveRule(rules: PermissionRule[], index: number, offset: number) {
    const destination = Math.min(Math.max(index + offset, 0), rules.length - 1);
    const next = [...rules];
    next.splice(destination, 0, ...next.splice(index, 1));

    return next;
}

export const newRule = (): PermissionRule => ({ kind: "bash", pattern: "", decision: "ask" });

/** Edits rule kinds, patterns, decisions, and list order. */
export function RuleList(props: { rules: PermissionRule[]; onChange: (rules: PermissionRule[]) => void }) {
    const update = (index: number, change: Partial<PermissionRule>) => {
        const next = props.rules.map((rule, position) => (position === index ? { ...rule, ...change } : rule));

        props.onChange(next);
    };

    return (
        <Show
            when={props.rules.length > 0}
            fallback={<div class="px-1 py-3 text-xs text-ink-faint">{t("drift.permissions.empty")}</div>}
        >
            <div>
                <For each={props.rules}>
                    {(rule, index) => (
                        <div class="flex items-center gap-2 border-b border-edge/70 px-1 py-2">
                            <Picker
                                label={t("drift.permissions.kind")}
                                items={permissionKinds.map((kind) => ({
                                    id: kind,
                                    label: kind === "*" ? t("drift.permissions.kind.all") : kind,
                                }))}
                                selected={rule.kind}
                                fallbackLabel={rule.kind}
                                floating
                                bordered
                                chevronAtEnd
                                placement="below"
                                width="9.5rem"
                                onPick={(kind) => update(index(), { kind })}
                            />
                            <input
                                aria-label={t("drift.permissions.pattern")}
                                class="h-8 min-w-0 flex-1 rounded-md border border-edge bg-raised/45 px-2.5 font-mono text-xs text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
                                placeholder="git push*"
                                value={rule.pattern}
                                onInput={(event) => update(index(), { pattern: event.currentTarget.value })}
                            />
                            <Picker
                                label={t("drift.permissions.decision")}
                                items={decisions.map((decision) => ({
                                    id: decision,
                                    label: t(`drift.permissions.decision.${decision}`),
                                }))}
                                selected={rule.decision}
                                floating
                                bordered
                                chevronAtEnd
                                placement="below"
                                width="6.5rem"
                                onPick={(decision) =>
                                    update(index(), { decision: decision as PermissionRule["decision"] })
                                }
                            />
                            <RowButton
                                title={t("drift.permissions.moveUp")}
                                disabled={index() === 0}
                                onClick={() => props.onChange(moveRule(props.rules, index(), -1))}
                            >
                                <IconArrowUp class="size-3.5" />
                            </RowButton>
                            <RowButton
                                title={t("drift.permissions.moveDown")}
                                disabled={index() === props.rules.length - 1}
                                onClick={() => props.onChange(moveRule(props.rules, index(), 1))}
                            >
                                <IconArrowDown class="size-3.5" />
                            </RowButton>
                            <RowButton
                                title={t("drift.permissions.remove")}
                                onClick={() =>
                                    props.onChange(props.rules.filter((_, position) => position !== index()))
                                }
                            >
                                <IconTrash class="size-3.5" />
                            </RowButton>
                        </div>
                    )}
                </For>
            </div>
        </Show>
    );
}

export function AddRule(props: { disabled?: boolean; onAdd: () => void }) {
    return (
        <button
            class="flex h-8 items-center gap-1.5 rounded-md border border-edge px-2.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
            disabled={props.disabled}
            onClick={() => props.onAdd()}
        >
            <IconPlus class="size-3.5" />
            {t("drift.permissions.add")}
        </button>
    );
}

function RowButton(props: { title: string; disabled?: boolean; onClick: () => void; children: JSX.Element }) {
    return (
        <button
            type="button"
            title={props.title}
            aria-label={props.title}
            disabled={props.disabled}
            class="flex size-8 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-30"
            onClick={() => props.onClick()}
        >
            {props.children}
        </button>
    );
}
