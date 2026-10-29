import { createStore } from "solid-js/store";
import { backendInvoke } from "../backend";
import { t } from "./i18n";

import type { MessageEntry } from "../engine/store";

type UsageWindowKind = "session" | "weekly" | "monthly" | "period";
export type UsageWindow = { kind: UsageWindowKind; label: string | null; usedPercent: number; resetsAt: number | null };
export type ProviderUsage = { status: "ok" | "expired" | "unsubscribed"; plan: string | null; windows: UsageWindow[] };
/** One account's plan usage; `failed` when its last request failed, keeping the usage it showed before. */
export type AccountUsage = { key: string; label: string | null; usage: ProviderUsage | null; failed: boolean };
/** A provider's accounts in use order; `accounts` is null when the provider reports no plan usage. */
export type UsageEntry = { accounts: AccountUsage[] | null; failed: boolean; fetchedAt: number; loading: boolean };
export type UsageTone = "normal" | "warn" | "danger";

/** One account as `provider_usage` answers; `usage` is null when its request failed. */
type ReportedAccount = Omit<AccountUsage, "failed">;

// Plan usage moves slowly and the endpoints are private, so hovering never polls faster than this.
const freshMs = 60_000;
const warnPercent = 70;
const dangerPercent = 90;
const minuteMs = 60_000;
const hourMs = 60 * minuteMs;
const dayMs = 24 * hourMs;

const [entries, setEntries] = createStore<Record<string, UsageEntry>>({});

export function usageFor(provider: string): UsageEntry | undefined {
    return entries[provider];
}

export async function refreshUsage(provider: string, now = Date.now(), force = false) {
    const current = entries[provider];
    if (current?.loading || (!force && current && now - current.fetchedAt < freshMs)) return;

    const invoke = backendInvoke();
    if (!invoke) return;

    const previous = current?.accounts ?? null;
    setEntries(provider, {
        accounts: previous,
        failed: false,
        fetchedAt: current?.fetchedAt ?? 0,
        loading: true,
    });
    const result = await invoke<ReportedAccount[] | null>("provider_usage", { provider }).then(
        (reported) => ({ accounts: reported && merged(reported, previous), failed: false }),
        () => ({ accounts: previous, failed: true }),
    );
    setEntries(provider, { ...result, fetchedAt: Date.now(), loading: false });
}

/** An account whose request failed keeps the usage it showed before. */
function merged(reported: ReportedAccount[], previous: AccountUsage[] | null): AccountUsage[] {
    return reported.map((account) => {
        if (account.usage) return { ...account, failed: false };

        const before = previous?.find((old) => old.key === account.key);
        return { ...account, usage: before?.usage ?? null, failed: true };
    });
}

/** The chat's own accounts: the one the user set it to use, and the one that sent its latest reply. */
export type ChatAccounts = { pinned?: string | null; last?: string | null };

function hasRoom(account: AccountUsage) {
    const usage = account.usage;
    return usage?.status === "ok" && usage.windows.every((window) => window.usedPercent < 100);
}

/** Usage that says the account cannot take a turn; unknown usage does not. */
function spent(account: AccountUsage) {
    return !!account.usage && !hasRoom(account);
}

/**
 * The account the chat's next turn would use, as the engine picks it: the pinned one while it has usage left, else the
 * one the chat is on while it has, else the first with usage left, else the one the chat is on, else the first.
 */
export function activeAccount(entry: UsageEntry | undefined, chat: ChatAccounts = {}): AccountUsage | undefined {
    const accounts = entry?.accounts ?? [];
    const named = (key: string | null | undefined) => accounts.find((account) => account.key === key);
    const own = [named(chat.pinned), named(chat.last)].find((account) => account && !spent(account));

    return own ?? accounts.find(hasRoom) ?? named(chat.last) ?? accounts[0];
}

/** The account that sent the chat's latest reply from this provider. */
export function lastAccount(entries: MessageEntry[], provider: string): string | undefined {
    for (let index = entries.length - 1; index >= 0; index--) {
        const info = entries[index]!.info;
        if (info.role === "assistant" && info.model?.provider === provider && info.account) return info.account;
    }

    return undefined;
}

export function usageTone(percent: number): UsageTone {
    if (percent >= dangerPercent) return "danger";
    if (percent >= warnPercent) return "warn";
    return "normal";
}

export function windowLabel(window: UsageWindow) {
    if (window.kind === "session") return t("drift.usage.session");
    if (window.kind === "weekly")
        return window.label ? t("drift.usage.weeklyModel", { model: window.label }) : t("drift.usage.weekly");
    if (window.kind === "period") return t("drift.usage.period");
    if (window.label === "premium") return t("drift.usage.premium");
    if (window.label === "chat") return t("drift.usage.chat");
    return t("drift.usage.monthly");
}

export function resetLabel(resetsAt: number | null, now = Date.now()) {
    if (resetsAt === null) return "";
    const remaining = resetsAt - now;
    if (remaining <= 0) return t("drift.usage.resetsSoon");
    if (remaining < hourMs)
        return t("drift.usage.resetsInMinutes", { minutes: Math.max(1, Math.round(remaining / minuteMs)) });
    if (remaining < dayMs) {
        const hours = Math.floor(remaining / hourMs);
        return t("drift.usage.resetsInHours", { hours, minutes: Math.round((remaining % hourMs) / minuteMs) });
    }
    return t("drift.usage.resetsInDays", {
        days: Math.floor(remaining / dayMs),
        hours: Math.floor((remaining % dayMs) / hourMs),
    });
}

export function resetTitle(resetsAt: number | null) {
    if (resetsAt === null) return undefined;
    const options: Intl.DateTimeFormatOptions = {
        weekday: "short",
        month: "short",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
    };
    return t("drift.usage.resetsAt", { time: new Intl.DateTimeFormat(undefined, options).format(resetsAt) });
}

export function planLabel(plan: string) {
    return plan.charAt(0).toUpperCase() + plan.slice(1);
}
