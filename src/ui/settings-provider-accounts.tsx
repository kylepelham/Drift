import { IconArrowDown, IconArrowUp, IconCheck, IconSquarePen, IconTrash } from "./icons";
import { createSignal, For, Show } from "solid-js";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { ProviderNotice } from "./settings-providers";
import type { ProviderAccount } from "../engine/store";

const iconButton =
    "flex size-7 shrink-0 items-center justify-center rounded-md border border-edge text-ink-muted transition-colors hover:text-ink disabled:opacity-40";

/** A provider's sign-ins, first used first; Drift moves on to the next when one reaches its usage limit. */
export function ProviderAccounts(props: {
    providerId: string;
    providerName: string;
    accounts: ProviderAccount[];
    onNotice: (notice: ProviderNotice) => void;
}) {
    const engine = useEngine();
    const [busy, setBusy] = createSignal(false);
    const [editing, setEditing] = createSignal<string | null>(null);
    const [draft, setDraft] = createSignal("");

    const name = (account: ProviderAccount, index: number) =>
        account.label ?? t("drift.provider.accounts.unnamed", { number: index + 1 });

    async function run(change: Promise<boolean>) {
        setBusy(true);
        const done = await change;
        setBusy(false);

        if (!done)
            props.onNotice({
                tone: "error",
                text: t("drift.provider.accounts.failed", { provider: props.providerName }),
            });
        return done;
    }

    function move(index: number, by: -1 | 1) {
        const order = props.accounts.map((account) => account.id);
        [order[index], order[index + by]] = [order[index + by], order[index]];

        void run(engine.actions.reorderProviderAccounts(props.providerId, order));
    }

    function startRename(account: ProviderAccount) {
        setDraft(account.label ?? "");
        setEditing(account.id);
    }

    async function rename(account: ProviderAccount) {
        if (await run(engine.actions.renameProviderAccount(props.providerId, account.id, draft().trim())))
            setEditing(null);
    }

    async function remove(account: ProviderAccount, index: number) {
        if (await run(engine.actions.removeProviderAccount(props.providerId, account.id)))
            props.onNotice({
                tone: "success",
                text: t("drift.provider.accounts.removed", { account: name(account, index) }),
            });
    }

    const row = (account: ProviderAccount, index: () => number) => (
        <div class="flex items-center gap-2 rounded-md border border-edge bg-overlay/35 px-2.5 py-1.5">
            <Show
                when={editing() === account.id}
                fallback={<span class="min-w-0 flex-1 truncate text-sm text-ink">{name(account, index())}</span>}
            >
                <input
                    class="h-7 min-w-0 flex-1 rounded-md border border-edge bg-overlay/50 px-2 text-sm outline-none focus:border-edge-strong"
                    placeholder={t("drift.provider.accounts.namePlaceholder")}
                    value={draft()}
                    onInput={(event) => setDraft(event.currentTarget.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter") void rename(account);
                        if (event.key === "Escape") setEditing(null);
                    }}
                    ref={(input) => queueMicrotask(() => input.focus())}
                />
            </Show>

            {/* The first account is the one requests go to. */}
            <Show when={index() === 0 && props.accounts.length > 1}>
                <span class="shrink-0 rounded-full border border-ok/35 bg-ok/10 px-2 py-0.5 text-[0.68rem] text-ok">
                    {t("drift.provider.accounts.inUse")}
                </span>
            </Show>

            <Show
                when={editing() === account.id}
                fallback={
                    <button
                        type="button"
                        class={iconButton}
                        title={t("drift.provider.accounts.rename")}
                        aria-label={t("drift.provider.accounts.rename")}
                        disabled={busy()}
                        onClick={() => startRename(account)}
                    >
                        <IconSquarePen class="size-3.5" />
                    </button>
                }
            >
                <button
                    type="button"
                    class={iconButton}
                    title={t("common.save")}
                    aria-label={t("common.save")}
                    disabled={busy()}
                    onClick={() => void rename(account)}
                >
                    <IconCheck class="size-3.5" />
                </button>
            </Show>

            <Show when={props.accounts.length > 1}>
                <button
                    type="button"
                    class={iconButton}
                    title={t("drift.provider.accounts.moveUp")}
                    aria-label={t("drift.provider.accounts.moveUp")}
                    disabled={busy() || index() === 0}
                    onClick={() => move(index(), -1)}
                >
                    <IconArrowUp class="size-3.5" />
                </button>
                <button
                    type="button"
                    class={iconButton}
                    title={t("drift.provider.accounts.moveDown")}
                    aria-label={t("drift.provider.accounts.moveDown")}
                    disabled={busy() || index() === props.accounts.length - 1}
                    onClick={() => move(index(), 1)}
                >
                    <IconArrowDown class="size-3.5" />
                </button>
            </Show>

            <button
                type="button"
                class="flex size-7 shrink-0 items-center justify-center rounded-md border border-danger/40 text-danger transition-colors hover:bg-danger/10 disabled:opacity-40"
                title={t("drift.provider.accounts.remove")}
                aria-label={t("drift.provider.accounts.remove")}
                disabled={busy()}
                onClick={() => void remove(account, index())}
            >
                <IconTrash class="size-3.5" />
            </button>
        </div>
    );

    return (
        <div class="space-y-1.5">
            <div class="text-[0.68rem] tracking-wider text-ink-faint uppercase">
                {t("drift.provider.accounts.title")}
            </div>
            <For each={props.accounts}>{row}</For>
            <div class="text-xs text-ink-faint">{t("drift.provider.accounts.hint")}</div>
        </div>
    );
}
