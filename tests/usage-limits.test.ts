import { estimateContextBreakdown } from "../src/engine/context-breakdown";
import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import {
    activeAccount,
    refreshUsage,
    resetLabel,
    resetTitle,
    usageFor,
    usageTone,
    windowLabel,
} from "../src/state/usage-limits";

import type { AccountUsage, ProviderUsage, UsageEntry } from "../src/state/usage-limits";
import type { MessageEntry } from "../src/engine/store";

const minute = 60_000;
const now = Date.UTC(2026, 8, 28, 12, 0);
let invoke: ReturnType<typeof mock>;
let original: PropertyDescriptor | undefined;

beforeEach(() => {
    original = Object.getOwnPropertyDescriptor(globalThis, "__TAURI__");
    invoke = mock(async () => [
        {
            key: "anthropic",
            label: null,
            usage: {
                status: "ok",
                plan: "max",
                windows: [{ kind: "session", label: null, usedPercent: 91, resetsAt: now + 49 * minute }],
            },
        },
    ]);
    Object.defineProperty(globalThis, "__TAURI__", { configurable: true, writable: true, value: { core: { invoke } } });
});

afterEach(() => {
    if (original) Object.defineProperty(globalThis, "__TAURI__", original);
    else delete (globalThis as Record<string, unknown>).__TAURI__;
});

test("limit bars turn amber at 70 percent and red at 90 percent", () => {
    expect(usageTone(0)).toBe("normal");
    expect(usageTone(69.9)).toBe("normal");
    expect(usageTone(70)).toBe("warn");
    expect(usageTone(89)).toBe("warn");
    expect(usageTone(90)).toBe("danger");
    expect(usageTone(100)).toBe("danger");
});

test("reset labels always count down and the hover title names the exact time", () => {
    expect(resetLabel(null, now)).toBe("");
    expect(resetLabel(now - 1, now)).toBe("Resetting now");
    expect(resetLabel(now + 49 * minute, now)).toBe("Resets in 49 min");
    expect(resetLabel(now + 20 * 1000, now)).toBe("Resets in 1 min");
    expect(resetLabel(now + (3 * 60 + 12) * minute, now)).toBe("Resets in 3 hr 12 min");
    expect(resetLabel(now + (3 * 24 * 60 + 5 * 60 + 30) * minute, now)).toBe("Resets in 3 d 5 hr");
    expect(resetTitle(null)).toBeUndefined();
    expect(resetTitle(now)).toMatch(/^Resets \S+/);
});

test("window labels follow the window kind and any model scope", () => {
    const base = { usedPercent: 0, resetsAt: null };
    expect(windowLabel({ ...base, kind: "session", label: null })).toBe("5-hour limit");
    expect(windowLabel({ ...base, kind: "weekly", label: null })).toBe("Weekly · all models");
    expect(windowLabel({ ...base, kind: "weekly", label: "Opus" })).toBe("Weekly · Opus");
    expect(windowLabel({ ...base, kind: "monthly", label: "premium" })).toBe("Premium requests");
    expect(windowLabel({ ...base, kind: "period", label: null })).toBe("Billing period");
});

test("usage is fetched once per minute per provider and keeps the last value on failure", async () => {
    await refreshUsage("anthropic");
    expect(invoke.mock.calls).toEqual([["provider_usage", { provider: "anthropic" }]]);
    expect(usageFor("anthropic")?.accounts?.[0]?.usage?.plan).toBe("max");
    await refreshUsage("anthropic");
    expect(invoke).toHaveBeenCalledTimes(1);

    invoke.mockRejectedValue(new Error("no engine"));
    await refreshUsage("anthropic", Date.now() + 2 * minute);
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(usageFor("anthropic")).toMatchObject({
        failed: true,
        loading: false,
        accounts: [{ key: "anthropic", usage: { plan: "max" } }],
    });
});

test("providers without plan windows resolve to null, which hides the section", async () => {
    invoke.mockResolvedValue(null);
    await refreshUsage("openrouter");
    expect(usageFor("openrouter")).toMatchObject({ accounts: null, failed: false });
});

test("every account comes back in order, and one whose request failed keeps what it showed before", async () => {
    const plan = (name: string, usedPercent: number): ProviderUsage => ({
        status: "ok",
        plan: name,
        windows: [{ kind: "weekly", label: null, usedPercent, resetsAt: null }],
    });
    invoke.mockResolvedValue([
        { key: "openai", label: "ann@example.com", usage: plan("pro", 40) },
        { key: "openai~b0b", label: null, usage: plan("plus", 12) },
    ]);
    await refreshUsage("openai");
    expect(usageFor("openai")?.accounts).toEqual([
        { key: "openai", label: "ann@example.com", usage: plan("pro", 40), failed: false },
        { key: "openai~b0b", label: null, usage: plan("plus", 12), failed: false },
    ]);

    invoke.mockResolvedValue([
        { key: "openai", label: "ann@example.com", usage: plan("pro", 55) },
        { key: "openai~b0b", label: null, usage: null },
    ]);
    await refreshUsage("openai", Date.now(), true);
    expect(usageFor("openai")).toMatchObject({
        failed: false,
        accounts: [
            { key: "openai", usage: { plan: "pro", windows: [{ usedPercent: 55 }] }, failed: false },
            { key: "openai~b0b", usage: { plan: "plus", windows: [{ usedPercent: 12 }] }, failed: true },
        ],
    });
});

test("the meter shows the first account with usage left, else the first", () => {
    const account = (key: string, usage: ProviderUsage | null): AccountUsage => ({
        key,
        label: null,
        usage,
        failed: false,
    });
    const used = (...percents: number[]): ProviderUsage => ({
        status: "ok",
        plan: null,
        windows: percents.map((usedPercent) => ({ kind: "session", label: null, usedPercent, resetsAt: null })),
    });
    const entry = (...accounts: AccountUsage[]): UsageEntry => ({
        accounts,
        failed: false,
        fetchedAt: now,
        loading: false,
    });
    const expired: ProviderUsage = { status: "expired", plan: null, windows: [] };

    const spent = account("a", used(100, 20));
    const free = account("b", used(99, 99));
    expect(activeAccount(entry(spent, free))).toBe(free);
    expect(activeAccount(entry(account("c", expired), account("d", null), free))).toBe(free);
    expect(activeAccount(entry(spent, account("e", used(100))))).toBe(spent);
    expect(activeAccount(entry(account("f", used())))?.key).toBe("f");
    expect(activeAccount(entry())).toBeUndefined();
    expect(activeAccount(undefined)).toBeUndefined();
});

function entry(role: "user" | "assistant", parts: unknown[], extra: Record<string, unknown> = {}): MessageEntry {
    return {
        info: { id: `${role}-${Math.random()}`, role, time: { created: 0 }, ...extra },
        parts,
    } as unknown as MessageEntry;
}

test("the breakdown estimates transcript categories and attributes the rest to system and tools", () => {
    const entries = [
        entry("user", [{ type: "text", text: "x".repeat(400) }]),
        entry("assistant", [
            { type: "text", text: "y".repeat(800) },
            { type: "tool_call", name: "bash", status: "done", input: { path: "a" }, output: "z".repeat(1184) },
        ]),
    ];
    expect(estimateContextBreakdown(entries, 10_000)).toEqual([
        { key: "system", tokens: 9_400 },
        { key: "user", tokens: 100 },
        { key: "assistant", tokens: 200 },
        { key: "tool", tokens: 300 },
    ]);
});

test("the breakdown ignores messages before the latest compaction and scales down overestimates", () => {
    const entries = [
        entry("user", [{ type: "text", text: "old".repeat(10_000) }]),
        entry("assistant", [{ type: "text", text: "s".repeat(400) }], { summary: true }),
        entry("user", [{ type: "text", text: "u".repeat(400) }]),
    ];
    expect(estimateContextBreakdown(entries, 1_000)).toEqual([
        { key: "system", tokens: 800 },
        { key: "user", tokens: 100 },
        { key: "assistant", tokens: 100 },
    ]);
    const scaled = estimateContextBreakdown(entries, 100);
    expect(scaled.reduce((sum, segment) => sum + segment.tokens, 0)).toBe(100);
    expect(estimateContextBreakdown(entries, 0)).toEqual([]);
});

test("the context meter shows usage limits and the breakdown, and remote access can ask for usage", async () => {
    const meter = await Bun.file("src/ui/context-meter.tsx").text();
    expect(meter).toContain("data-context-bar");
    const { dict, drift } = await import("../src/i18n/en");
    const english: Record<string, string> = { ...dict, ...drift };
    const keys = [...meter.matchAll(/"((?:drift|context)\.[a-zA-Z.]+)"/g)].map((match) => match[1]!);
    expect(keys.length).toBeGreaterThan(5);
    expect(keys.filter((key) => !(key in english))).toEqual([]);
    expect(meter).toContain("<UsageSection provider=");
    expect(meter).toContainCode("const account = () => activeAccount(entry());");
    expect(meter).toContainCode("<AccountLimits entry={entry()} account={account()} />");
    expect(meter).toContain("<ProviderIcon id={props.provider}");
    expect(meter).not.toContain("detailedBreakdown");
    expect(meter).toContain("onMouseEnter={openPopover}");
    expect(meter).toContainCode("const openPopover = () => { setHovered(true); refresh(); };");
    expect(meter).toContain("<Show when={hovered()}>");
    expect(await Bun.file("src/ui/header.tsx").text()).toContain("<ContextMeter sessionId=");
    expect(await Bun.file("src/ui/debug.tsx").text()).toContain("<ContextSection sessionId=");
    const remote = await Bun.file("src-tauri/src/remote.rs").text();
    expect(remote).toContain(
        '"provider_usage" => value(crate::usage_limits::provider_usage(app.state(), arg(args, "provider")?).await?)',
    );
    expect(await Bun.file("src-tauri/src/main.rs").text()).toContain("usage_limits::provider_usage,");
});

test("settings lists usage for every linked provider and forced refresh skips the one-minute cache", async () => {
    await refreshUsage("zai-coding-plan");
    const calls = invoke.mock.calls.length;
    await refreshUsage("zai-coding-plan");
    expect(invoke.mock.calls.length).toBe(calls);
    await refreshUsage("zai-coding-plan", Date.now(), true);
    expect(invoke.mock.calls.length).toBe(calls + 1);
    const settings = await Bun.file("src/ui/settings.tsx").text();
    const navigation = await Bun.file("src/ui/settings-search.ts").text();
    expect(navigation).toContainCode(
        'items: ["Tools", "Browser", "Providers", "Usage", "Skills", "MCP", "Plugins", "Prompts", "Permissions"]',
    );
    expect(settings).toContainCode("<UsageLimitsSection />");
    const section = await Bun.file("src/ui/settings-usage.tsx").text();
    expect(section).toContain("engine.state.connected.includes(provider.id)");
    expect(section).toContain("usageFor(provider.id)?.accounts !== null");
    expect(section).toContainCode("const several = () => accounts().length > 1;");
    expect(section).toContainCode('account.label ?? t("drift.provider.accounts.unnamed", { number: index + 1 })');
    expect(section).toContainCode(
        "<Show when={several()} fallback={<AccountLimits entry={entry()} account={accounts()[0]} />}>",
    );
});
