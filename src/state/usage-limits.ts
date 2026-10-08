import { createStore } from "solid-js/store";
import { backendInvoke } from "../backend";
import { t } from "./i18n";

type UsageWindowKind = "session" | "weekly" | "monthly" | "period";
export type UsageWindow = { kind: UsageWindowKind; label: string | null; usedPercent: number; resetsAt: number | null };
export type ProviderUsage = { status: "ok" | "expired" | "unsubscribed"; plan: string | null; windows: UsageWindow[] };
export type UsageEntry = { usage: ProviderUsage | null; failed: boolean; fetchedAt: number; loading: boolean };
export type UsageTone = "normal" | "warn" | "danger";

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
    setEntries(provider, {
        usage: current?.usage ?? null,
        failed: false,
        fetchedAt: current?.fetchedAt ?? 0,
        loading: true,
    });
    const result = await invoke<ProviderUsage | null>("provider_usage", { provider }).then(
        (usage) => ({ usage, failed: false }),
        () => ({ usage: current?.usage ?? null, failed: true }),
    );
    setEntries(provider, { ...result, fetchedAt: Date.now(), loading: false });
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
