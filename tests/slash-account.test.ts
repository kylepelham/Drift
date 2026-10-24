import { runSlash, slashItem, slashItems, slashPresets } from "../src/ui/slash";
import { applyMirroredSession, selectedSession } from "../src/state/selection";
import { afterEach, expect, mock, test } from "bun:test";

import type { Engine } from "../src/engine";

const previous = selectedSession();
afterEach(() => applyMirroredSession(previous));

function engineWith(accounts: { id: string; label?: string; cyberVerified?: boolean }[], pinned?: string) {
    applyMirroredSession("ses");
    const setSessionAccount = mock(async (_session: string, _account: string | null) => {});
    const notice = mock((_notice: { message: string }) => {});
    const engine = {
        state: {
            commands: [],
            connection: "online",
            connected: ["anthropic"],
            providers: [{ id: "anthropic", name: "Anthropic", models: { "claude-opus": {} } }],
            providerAccounts: {
                anthropic: accounts.map((account) => ({ signedIn: true, cyberVerified: false, ...account })),
            },
            sessions: {
                ses: {
                    id: "ses",
                    agent: "build",
                    model: { provider: "anthropic", model: "claude-opus" },
                    account: pinned,
                },
            },
        },
        actions: { setSessionAccount, notice },
    } as unknown as Engine;

    return { engine, setSessionAccount, notice };
}

test("/account is offered only when the chat's provider has more than one account", () => {
    expect(slashItem(engineWith([{ id: "anthropic" }]).engine, "account")).toBeUndefined();
    expect(slashItem(engineWith([{ id: "anthropic" }, { id: "anthropic~b" }]).engine, "account")?.name).toBe("account");
});

test("/account lists Auto and each account by name, marking the one in use", () => {
    const { engine } = engineWith(
        [
            { id: "anthropic", label: "main@example.com" },
            { id: "anthropic~b", cyberVerified: true },
        ],
        "anthropic~b",
    );
    const item = slashItems(engine, "account")[0];
    const presets = slashPresets(item, "");

    expect(presets.map((preset) => preset.value)).toEqual(["auto", "main@example.com", "Account 2"]);
    expect(presets.every((preset) => preset.execute)).toBe(true);
    expect(presets[2].description).toStartWith("In use. Cyber Verification. ");
    expect(presets[0].description).not.toStartWith("In use.");
    expect(slashPresets(item, "main").map((preset) => preset.value)).toEqual(["main@example.com"]);
});

test("/account sets the chat by name or number, clears it with auto, and says when nothing matches", async () => {
    const { engine, setSessionAccount, notice } = engineWith([
        { id: "anthropic", label: "main@example.com" },
        { id: "anthropic~b", label: "security@example.com" },
    ]);
    const item = slashItem(engine, "account")!;

    await runSlash(engine, item, "Security@Example.com");
    await runSlash(engine, item, "1");
    await runSlash(engine, item, "auto");
    await runSlash(engine, item, "nobody");

    expect(setSessionAccount.mock.calls).toEqual([
        ["ses", "anthropic~b"],
        ["ses", "anthropic"],
        ["ses", null],
    ]);
    expect(notice.mock.calls.at(-1)?.[0].message).toBe('No Anthropic account matches "nobody".');
});
