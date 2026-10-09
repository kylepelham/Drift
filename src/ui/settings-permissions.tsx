import { createEffect, createMemo, createSignal, For, on, onMount, Show } from "solid-js";
import { AddRule, newRule, RuleList } from "./settings-permission-rules";
import { activeWorkspace, workspaces } from "../state/workspaces";
import { SettingsGroup } from "./settings-controls";
import { useEngine } from "../engine";
import { IconTrash } from "./icons";
import { t } from "../state/i18n";
import { Picker } from "./picker";

import type { PermissionGrant, PermissionRule } from "../engine/native/client";

/** What a grant covers, as the user approved it. */
export function grantLabel(grant: PermissionGrant) {
    if (grant.grant === "exact") return `${grant.kind}: ${grant.target}`;
    if (grant.grant === "subcommand")
        return `bash: ${t("drift.permissions.grant.subcommand", { prefix: grant.prefix })}`;
    if (grant.grant === "folder")
        return `${grant.kind}: ${t("drift.permissions.grant.folder", { folder: grant.folder })}`;
    return `${grant.kind}: ${grant.pattern}`;
}

export function PermissionsSection() {
    return (
        <div class="space-y-6">
            <RulesGroup />
            <GrantsGroup />
        </div>
    );
}

function RulesGroup() {
    const engine = useEngine();
    const [saved, setSaved] = createSignal<PermissionRule[]>([]);
    const [rules, setRules] = createSignal<PermissionRule[]>([]);
    const [error, setError] = createSignal("");
    const [notice, setNotice] = createSignal(false);
    const [busy, setBusy] = createSignal(false);
    const dirty = () => JSON.stringify(rules()) !== JSON.stringify(saved());

    async function run(action: () => Promise<PermissionRule[]>, announce: boolean) {
        setBusy(true);
        setError("");

        try {
            const next = await action();
            setSaved(next);
            setRules(next);
            setNotice(announce);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        } finally {
            setBusy(false);
        }
    }

    onMount(() => void run(() => engine.actions.permissionRules(), false));

    const actions = (
        <div class="flex gap-2">
            <Show when={dirty()}>
                <button
                    class="rounded-md border border-edge px-3 py-1 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink"
                    onClick={() => {
                        setError("");
                        setRules(saved());
                    }}
                >
                    {t("common.reset")}
                </button>
            </Show>
            <button
                class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink disabled:opacity-40"
                disabled={busy() || !dirty()}
                onClick={() => void run(() => engine.actions.savePermissionRules(rules()), true)}
            >
                {t("common.save")}
            </button>
        </div>
    );

    return (
        <SettingsGroup title={t("drift.permissions.rules")} action={actions}>
            <RuleList
                rules={rules()}
                onChange={(next) => {
                    setNotice(false);
                    setRules(next);
                }}
            />
            <div class="flex items-center gap-3 px-1 py-2.5">
                <AddRule disabled={busy()} onAdd={() => setRules((list) => [...list, newRule()])} />
                <Show when={error()}>
                    <div role="alert" class="text-xs text-danger">
                        {error()}
                    </div>
                </Show>
                <Show when={notice()}>
                    <div role="status" class="text-xs text-ok">
                        {t("drift.permissions.saved")}
                    </div>
                </Show>
            </div>
        </SettingsGroup>
    );
}

/** Groups grants by the access they allow rather than the tool that requested them. */
export function grantGroup(grant: PermissionGrant): "shell" | "files" | "web" | "mcp" | "other" {
    const kind = grant.grant === "subcommand" ? "bash" : grant.kind;
    if (kind === "bash") return "shell";
    if (["read", "edit", "glob", "grep"].includes(kind)) return "files";
    if (kind === "webfetch") return "web";
    if (kind === "mcp") return "mcp";
    return "other";
}

/** What a grant covers, without the kind its group already names. */
export function grantText(grant: PermissionGrant) {
    if (grant.grant === "exact") return grant.target;
    if (grant.grant === "subcommand") return t("drift.permissions.grant.subcommand", { prefix: grant.prefix });
    if (grant.grant === "folder") return t("drift.permissions.grant.folder", { folder: grant.folder });
    return grant.pattern;
}

const grantGroups = ["shell", "files", "web", "mcp", "other"] as const;
const filterFrom = 8;

function GrantsGroup() {
    const engine = useEngine();
    const [chosen, setChosen] = createSignal<string>();
    const [grants, setGrants] = createSignal<PermissionGrant[]>([]);
    const [filter, setFilter] = createSignal("");
    const [error, setError] = createSignal("");
    const [busy, setBusy] = createSignal(false);
    const workspace = () => workspaces().find((item) => item.id === chosen()) ?? activeWorkspace() ?? workspaces()[0];
    const directory = () => workspace()?.path;

    async function run(action: () => Promise<unknown>) {
        const folder = directory();
        if (!folder) return;

        setBusy(true);
        setError("");

        try {
            await action();
            setGrants(await engine.actions.workspaceGrants(folder));
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        } finally {
            setBusy(false);
        }
    }

    createEffect(
        on(directory, () => {
            setFilter("");
            void run(async () => undefined);
        }),
    );

    const shown = createMemo(() => {
        const words = filter().trim().toLowerCase();
        const matching = grants().filter((grant) => {
            if (!words) return true;

            const kind = grant.grant === "subcommand" ? "bash" : grant.kind;
            const text = `${kind} ${grantText(grant)}`.toLowerCase();

            return text.includes(words);
        });

        return grantGroups
            .map((group) => ({ group, grants: matching.filter((grant) => grantGroup(grant) === group) }))
            .filter((entry) => entry.grants.length);
    });

    const picker = (
        <Show when={workspaces().length}>
            <Picker
                label={t("drift.permissions.workspace")}
                items={workspaces().map((item) => ({ id: item.id, label: item.name, hint: item.path }))}
                selected={workspace()?.id}
                floating
                bordered
                chevronAtEnd
                placement="below"
                width="13rem"
                onPick={setChosen}
            />
        </Show>
    );

    return (
        <SettingsGroup title={t("drift.permissions.grants")} action={picker}>
            <Show when={workspace()} fallback={<Empty text={t("drift.permissions.noWorkspace")} />}>
                {(current) => (
                    <Show when={grants().length} fallback={<Empty text={t("drift.permissions.noGrants")} />}>
                        <div class="flex items-center justify-end gap-3 border-b border-edge/70 px-1 py-2.5">
                            <Show when={grants().length >= filterFrom}>
                                <input
                                    class="h-8 min-w-0 flex-1 rounded-md border border-edge bg-raised/45 px-2.5 text-xs text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
                                    placeholder={t("drift.permissions.filter")}
                                    aria-label={t("drift.permissions.filter")}
                                    value={filter()}
                                    onInput={(event) => setFilter(event.currentTarget.value)}
                                />
                            </Show>
                            <button
                                class="h-8 shrink-0 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-danger/50 hover:text-danger disabled:opacity-40"
                                disabled={busy()}
                                onClick={() => void run(() => engine.actions.revokeGrant(current().path))}
                            >
                                {t("drift.permissions.revokeAll")}
                            </button>
                        </div>
                        <For each={shown()}>
                            {(entry) => (
                                <>
                                    <div class="flex items-center gap-2 border-b border-edge/70 px-1 pt-4 pb-1.5 text-[0.78rem] font-medium text-ink">
                                        {t(`drift.permissions.group.${entry.group}`)}
                                        <span class="text-ink-faint">{entry.grants.length}</span>
                                    </div>
                                    <For each={entry.grants}>
                                        {(grant) => (
                                            <div class="group/grant flex min-h-9 items-center gap-3 border-b border-edge/70 px-1 py-1.5 last:border-b-0 hover:bg-raised/40">
                                                <Show when={entry.group === "other" || entry.group === "files"}>
                                                    <span class="w-16 shrink-0 text-[0.72rem] text-ink-faint">
                                                        {grant.grant === "subcommand" ? "bash" : grant.kind}
                                                    </span>
                                                </Show>
                                                <span
                                                    class="min-w-0 flex-1 truncate font-mono text-[0.74rem] text-ink-muted"
                                                    title={grantText(grant)}
                                                >
                                                    {grantText(grant)}
                                                </span>
                                                <button
                                                    type="button"
                                                    title={t("drift.permissions.revoke")}
                                                    aria-label={t("drift.permissions.revoke")}
                                                    class="flex size-6 shrink-0 items-center justify-center rounded text-ink-faint opacity-0 transition-opacity group-hover/grant:opacity-100 hover:bg-danger/10 hover:text-danger focus-visible:opacity-100 disabled:opacity-30"
                                                    disabled={busy()}
                                                    onClick={() =>
                                                        void run(() =>
                                                            engine.actions.revokeGrant(current().path, grant),
                                                        )
                                                    }
                                                >
                                                    <IconTrash class="size-3.5" />
                                                </button>
                                            </div>
                                        )}
                                    </For>
                                </>
                            )}
                        </For>
                        <Show when={!shown().length}>
                            <Empty text={t("drift.permissions.noMatch")} />
                        </Show>
                    </Show>
                )}
            </Show>
            <Show when={error()}>
                <div role="alert" class="px-1 py-2.5 text-xs text-danger">
                    {error()}
                </div>
            </Show>
        </SettingsGroup>
    );
}

function Empty(props: { text: string }) {
    return <div class="px-1 py-3 text-xs text-ink-faint">{props.text}</div>;
}
