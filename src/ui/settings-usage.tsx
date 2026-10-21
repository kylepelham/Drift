import { planLabel, refreshUsage, usageFor } from "../state/usage-limits";
import { createEffect, createMemo, For, Show } from "solid-js";
import { AccountLimits } from "./context-meter";
import { ProviderIcon } from "./provider-icon";
import { useEngine } from "../engine";
import { t } from "../state/i18n";

import type { AccountUsage } from "../state/usage-limits";

export function UsageLimitsSection() {
    const engine = useEngine();
    const linked = createMemo(() =>
        engine.state.providers
            .filter((provider) => engine.state.connected.includes(provider.id))
            .sort((a, b) => a.name.localeCompare(b.name)),
    );
    createEffect(() => {
        for (const provider of linked()) void refreshUsage(provider.id);
    });
    // Providers without plan windows answer null; they are left out rather than listed as empty.
    const reporting = () => linked().filter((provider) => usageFor(provider.id)?.accounts !== null);
    const loading = () => linked().some((provider) => !usageFor(provider.id) || usageFor(provider.id)?.loading);
    const refreshAll = () => {
        for (const provider of linked()) void refreshUsage(provider.id, Date.now(), true);
    };
    return (
        <div class="space-y-3">
            <div class="flex items-center justify-between gap-4">
                <p class="text-[length:calc(0.72rem*var(--settings-scale,1))] leading-relaxed text-ink-faint">
                    {t("drift.usage.settingsDescription")}
                </p>
                <button
                    class="h-8 shrink-0 rounded-md border border-edge px-3 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                    disabled={loading()}
                    onClick={refreshAll}
                >
                    {t("drift.usage.refresh")}
                </button>
            </div>
            <Show
                when={reporting().length || loading()}
                fallback={<p class="text-xs text-ink-faint">{t("drift.usage.none")}</p>}
            >
                <div class="space-y-2" data-usage-settings>
                    <For each={reporting()}>
                        {(provider) => <ProviderLimits providerId={provider.id} providerName={provider.name} />}
                    </For>
                </div>
            </Show>
        </div>
    );
}

/** A provider's plan usage; with several accounts, one block each in use order under the provider's header. */
function ProviderLimits(props: { providerId: string; providerName: string }) {
    const entry = () => usageFor(props.providerId);
    const accounts = () => entry()?.accounts ?? [];
    const several = () => accounts().length > 1;
    const name = (account: AccountUsage, index: number) =>
        account.label ?? t("drift.provider.accounts.unnamed", { number: index + 1 });

    return (
        <section class="space-y-2.5 rounded-xl border border-edge bg-raised/25 px-3 py-3 text-xs">
            <div class="flex items-center gap-2.5">
                <span class="flex size-7 shrink-0 items-center justify-center rounded-lg border border-edge bg-surface text-ink-muted">
                    <ProviderIcon id={props.providerId} class="size-4" />
                </span>
                <span class="min-w-0 flex-1 truncate text-sm font-medium text-ink">{props.providerName}</span>
                <Show when={!several()}>
                    <PlanBadge plan={accounts()[0]?.usage?.plan} />
                </Show>
            </div>
            <Show when={several()} fallback={<AccountLimits entry={entry()} account={accounts()[0]} />}>
                <For each={accounts()}>
                    {(account, index) => (
                        <div class="space-y-2 border-t border-edge pt-2.5" data-usage-account>
                            <div class="flex items-center gap-2">
                                <span class="min-w-0 flex-1 truncate font-medium text-ink-muted">
                                    {name(account, index())}
                                </span>
                                <PlanBadge plan={account.usage?.plan} />
                            </div>
                            <AccountLimits entry={entry()} account={account} />
                        </div>
                    )}
                </For>
            </Show>
        </section>
    );
}

function PlanBadge(props: { plan: string | null | undefined }) {
    return (
        <Show when={props.plan}>
            {(plan) => (
                <span class="shrink-0 rounded-full border border-edge px-2 py-0.5 text-[length:calc(0.68rem*var(--settings-scale,1))] text-ink-muted">
                    {planLabel(plan())}
                </span>
            )}
        </Show>
    );
}
